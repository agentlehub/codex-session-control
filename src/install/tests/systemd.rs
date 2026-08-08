pub(super) async fn run_disposable_systemd_user() {
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        fs,
        io::Write as _,
        os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::Duration,
    };

    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use tokio::net::UnixListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use crate::install::{display_command_for_paths, service::run_systemctl};

    use super::*;

    const OPT_IN: &str = "CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER";
    const HELPER: &str = "CODEX_SESSION_CONTROL_SYSTEMD_HELPER";
    const HELPER_SOCKET: &str = "CODEX_SESSION_CONTROL_SYSTEMD_HELPER_SOCKET";
    const HELPER_LOG: &str = "CODEX_SESSION_CONTROL_SYSTEMD_HELPER_LOG";
    const MANAGED_PROBE: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE";
    const MANAGED_PROBE_STARTED: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_STARTED";
    const MANAGED_PROBE_GO: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_GO";
    const MANAGED_PROBE_RESULT: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_RESULT";
    const MANAGED_PROBE_EXIT: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_EXIT";
    const MANAGED_PROBE_STDOUT: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_STDOUT";
    const MANAGED_PROBE_STDERR: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_STDERR";
    const MANAGED_PROBE_UNIT: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_UNIT";
    const MANAGED_PROBE_SYSTEMCTL: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_SYSTEMCTL";
    const MANAGED_PROBE_REAL_SYSTEMCTL: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_REAL_SYSTEMCTL";
    const MANAGED_PROBE_BASE_PATH: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_BASE_PATH";
    const MANAGED_PROBE_RESTART_PATH: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_RESTART_PATH";
    const MANAGED_PROBE_NO_RESTART_CANDIDATE: &str =
        "CODEX_SESSION_CONTROL_MANAGED_PROBE_NO_RESTART_CANDIDATE";
    const MANAGED_PROBE_RESTART_CANDIDATE: &str =
        "CODEX_SESSION_CONTROL_MANAGED_PROBE_RESTART_CANDIDATE";
    const MANAGED_PROBE_WHOAMI_FAIL: &str = "CODEX_SESSION_CONTROL_MANAGED_PROBE_WHOAMI_FAIL";

    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    struct ManagedProbeResult {
        no_restart_stdout: String,
        no_restart_stderr: String,
        whoami_evidence: String,
        fallback_evidence: String,
        restart_error: String,
        disable_error: String,
        uninstall_error: String,
        unavailable_update_error: String,
        unavailable_disable_error: String,
        unavailable_uninstall_error: String,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct LiveFileSnapshot {
        path: PathBuf,
        bytes: Option<Vec<u8>>,
        owner: Option<u32>,
        mode: Option<u32>,
    }

    fn snapshot_live_file(path: PathBuf) -> LiveFileSnapshot {
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let bytes = fs::read(&path).unwrap();
                LiveFileSnapshot {
                    path,
                    bytes: Some(bytes),
                    owner: Some(metadata.uid()),
                    mode: Some(metadata.mode() & 0o7777),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => LiveFileSnapshot {
                path,
                bytes: None,
                owner: None,
                mode: None,
            },
            Err(error) => panic!("cannot snapshot guarded path {}: {error}", path.display()),
        }
    }

    fn snapshot_live_guarded_state(
        paths: &ResolvedUserPaths,
        desktop_descriptor: &Path,
    ) -> Vec<LiveFileSnapshot> {
        [
            paths.binary.clone(),
            paths.config.clone(),
            paths.unit.clone(),
            paths.manifest.clone(),
            desktop_descriptor.to_path_buf(),
            paths.codex_home.join("auth.json"),
            paths.codex_home.join("tasks/task-sentinel"),
            paths.codex_home.join("rollouts/rollout-sentinel"),
        ]
        .into_iter()
        .map(snapshot_live_file)
        .collect()
    }

    fn expected_restart_candidate_version() -> String {
        let mut version = semver::Version::parse(&higher_test_release_version()).unwrap();
        version.minor = version.minor.checked_add(1).unwrap();
        version.patch = 0;
        version.to_string()
    }

    fn bounded_probe_diagnostic(path: &Path) -> String {
        const MAX_BYTES: usize = 8 * 1024;

        match fs::read(path) {
            Ok(bytes) => {
                let truncated = bytes.len() > MAX_BYTES;
                let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BYTES)]);
                if truncated {
                    format!("{text}\n[truncated after {MAX_BYTES} bytes]")
                } else {
                    text.into_owned()
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                "<not created>".to_owned()
            }
            Err(error) => format!("<unreadable: {error}>"),
        }
    }

    if std::env::var_os(MANAGED_PROBE).as_deref() == Some(std::ffi::OsStr::new("1")) {
        let euid = rustix::process::geteuid().as_raw();
        let home = PathBuf::from(std::env::var_os("HOME").expect("managed probe HOME is required"));
        let runtime = PathBuf::from(
            std::env::var_os("XDG_RUNTIME_DIR").expect("managed probe runtime is required"),
        );
        let unit_name = std::env::var(MANAGED_PROBE_UNIT).unwrap();
        let nonce = unit_name
            .strip_prefix("codex-session-control-test-")
            .and_then(|name| name.strip_suffix(".service"))
            .expect("managed probe unit must be nonce-suffixed");
        let production_paths = ResolvedUserPaths::for_test(euid, home.clone(), runtime);
        let mut test_paths = production_paths.clone();
        test_paths
            .socket
            .set_file_name(format!("app-server-test-{nonce}.sock"));
        let target = LifecycleTarget::suffixed(test_paths, nonce);
        assert_eq!(target.unit_name, unit_name);
        let desktop_descriptor = home
            .join(".config/codex-desktop")
            .join("app-server-attachment.json");
        let fake_systemctl = PathBuf::from(std::env::var_os(MANAGED_PROBE_SYSTEMCTL).unwrap());
        let real_systemctl = PathBuf::from(std::env::var_os(MANAGED_PROBE_REAL_SYSTEMCTL).unwrap());
        let base_path = std::env::var_os(MANAGED_PROBE_BASE_PATH).unwrap();
        let restart_path = std::env::var_os(MANAGED_PROBE_RESTART_PATH).unwrap();
        let no_restart_candidate =
            PathBuf::from(std::env::var_os(MANAGED_PROBE_NO_RESTART_CANDIDATE).unwrap());
        let restart_candidate =
            PathBuf::from(std::env::var_os(MANAGED_PROBE_RESTART_CANDIDATE).unwrap());
        let whoami_fail = PathBuf::from(std::env::var_os(MANAGED_PROBE_WHOAMI_FAIL).unwrap());
        let mut desktop_environment = std::env::vars_os().collect::<BTreeMap<_, _>>();
        desktop_environment.insert(OsString::from("HOME"), home.as_os_str().to_owned());
        desktop_environment.insert(
            OsString::from("XDG_CONFIG_HOME"),
            home.join(".config").into_os_string(),
        );
        let base_lifecycle = LifecycleContext {
            target: target.clone(),
            path_environment: base_path,
            desktop_environment: desktop_environment.clone(),
            cwd: home.clone(),
        };
        let restart_lifecycle = LifecycleContext {
            target: target.clone(),
            path_environment: restart_path,
            desktop_environment,
            cwd: home.clone(),
        };
        let go = PathBuf::from(std::env::var_os(MANAGED_PROBE_GO).unwrap());
        tokio::time::timeout(Duration::from_secs(15), async {
            while !go.exists() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("outer harness did not release managed lifecycle probe");

        let production_unit_before = command_snapshot(
            &real_systemctl,
            &[
                "--user",
                "show",
                "codex-session-control.service",
                "--no-pager",
            ],
        );
        let production_socket_before = fs::symlink_metadata(&production_paths.socket)
            .ok()
            .map(|metadata| (metadata.uid(), metadata.mode(), metadata.len()));
        let assert_production_unchanged = || {
            assert_production_state(
                &real_systemctl,
                &production_paths.socket,
                &production_unit_before,
                &production_socket_before,
            );
        };
        let setup_manifest: Value =
            serde_json::from_slice(&fs::read(&target.paths.manifest).unwrap()).unwrap();
        assert_eq!(setup_manifest["schemaVersion"], 3);
        assert!(setup_manifest.get("codexVersion").is_none());
        let descriptor_before_no_restart = fs::read(&desktop_descriptor).unwrap();

        let no_restart = staged_update_with_context(UpdateContext {
            lifecycle: base_lifecycle.clone(),
            candidate: no_restart_candidate,
            terminal: TerminalState::noninteractive(),
        })
        .await
        .unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(&target.paths.manifest).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 3);
        assert!(manifest.get("codexVersion").is_none());
        assert_eq!(
            fs::read(&desktop_descriptor).unwrap(),
            descriptor_before_no_restart
        );
        assert_production_unchanged();
        assert_eq!(
            fs::read(target.paths.codex_home.join("auth.json")).unwrap(),
            b"auth sentinel"
        );
        assert_eq!(
            fs::read(target.paths.codex_home.join("tasks/task-sentinel")).unwrap(),
            b"task sentinel"
        );
        assert_eq!(
            fs::read(target.paths.codex_home.join("rollouts/rollout-sentinel")).unwrap(),
            b"rollout sentinel"
        );
        let guarded = snapshot_live_guarded_state(&target.paths, &desktop_descriptor);
        let whoami_evidence = format!("{:?}", inspect_caller_unit(&fake_systemctl, &target));

        let restart_error = staged_update_with_context(UpdateContext {
            lifecycle: restart_lifecycle.clone(),
            candidate: restart_candidate.clone(),
            terminal: TerminalState::noninteractive(),
        })
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(
            snapshot_live_guarded_state(&target.paths, &desktop_descriptor),
            guarded
        );
        assert_production_unchanged();

        let disable_error = disable_with_context(base_lifecycle.clone())
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            snapshot_live_guarded_state(&target.paths, &desktop_descriptor),
            guarded
        );
        assert_production_unchanged();

        let uninstall_error = uninstall_with_context(base_lifecycle.clone())
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            snapshot_live_guarded_state(&target.paths, &desktop_descriptor),
            guarded
        );
        assert_production_unchanged();

        struct RemoveOnDrop(PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _remove_whoami_fail = RemoveOnDrop(whoami_fail.clone());
        fs::write(&whoami_fail, b"fail whoami\n").unwrap();
        let fallback_evidence = format!("{:?}", inspect_caller_unit(&fake_systemctl, &target));
        let unavailable_update_error = staged_update_with_context(UpdateContext {
            lifecycle: restart_lifecycle,
            candidate: restart_candidate,
            terminal: TerminalState::noninteractive(),
        })
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(
            snapshot_live_guarded_state(&target.paths, &desktop_descriptor),
            guarded
        );
        assert_production_unchanged();
        let unavailable_disable_error = disable_with_context(base_lifecycle.clone())
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            snapshot_live_guarded_state(&target.paths, &desktop_descriptor),
            guarded
        );
        assert_production_unchanged();
        let unavailable_uninstall_error = uninstall_with_context(base_lifecycle)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            snapshot_live_guarded_state(&target.paths, &desktop_descriptor),
            guarded
        );
        assert_production_unchanged();
        fs::remove_file(&whoami_fail).unwrap();

        let result = ManagedProbeResult {
            no_restart_stdout: no_restart.stdout,
            no_restart_stderr: no_restart.stderr,
            whoami_evidence,
            fallback_evidence,
            restart_error,
            disable_error,
            uninstall_error,
            unavailable_update_error,
            unavailable_disable_error,
            unavailable_uninstall_error,
        };
        let result_path = PathBuf::from(std::env::var_os(MANAGED_PROBE_RESULT).unwrap());
        let result_stage = result_path.with_extension("stage");
        let mut result_bytes = serde_json::to_vec_pretty(&result).unwrap();
        result_bytes.push(b'\n');
        let mut result_file = fs::File::create(&result_stage).unwrap();
        result_file.write_all(&result_bytes).unwrap();
        result_file.sync_all().unwrap();
        drop(result_file);
        fs::rename(result_stage, result_path).unwrap();
        return;
    }

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
        if fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(std::env::var_os(MANAGED_PROBE_STARTED).unwrap())
            .is_ok()
        {
            let probe_exit = PathBuf::from(std::env::var_os(MANAGED_PROBE_EXIT).unwrap());
            let probe_stdout = fs::File::create(PathBuf::from(
                std::env::var_os(MANAGED_PROBE_STDOUT).unwrap(),
            ))
            .unwrap();
            let probe_stderr = fs::File::create(PathBuf::from(
                std::env::var_os(MANAGED_PROBE_STDERR).unwrap(),
            ))
            .unwrap();
            let mut probe = Command::new(std::env::current_exe().unwrap());
            probe
                .args([
                    "--exact",
                    "install::tests::disposable_systemd_user",
                    "--ignored",
                    "--nocapture",
                ])
                .env(MANAGED_PROBE, "1")
                .stdout(Stdio::from(probe_stdout))
                .stderr(Stdio::from(probe_stderr));
            let mut child = probe.spawn().unwrap_or_else(|error| {
                fs::write(&probe_exit, format!("spawn failed: {error}\n")).unwrap();
                panic!("managed lifecycle probe spawn failed: {error}");
            });
            fs::write(&probe_exit, format!("spawned pid {}\n", child.id())).unwrap();
            std::thread::spawn(move || {
                let outcome = match child.wait() {
                    Ok(status) => format!("exit status: {status}\n"),
                    Err(error) => format!("wait failed: {error}\n"),
                };
                let _ = fs::write(probe_exit, outcome);
            });
        }
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

    fn assert_production_state(
        real_systemctl: &Path,
        production_socket: &Path,
        production_unit_before: &(i32, Vec<u8>, Vec<u8>),
        production_socket_before: &Option<(u32, u32, u64)>,
    ) {
        assert_eq!(
            command_snapshot(
                real_systemctl,
                &[
                    "--user",
                    "show",
                    "codex-session-control.service",
                    "--no-pager",
                ],
            ),
            *production_unit_before
        );
        assert_eq!(
            fs::symlink_metadata(production_socket)
                .ok()
                .map(|metadata| (metadata.uid(), metadata.mode(), metadata.len())),
            *production_socket_before
        );
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

    let production_paths = ResolvedUserPaths::for_test(euid, home.clone(), runtime.clone());
    let production_socket = production_paths.socket.clone();
    for absent in [
        &production_paths.binary,
        &production_paths.config,
        &production_paths.unit,
        &production_paths.marketplace,
        &production_paths.manifest,
        &production_paths.runtime_dir,
    ] {
        assert!(
            !absent.exists(),
            "disposable user is not clean: {}",
            absent.display()
        );
    }
    let nonce = format!("Systemd{}", std::process::id());
    assert!(!production_socket.exists());
    let mut test_paths = production_paths.clone();
    test_paths
        .socket
        .set_file_name(format!("app-server-test-{nonce}.sock"));
    let target = LifecycleTarget::suffixed(test_paths, &nonce);
    let paths = target.paths.clone();
    assert_ne!(paths.socket, production_socket);
    assert!(!paths.socket.exists());
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
    let production_socket_before = fs::symlink_metadata(&production_socket)
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
    let managed_probe_started = fixture_root.path().join("managed-probe-started");
    let managed_probe_go = fixture_root.path().join("managed-probe-go");
    let managed_probe_result = fixture_root.path().join("managed-probe-result.json");
    let managed_probe_exit = fixture_root.path().join("managed-probe-exit");
    let managed_probe_stdout = fixture_root.path().join("managed-probe.stdout");
    let managed_probe_stderr = fixture_root.path().join("managed-probe.stderr");
    let whoami_fail = fixture_root.path().join("managed-probe-whoami-fail");
    let desktop_descriptor = home
        .join(".config/codex-desktop")
        .join("app-server-attachment.json");
    let current_test = std::env::current_exe().unwrap();
    let quoted_test = shell_quote_path(&current_test).unwrap();
    let quoted_runtime = shell_quote_path(&runtime).unwrap();
    let probe_dbus_address = format!("unix:path={}", runtime.join("bus").display());
    let quoted_probe_dbus_address = shell_quote_path(Path::new(&probe_dbus_address)).unwrap();
    let quoted_socket = shell_quote_path(&paths.socket).unwrap();
    let quoted_helper_log = shell_quote_path(&helper_log).unwrap();
    let quoted_managed_probe_started = shell_quote_path(&managed_probe_started).unwrap();
    let quoted_managed_probe_go = shell_quote_path(&managed_probe_go).unwrap();
    let quoted_managed_probe_result = shell_quote_path(&managed_probe_result).unwrap();
    let quoted_managed_probe_exit = shell_quote_path(&managed_probe_exit).unwrap();
    let quoted_managed_probe_stdout = shell_quote_path(&managed_probe_stdout).unwrap();
    let quoted_managed_probe_stderr = shell_quote_path(&managed_probe_stderr).unwrap();
    let quoted_whoami_fail = shell_quote_path(&whoami_fail).unwrap();
    let quoted_unit_name = shell_quote_path(Path::new(&unit_name)).unwrap();
    let fake_codex = fake_bin.join("codex");
    let fake_systemctl = fake_bin.join("systemctl");
    let candidate = fixture_root.path().join("candidate");
    let no_restart_candidate = fixture_root.path().join("no-restart-candidate");
    let restart_candidate = fixture_root.path().join("restart-candidate");
    let restart_bin = fixture_root.path().join("restart-bin");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&restart_bin)
        .unwrap();
    let restart_codex = restart_bin.join("codex");
    let path_environment = std::env::join_paths(
        std::iter::once(fake_bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let restart_path_environment = std::env::join_paths(
        [restart_bin.clone(), fake_bin.clone()]
            .into_iter()
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let quoted_fake_systemctl = shell_quote_path(&fake_systemctl).unwrap();
    let quoted_real_systemctl = shell_quote_path(&real_systemctl).unwrap();
    let quoted_path_environment = shell_quote_path(Path::new(&path_environment)).unwrap();
    let quoted_restart_path_environment =
        shell_quote_path(Path::new(&restart_path_environment)).unwrap();
    let quoted_no_restart_candidate = shell_quote_path(&no_restart_candidate).unwrap();
    let quoted_restart_candidate = shell_quote_path(&restart_candidate).unwrap();
    let quoted_descriptor = shell_quote_path(&desktop_descriptor).unwrap();
    let quoted_marketplace = shell_quote_path(&marketplace_state).unwrap();
    let quoted_plugin = shell_quote_path(&plugin_state).unwrap();
    let quoted_plugin_version = shell_quote_path(&plugin_version).unwrap();
    write_executable_fixture(
        &fake_codex,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf '{tested_codex_cli_version}\n'; exit 0; fi
if [ "$1" = "app-server" ]; then
  export {HELPER}=1
  export XDG_RUNTIME_DIR={quoted_runtime}
  export DBUS_SESSION_BUS_ADDRESS={quoted_probe_dbus_address}
  export {HELPER_SOCKET}={quoted_socket}
  export {HELPER_LOG}={quoted_helper_log}
  export {MANAGED_PROBE_STARTED}={quoted_managed_probe_started}
  export {MANAGED_PROBE_GO}={quoted_managed_probe_go}
  export {MANAGED_PROBE_RESULT}={quoted_managed_probe_result}
  export {MANAGED_PROBE_EXIT}={quoted_managed_probe_exit}
  export {MANAGED_PROBE_STDOUT}={quoted_managed_probe_stdout}
  export {MANAGED_PROBE_STDERR}={quoted_managed_probe_stderr}
  export {MANAGED_PROBE_UNIT}={quoted_unit_name}
  export {MANAGED_PROBE_SYSTEMCTL}={quoted_fake_systemctl}
  export {MANAGED_PROBE_REAL_SYSTEMCTL}={quoted_real_systemctl}
  export {MANAGED_PROBE_BASE_PATH}={quoted_path_environment}
  export {MANAGED_PROBE_RESTART_PATH}={quoted_restart_path_environment}
  export {MANAGED_PROBE_NO_RESTART_CANDIDATE}={quoted_no_restart_candidate}
  export {MANAGED_PROBE_RESTART_CANDIDATE}={quoted_restart_candidate}
  export {MANAGED_PROBE_WHOAMI_FAIL}={quoted_whoami_fail}
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
"#,
            tested_codex_cli_version = TESTED_CODEX_CLI_VERSION,
            quoted_runtime = quoted_runtime,
            quoted_probe_dbus_address = quoted_probe_dbus_address,
            quoted_managed_probe_started = quoted_managed_probe_started,
            quoted_managed_probe_go = quoted_managed_probe_go,
            quoted_managed_probe_result = quoted_managed_probe_result,
            quoted_managed_probe_exit = quoted_managed_probe_exit,
            quoted_managed_probe_stdout = quoted_managed_probe_stdout,
            quoted_managed_probe_stderr = quoted_managed_probe_stderr,
            quoted_whoami_fail = quoted_whoami_fail,
            quoted_unit_name = quoted_unit_name,
            quoted_fake_systemctl = quoted_fake_systemctl,
            quoted_real_systemctl = quoted_real_systemctl,
            quoted_path_environment = quoted_path_environment,
            quoted_restart_path_environment = quoted_restart_path_environment,
            quoted_no_restart_candidate = quoted_no_restart_candidate,
            quoted_restart_candidate = quoted_restart_candidate,
        ),
    );
    let fake_desktop = fixture_root.path().join("codex-desktop");
    write_executable_fixture(
        &fake_desktop,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    write_executable_fixture(
        &fake_systemctl,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
if [ -f {quoted_whoami_fail} ] &&
   [ "$#" -eq 2 ] && [ "$1" = "--user" ] && [ "$2" = "whoami" ]; then
  exit 1
fi
if [ "$1" = "--user" ] && [ "$2" = "enable" ] && [ "$3" = "--now" ] && [ ! -f {quoted_descriptor} ]; then
  exit 69
fi
if [ "$1" = "--user" ] && [ "$2" = "disable" ] && [ "$3" = "--now" ] && [ ! -f {quoted_descriptor} ]; then
  exit 70
fi
{real_systemctl} "$@"
status=$?
if [ "$status" -eq 0 ] && [ "$1" = "--user" ] && [ "$2" = "disable" ] && [ "$3" = "--now" ]; then
  [ ! -S {quoted_socket} ] || exit 71
  [ -f {quoted_descriptor} ] || exit 72
fi
exit "$status"
"#,
            systemctl_log.display(),
            quoted_whoami_fail = quoted_whoami_fail,
            real_systemctl = shell_quote_path(&real_systemctl).unwrap(),
        ),
    );
    write_executable_fixture(
        &candidate,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            env!("CARGO_PKG_VERSION"),
            test_target()
        ),
    );
    write_executable_fixture(
        &no_restart_candidate,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            higher_test_release_version(),
            product_target()
        ),
    );
    write_executable_fixture(
        &restart_candidate,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            expected_restart_candidate_version(),
            product_target()
        ),
    );
    fs::copy(&fake_codex, &restart_codex).unwrap();
    fs::set_permissions(&restart_codex, fs::Permissions::from_mode(0o755)).unwrap();
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
        (restart_bin.as_path(), FileKind::Directory),
        (fake_codex.as_path(), FileKind::RegularFile),
        (restart_codex.as_path(), FileKind::RegularFile),
        (
            setup_context.desktop_launcher.as_deref().unwrap(),
            FileKind::RegularFile,
        ),
        (fake_systemctl.as_path(), FileKind::RegularFile),
        (
            setup_context.candidate.executable.as_path(),
            FileKind::RegularFile,
        ),
        (no_restart_candidate.as_path(), FileKind::RegularFile),
        (restart_candidate.as_path(), FileKind::RegularFile),
    ] {
        assert_disposable_systemd_fixture_path(path, expected, euid);
    }
    for directory in [
        paths.codex_home.clone(),
        paths.codex_home.join("tasks"),
        paths.codex_home.join("rollouts"),
    ] {
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let metadata = fs::symlink_metadata(&directory).unwrap();
        assert_eq!(metadata.uid(), euid, "{}", directory.display());
        assert_eq!(metadata.mode() & 0o777, 0o700, "{}", directory.display());
    }
    for (path, bytes, mode) in [
        (
            paths.codex_home.join("auth.json"),
            b"auth sentinel".as_slice(),
            0o600,
        ),
        (
            paths.codex_home.join("tasks/task-sentinel"),
            b"task sentinel".as_slice(),
            0o600,
        ),
        (
            paths.codex_home.join("rollouts/rollout-sentinel"),
            b"rollout sentinel".as_slice(),
            0o600,
        ),
    ] {
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    setup_with_context(setup_context.clone()).await.unwrap();
    wait_until(|| paths.socket.exists() && main_pid(&real_systemctl, &unit_name) != 0).await;
    assert!(desktop_descriptor.is_file());
    let receipt_before_probe = fs::read(&paths.manifest).unwrap();
    let descriptor_before_probe = fs::read(&desktop_descriptor).unwrap();
    let auth_before_probe = fs::read(paths.codex_home.join("auth.json")).unwrap();
    let task_before_probe = fs::read(paths.codex_home.join("tasks/task-sentinel")).unwrap();
    let rollout_before_probe =
        fs::read(paths.codex_home.join("rollouts/rollout-sentinel")).unwrap();
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
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&paths.manifest).unwrap()).unwrap()["schemaVersion"],
        3
    );
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    fs::write(&managed_probe_go, b"go\n").unwrap();
    if tokio::time::timeout(Duration::from_secs(15), async {
        while !managed_probe_result.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_err()
    {
        panic!(
            "managed lifecycle probe did not publish result\nprobe exit:\n{}\nprobe stdout:\n{}\nprobe stderr:\n{}",
            bounded_probe_diagnostic(&managed_probe_exit),
            bounded_probe_diagnostic(&managed_probe_stdout),
            bounded_probe_diagnostic(&managed_probe_stderr),
        );
    }
    let result: ManagedProbeResult =
        serde_json::from_slice(&fs::read(&managed_probe_result).unwrap()).unwrap();

    let display_command = display_command_for_paths(&paths, &path_environment);
    assert!(result.no_restart_stdout.contains("Installed release:"));
    assert!(
        result
            .no_restart_stderr
            .find("completed: service-verify")
            .unwrap()
            < result
                .no_restart_stderr
                .find("completed: manifest")
                .unwrap()
    );
    assert_eq!(result.whoami_evidence, "SelfHosted(WhoAmI)");
    assert_eq!(result.fallback_evidence, "SelfHosted(ControlGroup)");
    for error in [
        &result.restart_error,
        &result.disable_error,
        &result.uninstall_error,
    ] {
        assert!(
            error.contains("running inside the managed app-server"),
            "{error}"
        );
        assert!(
            error.contains("run from an independent terminal"),
            "{error}"
        );
    }
    assert!(
        result
            .restart_error
            .contains(&format!("{display_command} update"))
    );
    assert!(
        result
            .disable_error
            .contains(&format!("{display_command} disable"))
    );
    assert!(
        result
            .uninstall_error
            .contains(&format!("{display_command} uninstall"))
    );
    assert!(result.unavailable_update_error.contains("systemd"));
    assert!(result.unavailable_update_error.contains("repair"));
    assert!(
        !result
            .unavailable_update_error
            .contains("systemctl --user stop")
    );
    assert!(!result.unavailable_update_error.contains(" disable\n"));
    assert!(result.unavailable_disable_error.contains(&format!(
        "systemctl --user stop {unit_name}\n{display_command} disable"
    )));
    assert!(result.unavailable_uninstall_error.contains(&format!(
        "systemctl --user stop {unit_name}\n{display_command} uninstall"
    )));
    assert_eq!(main_pid(&real_systemctl, &unit_name), first_pid);
    assert!(Path::new(&format!("/proc/{first_pid}")).exists());
    assert_eq!(
        fs::symlink_metadata(&paths.socket).unwrap().ino(),
        first_socket_inode
    );
    assert_ne!(fs::read(&paths.manifest).unwrap(), receipt_before_probe);
    assert_eq!(
        fs::read(&desktop_descriptor).unwrap(),
        descriptor_before_probe
    );
    assert_eq!(
        fs::read(paths.codex_home.join("auth.json")).unwrap(),
        auth_before_probe
    );
    assert_eq!(
        fs::read(paths.codex_home.join("tasks/task-sentinel")).unwrap(),
        task_before_probe
    );
    assert_eq!(
        fs::read(paths.codex_home.join("rollouts/rollout-sentinel")).unwrap(),
        rollout_before_probe
    );
    let log_before_independent = fs::read_to_string(&systemctl_log).unwrap();
    assert!(!log_before_independent.contains(" restart "));
    assert!(!log_before_independent.contains(" disable --now "));
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    assert_eq!(
        inspect_caller_unit(&fake_systemctl, &target),
        CallerUnitInspection::Independent
    );
    let lifecycle = || LifecycleContext {
        target: target.clone(),
        path_environment: path_environment.clone(),
        desktop_environment: desktop_environment.clone(),
        cwd: home.clone(),
    };
    let restart_lifecycle = LifecycleContext {
        target: target.clone(),
        path_environment: restart_path_environment.clone(),
        desktop_environment: desktop_environment.clone(),
        cwd: home.clone(),
    };
    let restart_log_before = fs::read_to_string(&systemctl_log).unwrap();
    let restart_receipt = staged_update_with_context(UpdateContext {
        lifecycle: restart_lifecycle,
        candidate: restart_candidate.clone(),
        terminal: TerminalState::noninteractive(),
    })
    .await
    .unwrap();
    let restart_log_after = fs::read_to_string(&systemctl_log).unwrap();
    assert_eq!(
        restart_log_after
            .matches(&format!("--user restart {unit_name}"))
            .count(),
        restart_log_before
            .matches(&format!("--user restart {unit_name}"))
            .count()
            + 1
    );
    wait_until(|| {
        let replacement = main_pid(&real_systemctl, &unit_name);
        replacement != 0
            && replacement != first_pid
            && paths.socket.exists()
            && fs::symlink_metadata(&paths.socket).unwrap().ino() != first_socket_inode
    })
    .await;
    assert!(
        restart_receipt
            .stderr
            .find("completed: service-verify")
            .unwrap()
            < restart_receipt.stderr.find("completed: manifest").unwrap()
    );
    let manifest: Value = serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap();
    assert_eq!(manifest["schemaVersion"], 3);
    assert!(manifest.get("codexVersion").is_none());
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    let disable_log_before = fs::read_to_string(&systemctl_log).unwrap();
    disable_with_context(lifecycle()).await.unwrap();
    let disable_log = fs::read_to_string(&systemctl_log).unwrap();
    let disable_segment = &disable_log[disable_log_before.len()..];
    assert!(disable_segment.contains(&format!(
        "--user is-active {unit_name}\n--user whoami\n--user disable --now {unit_name}"
    )));
    assert!(!paths.socket.exists());
    assert!(!desktop_descriptor.exists());
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    enable_with_context(lifecycle()).await.unwrap();
    wait_until(|| paths.socket.exists() && main_pid(&real_systemctl, &unit_name) != 0).await;
    assert!(desktop_descriptor.is_file());
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );
    let uninstall_log_before = fs::read_to_string(&systemctl_log).unwrap();
    uninstall_with_context(lifecycle()).await.unwrap();
    let uninstall_log = fs::read_to_string(&systemctl_log).unwrap();
    let uninstall_segment = &uninstall_log[uninstall_log_before.len()..];
    assert!(uninstall_segment.contains(&format!(
        "--user is-active {unit_name}\n--user whoami\n--user disable --now {unit_name}"
    )));
    assert!(!paths.unit.exists());
    assert!(!paths.socket.exists());
    assert!(!desktop_descriptor.exists());
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    struct RemoveOnDrop(PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _remove_whoami_fail = RemoveOnDrop(whoami_fail.clone());
    setup_with_context(setup_context).await.unwrap();
    wait_until(|| paths.socket.exists() && main_pid(&real_systemctl, &unit_name) != 0).await;
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );
    fs::write(&whoami_fail, b"fail whoami\n").unwrap();
    let recovery_disable_log_before = fs::read_to_string(&systemctl_log).unwrap();
    run_systemctl(&fake_systemctl, ["--user", "stop", unit_name.as_str()]).unwrap();
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );
    disable_with_context(lifecycle()).await.unwrap();
    let recovery_disable_log = fs::read_to_string(&systemctl_log).unwrap();
    let recovery_disable_segment = &recovery_disable_log[recovery_disable_log_before.len()..];
    assert!(!recovery_disable_segment.contains("--user whoami"));
    assert!(!paths.socket.exists());
    assert!(!desktop_descriptor.exists());
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    enable_with_context(lifecycle()).await.unwrap();
    wait_until(|| paths.socket.exists() && main_pid(&real_systemctl, &unit_name) != 0).await;
    assert!(desktop_descriptor.is_file());
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );
    let recovery_uninstall_log_before = fs::read_to_string(&systemctl_log).unwrap();
    run_systemctl(&fake_systemctl, ["--user", "stop", unit_name.as_str()]).unwrap();
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );
    uninstall_with_context(lifecycle()).await.unwrap();
    let recovery_uninstall_log = fs::read_to_string(&systemctl_log).unwrap();
    let recovery_uninstall_segment = &recovery_uninstall_log[recovery_uninstall_log_before.len()..];
    assert!(!recovery_uninstall_segment.contains("--user whoami"));
    assert!(!paths.socket.exists());
    assert!(!desktop_descriptor.exists());
    assert_eq!(
        fs::read(paths.codex_home.join("auth.json")).unwrap(),
        auth_before_probe
    );
    assert_eq!(
        fs::read(paths.codex_home.join("tasks/task-sentinel")).unwrap(),
        task_before_probe
    );
    assert_eq!(
        fs::read(paths.codex_home.join("rollouts/rollout-sentinel")).unwrap(),
        rollout_before_probe
    );
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );

    let log = fs::read_to_string(&systemctl_log).unwrap();
    for line in log.lines() {
        if line == "--user daemon-reload" || line == "--user whoami" {
            continue;
        }
        assert!(line.contains(&unit_name), "{line}");
        assert!(!line.contains("codex-session-control.service"), "{line}");
    }
    assert_production_state(
        &real_systemctl,
        &production_socket,
        &production_unit_before,
        &production_socket_before,
    );
}
