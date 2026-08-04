pub(super) async fn run_disposable_systemd_user() {
    use std::{
        fs,
        io::Write as _,
        os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        path::{Path, PathBuf},
        process::Command,
        time::Duration,
    };

    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio::net::UnixListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::*;

    const OPT_IN: &str = "CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER";
    const HELPER: &str = "CODEX_SESSION_CONTROL_SYSTEMD_HELPER";
    const HELPER_SOCKET: &str = "CODEX_SESSION_CONTROL_SYSTEMD_HELPER_SOCKET";
    const HELPER_LOG: &str = "CODEX_SESSION_CONTROL_SYSTEMD_HELPER_LOG";

    if std::env::var_os(HELPER).as_deref() == Some(std::ffi::OsStr::new("1")) {
        let socket = PathBuf::from(std::env::var_os(HELPER_SOCKET).unwrap());
        let log = PathBuf::from(std::env::var_os(HELPER_LOG).unwrap());
        if let Ok(previous) = fs::read_to_string(&log) {
            let mut fields = previous
                .lines()
                .last()
                .unwrap()
                .split_whitespace()
                .map(|field| field.parse::<u32>().unwrap());
            let previous_parent = fields.next().unwrap();
            let previous_child = fields.next().unwrap();
            assert!(
                !Path::new(&format!("/proc/{previous_parent}")).exists(),
                "replacement started before the old main process exited"
            );
            assert!(
                !Path::new(&format!("/proc/{previous_child}")).exists(),
                "replacement started before the old child process exited"
            );
        }
        let mut child = Command::new("sleep").arg("600").spawn().unwrap();
        let child_id = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let mut log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        writeln!(
            log,
            "{} {} {}",
            std::process::id(),
            child_id,
            std::os::unix::process::parent_id()
        )
        .unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut websocket = accept_async(stream).await.unwrap();
                let Message::Text(initialize) = websocket.next().await.unwrap().unwrap() else {
                    return;
                };
                let initialize: Value = serde_json::from_str(initialize.as_str()).unwrap();
                let codex_home = std::env::var("CODEX_HOME").unwrap();
                websocket
                    .send(Message::text(
                        systemd_helper_initialize_response(&initialize["id"], &codex_home)
                            .to_string(),
                    ))
                    .await
                    .unwrap();
                let initialized = websocket.next().await.unwrap().unwrap();
                assert_eq!(
                    initialized.into_text().unwrap(),
                    json!({"method": "initialized"}).to_string()
                );
                while let Some(Ok(Message::Text(request))) = websocket.next().await {
                    let request: Value = serde_json::from_str(request.as_str()).unwrap();
                    if request["method"] == "thread/list" {
                        websocket
                            .send(Message::text(
                                json!({
                                    "id": request["id"],
                                    "result": {"data": [], "nextCursor": null}
                                })
                                .to_string(),
                            ))
                            .await
                            .unwrap();
                    }
                }
            });
        }
    }

    if std::env::var_os(OPT_IN).as_deref() != Some(std::ffi::OsStr::new("1")) {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI must not skip the disposable systemd-user proof"
        );
        eprintln!("SKIP: disposable systemd-user prerequisites unavailable");
        return;
    }

    let euid = rustix::process::geteuid().as_raw();
    assert_ne!(euid, 0, "the disposable systemd test must not run as root");
    let user = uzers::get_user_by_uid(euid).expect("disposable passwd user is required");
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is required"));
    assert_eq!(user.home_dir(), home);
    let runtime =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is required"));
    let runtime_metadata = fs::metadata(&runtime).expect("runtime directory is required");
    assert_eq!(runtime_metadata.uid(), euid);
    assert_eq!(runtime_metadata.mode() & 0o777, 0o700);
    assert!(
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some(),
        "the disposable systemd user bus address is required"
    );

    let real_systemctl = ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .expect("systemctl is required");
    assert!(
        Command::new(&real_systemctl)
            .args(["--user", "list-units"])
            .status()
            .unwrap()
            .success(),
        "a running disposable systemd --user manager is required"
    );

    fn command_snapshot(systemctl: &Path, args: &[&str]) -> (i32, Vec<u8>, Vec<u8>) {
        let output = Command::new(systemctl).args(args).output().unwrap();
        (
            output.status.code().unwrap_or(-1),
            output.stdout,
            output.stderr,
        )
    }

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("systemd fixture did not converge");
    }

    fn main_pid(systemctl: &Path, unit: &str) -> u32 {
        let output = Command::new(systemctl)
            .args(["--user", "show", "--property=MainPID", "--value", unit])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn recorded_processes(path: &Path) -> (u32, u32, u32) {
        let line = fs::read_to_string(path)
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned();
        let mut fields = line.split_whitespace().map(|field| field.parse().unwrap());
        (
            fields.next().unwrap(),
            fields.next().unwrap(),
            fields.next().unwrap(),
        )
    }

    struct Cleanup {
        systemctl: PathBuf,
        unit: String,
        paths: ResolvedUserPaths,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Command::new(&self.systemctl)
                .args(["--user", "disable", "--now", &self.unit])
                .status();
            let _ = fs::remove_file(&self.paths.unit);
            let _ = Command::new(&self.systemctl)
                .args(["--user", "daemon-reload"])
                .status();
            let _ = fs::remove_dir_all(&self.paths.marketplace);
            let _ = fs::remove_file(&self.paths.config);
            let _ = fs::remove_file(&self.paths.manifest);
            let _ = fs::remove_file(&self.paths.binary);
            let _ = fs::remove_dir_all(&self.paths.codex_home);
            let desktop_dir = self.paths.home.join(".config/codex-desktop");
            let _ = fs::remove_file(desktop_dir.join("app-server-attachment.json"));
            let _ = fs::remove_dir(desktop_dir);
        }
    }

    let paths = ResolvedUserPaths::for_test(euid, home.clone(), runtime.clone());
    for absent in [
        &paths.binary,
        &paths.config,
        &paths.unit,
        &paths.marketplace,
        &paths.manifest,
        &paths.runtime_dir,
    ] {
        assert!(
            !absent.exists(),
            "disposable user is not clean: {}",
            absent.display()
        );
    }
    let nonce = format!("Systemd{}", std::process::id());
    let target = LifecycleTarget::suffixed(paths.clone(), &nonce);
    let paths = target.paths.clone();
    assert!(
        !paths.unit.exists(),
        "disposable suffixed unit path is not clean: {}",
        paths.unit.display()
    );
    let unit_name = target.unit_name.clone();
    let production_unit_before = command_snapshot(
        &real_systemctl,
        &[
            "--user",
            "show",
            "codex-session-control.service",
            "--no-pager",
        ],
    );
    let production_socket_before = fs::symlink_metadata(&paths.socket)
        .ok()
        .map(|metadata| (metadata.uid(), metadata.mode(), metadata.len()));
    let _cleanup = Cleanup {
        systemctl: real_systemctl.clone(),
        unit: unit_name.clone(),
        paths: paths.clone(),
    };

    let fixture_root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&home)
        .unwrap();
    let fake_bin = fixture_root.path().join("bin");
    fs::DirBuilder::new().mode(0o700).create(&fake_bin).unwrap();
    let systemctl_log = fixture_root.path().join("systemctl.log");
    let marketplace_state = fixture_root.path().join("marketplace");
    let plugin_state = fixture_root.path().join("plugin");
    let plugin_version = fixture_root.path().join("plugin-version");
    let helper_log = fixture_root.path().join("helper.log");
    let desktop_descriptor = home
        .join(".config/codex-desktop")
        .join("app-server-attachment.json");
    let current_test = std::env::current_exe().unwrap();
    let quoted_test = shell_quote_path(&current_test).unwrap();
    let quoted_socket = shell_quote_path(&paths.socket).unwrap();
    let quoted_helper_log = shell_quote_path(&helper_log).unwrap();
    let quoted_descriptor = shell_quote_path(&desktop_descriptor).unwrap();
    let quoted_marketplace = shell_quote_path(&marketplace_state).unwrap();
    let quoted_plugin = shell_quote_path(&plugin_state).unwrap();
    let quoted_plugin_version = shell_quote_path(&plugin_version).unwrap();
    let fake_codex = fake_bin.join("codex");
    write_executable_fixture(
        &fake_codex,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'codex-cli 0.146.0\n'; exit 0; fi
if [ "$1" = "app-server" ]; then
  export {HELPER}=1
  export {HELPER_SOCKET}={quoted_socket}
  export {HELPER_LOG}={quoted_helper_log}
  exec {quoted_test} --exact install::tests::disposable_systemd_user --ignored --nocapture
fi
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ] && [ "$3" = "list" ]; then
  if [ -f {quoted_marketplace} ]; then root=$(cat {quoted_marketplace}); printf '{{"marketplaces":[{{"name":"codex-session-control-local","root":"%s","marketplaceSource":{{"sourceType":"local","source":"%s"}}}}]}}\n' "$root" "$root"; else printf '{{"marketplaces":[]}}\n'; fi
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ] && [ "$3" = "add" ]; then printf '%s' "$4" > {quoted_marketplace}; printf '{{}}\n'; exit 0; fi
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ] && [ "$3" = "remove" ]; then rm -f {quoted_marketplace}; printf '{{}}\n'; exit 0; fi
if [ "$1" = "plugin" ] && [ "$2" = "list" ]; then
  if [ -f {quoted_plugin} ]; then root=$(cat {quoted_plugin}); version=$(cat {quoted_plugin_version}); printf '{{"installed":[{{"pluginId":"codex-session-control@codex-session-control-local","name":"codex-session-control","marketplaceName":"codex-session-control-local","version":"%s","installed":true,"enabled":true,"source":{{"source":"local","path":"%s/plugins/codex-session-control"}},"marketplaceSource":{{"sourceType":"local","source":"%s"}},"installPolicy":"AVAILABLE"}}],"available":[]}}\n' "$version" "$root" "$root"; else printf '{{"installed":[],"available":[]}}\n'; fi
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "add" ]; then root=$(cat {quoted_marketplace}); printf '%s' "$root" > {quoted_plugin}; sed -n 's/.*"version": "\([^"]*\)".*/\1/p' "$root/plugins/codex-session-control/.codex-plugin/plugin.json" > {quoted_plugin_version}; printf '{{}}\n'; exit 0; fi
if [ "$1" = "plugin" ] && [ "$2" = "remove" ]; then rm -f {quoted_plugin} {quoted_plugin_version}; printf '{{}}\n'; exit 0; fi
exit 64
"#
        ),
    );
    let fake_desktop = fixture_root.path().join("codex-desktop");
    write_executable_fixture(
        &fake_desktop,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let fake_systemctl = fake_bin.join("systemctl");
    write_executable_fixture(
        &fake_systemctl,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
if [ "$1" = "--user" ] && [ "$2" = "enable" ] && [ "$3" = "--now" ] && [ ! -f {quoted_descriptor} ]; then
  exit 69
fi
if [ "$1" = "--user" ] && [ "$2" = "disable" ] && [ "$3" = "--now" ] && [ ! -f {quoted_descriptor} ]; then
  exit 70
fi
'{}' "$@"
status=$?
if [ "$status" -eq 0 ] && [ "$1" = "--user" ] && [ "$2" = "disable" ] && [ "$3" = "--now" ]; then
  [ ! -S {quoted_socket} ] || exit 71
  [ -f {quoted_descriptor} ] || exit 72
fi
exit "$status"
"#,
            systemctl_log.display(),
            real_systemctl.display(),
        ),
    );
    let candidate = fixture_root.path().join("candidate");
    write_executable_fixture(
        &candidate,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            env!("CARGO_PKG_VERSION"),
            test_target()
        ),
    );
    let path_environment = std::env::join_paths(
        std::iter::once(fake_bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut desktop_environment = std::env::vars_os().collect::<BTreeMap<_, _>>();
    desktop_environment.insert(OsString::from("HOME"), home.as_os_str().to_owned());
    desktop_environment.insert(
        OsString::from("XDG_CONFIG_HOME"),
        home.join(".config").into_os_string(),
    );
    let setup_context = SetupContext {
        target: target.clone(),
        candidate: CandidateRelease {
            executable: candidate,
            product_version: env!("CARGO_PKG_VERSION").to_owned(),
            target: test_target().to_owned(),
        },
        path_environment: path_environment.clone(),
        desktop_environment: desktop_environment.clone(),
        desktop_launcher: Some(fake_desktop),
        cwd: home.clone(),
    };

    for (path, expected) in [
        (home.as_path(), FileKind::Directory),
        (fixture_root.path(), FileKind::Directory),
        (fake_bin.as_path(), FileKind::Directory),
        (fake_codex.as_path(), FileKind::RegularFile),
        (
            setup_context.desktop_launcher.as_deref().unwrap(),
            FileKind::RegularFile,
        ),
        (fake_systemctl.as_path(), FileKind::RegularFile),
        (
            setup_context.candidate.executable.as_path(),
            FileKind::RegularFile,
        ),
    ] {
        assert_disposable_systemd_fixture_path(path, expected, euid);
    }
    setup_with_context(setup_context).await.unwrap();
    wait_until(|| paths.socket.exists() && main_pid(&real_systemctl, &unit_name) != 0).await;
    assert!(desktop_descriptor.is_file());
    let first_pid = main_pid(&real_systemctl, &unit_name);
    let first_socket_inode = fs::symlink_metadata(&paths.socket).unwrap().ino();
    let (recorded_pid, first_child, recorded_parent) = recorded_processes(&helper_log);
    assert_eq!(recorded_pid, first_pid);
    let proc_parent: u32 = fs::read_to_string(format!("/proc/{first_pid}/status"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("PPid:\t"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(recorded_parent, proc_parent);
    assert_eq!(
        fs::read_to_string(format!("/proc/{proc_parent}/comm"))
            .unwrap()
            .trim(),
        "systemd"
    );
    let child_parent: u32 = fs::read_to_string(format!("/proc/{first_child}/status"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("PPid:\t"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(child_parent, first_pid);

    assert!(
        Command::new("kill")
            .args(["-KILL", &first_pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_until(|| {
        let replacement = main_pid(&real_systemctl, &unit_name);
        replacement != 0
            && replacement != first_pid
            && paths.socket.exists()
            && !Path::new(&format!("/proc/{first_pid}")).exists()
            && !Path::new(&format!("/proc/{first_child}")).exists()
    })
    .await;
    let restarted_pid = main_pid(&real_systemctl, &unit_name);
    assert_ne!(
        fs::symlink_metadata(&paths.socket).unwrap().ino(),
        first_socket_inode
    );

    let lifecycle = || LifecycleContext {
        target: target.clone(),
        path_environment: path_environment.clone(),
        desktop_environment: desktop_environment.clone(),
        cwd: home.clone(),
    };
    disable_with_context(lifecycle()).await.unwrap();
    let starts_after_disable = fs::read_to_string(&helper_log).unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        fs::read_to_string(&helper_log).unwrap(),
        starts_after_disable
    );
    assert!(!Path::new(&format!("/proc/{restarted_pid}")).exists());
    assert!(!paths.socket.exists());
    assert!(!desktop_descriptor.exists());

    enable_with_context(lifecycle()).await.unwrap();
    assert!(desktop_descriptor.is_file());
    let enabled_pid = main_pid(&real_systemctl, &unit_name);
    let update_candidate = fixture_root.path().join("update-candidate");
    write_executable_fixture(
        &update_candidate,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control 0.2.0 ({})\\n'; exit 0; fi\nexit 64\n",
            product_target()
        ),
    );
    staged_update_with_context(UpdateContext {
        lifecycle: lifecycle(),
        candidate: update_candidate,
        terminal: TerminalState::noninteractive(),
    })
    .await
    .unwrap();
    assert_eq!(main_pid(&real_systemctl, &unit_name), enabled_pid);
    uninstall_with_context(lifecycle()).await.unwrap();
    assert!(
        !Command::new(&real_systemctl)
            .args(["--user", "is-active", "--quiet", &unit_name])
            .status()
            .unwrap()
            .success()
    );
    assert!(!paths.unit.exists());
    assert!(!paths.socket.exists());
    assert!(!desktop_descriptor.exists());

    let log = fs::read_to_string(&systemctl_log).unwrap();
    for line in log.lines().filter(|line| !line.contains("daemon-reload")) {
        assert!(line.contains(&unit_name), "{line}");
        assert!(!line.contains("codex-session-control.service"), "{line}");
    }
    assert_eq!(
        command_snapshot(
            &real_systemctl,
            &[
                "--user",
                "show",
                "codex-session-control.service",
                "--no-pager",
            ],
        ),
        production_unit_before
    );
    assert_eq!(
        fs::symlink_metadata(&paths.socket).ok().map(|metadata| (
            metadata.uid(),
            metadata.mode(),
            metadata.len()
        )),
        production_socket_before
    );
}
