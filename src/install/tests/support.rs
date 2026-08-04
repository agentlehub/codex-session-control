use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use super::*;

pub(super) struct Fixture {
    pub(super) _root: tempfile::TempDir,
    pub(super) paths: ResolvedUserPaths,
    pub(super) fake_bin: PathBuf,
    pub(super) candidate: PathBuf,
    codex_log: PathBuf,
    systemctl_log: PathBuf,
    pub(super) codex_version: PathBuf,
    pub(super) marketplace_state: PathBuf,
    pub(super) plugin_state: PathBuf,
    pub(super) plugin_version_state: PathBuf,
    pub(super) codex_fail: PathBuf,
    pub(super) systemctl_fail: PathBuf,
    pub(super) preserve_service_state: PathBuf,
    pub(super) enabled: PathBuf,
    pub(super) active: PathBuf,
    pub(super) wait_for_socket: PathBuf,
    pub(super) restart_requested: PathBuf,
    pub(super) required_descriptor: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let runtime = root.path().join("runtime");
        let fake_bin = root.path().join("fake-bin");
        let xdg_data_dirs = root.path().join("xdg-data-dirs");
        for directory in [&home, &runtime, &fake_bin, &xdg_data_dirs] {
            fs::create_dir(directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut paths =
            ResolvedUserPaths::for_test(rustix::process::geteuid().as_raw(), home, runtime);
        paths
            .unit
            .set_file_name("codex-session-control-test-Setup1.service");
        let candidate = root.path().join("candidate");
        super::write_executable_fixture(
            &candidate,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
                env!("CARGO_PKG_VERSION"),
                test_target()
            ),
        );
        let codex_log = root.path().join("codex.log");
        let systemctl_log = root.path().join("systemctl.log");
        let codex_version = root.path().join("codex-version");
        let marketplace_state = root.path().join("marketplace-state");
        let plugin_state = root.path().join("plugin-state");
        let plugin_version_state = root.path().join("plugin-version-state");
        let codex_fail = root.path().join("codex-fail");
        let systemctl_fail = root.path().join("systemctl-fail");
        let preserve_service_state = root.path().join("preserve-service-state");
        let enabled = root.path().join("enabled");
        let active = root.path().join("active");
        let wait_for_socket = root.path().join("wait-for-socket");
        let restart_requested = root.path().join("restart-requested");
        let required_descriptor = root.path().join("required-descriptor");
        fs::write(&codex_version, b"codex-cli 0.146.0\n").unwrap();

        let codex = fake_bin.join("codex");
        let codex_script = format!(
            r#"#!/bin/sh
printf '%s|CODEX_HOME=%s
' "$*" "$CODEX_HOME" >> '{codex_log}'
if [ -f '{codex_fail}' ] && [ "$(cat '{codex_fail}')" = "$*" ]; then
  exit 1
fi
if [ "$1" = "--version" ]; then
  cat '{codex_version}'
  exit 0
fi
if [ "$1" = "plugin" ] && [ ! -d "$CODEX_HOME" ]; then
  exit 1
fi
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ] && [ "$3" = "list" ]; then
  if [ -f '{marketplace_state}' ]; then
    root=$(cat '{marketplace_state}')
    printf '{{"marketplaces":[{{"name":"codex-session-control-local","root":"%s","marketplaceSource":{{"sourceType":"local","source":"%s"}}}}]}}\n' "$root" "$root"
  else
    printf '{{"marketplaces":[]}}\n'
  fi
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ] && [ "$3" = "remove" ]; then
  rm -f '{marketplace_state}'
  printf '{{}}\n'
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ] && [ "$3" = "add" ]; then
  printf '%s' "$4" > '{marketplace_state}'
  printf '{{"marketplaceName":"codex-session-control-local","installedRoot":"%s","alreadyAdded":false}}\n' "$4"
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "list" ]; then
  if [ -f '{plugin_state}' ]; then
    root=$(cat '{plugin_state}')
    if [ -f '{plugin_version_state}' ]; then version=$(cat '{plugin_version_state}'); else version='{version}'; fi
    printf '{{"installed":[{{"pluginId":"codex-session-control@codex-session-control-local","name":"codex-session-control","marketplaceName":"codex-session-control-local","version":"%s","installed":true,"enabled":true,"source":{{"source":"local","path":"%s/plugins/codex-session-control"}},"marketplaceSource":{{"sourceType":"local","source":"%s"}},"installPolicy":"AVAILABLE"}}],"available":[]}}\n' "$version" "$root" "$root"
  else
    printf '{{"installed":[],"available":[]}}\n'
  fi
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "remove" ]; then
  rm -f '{plugin_state}' '{plugin_version_state}'
  printf '{{}}\n'
  exit 0
fi
if [ "$1" = "plugin" ] && [ "$2" = "add" ]; then
  root=$(cat '{marketplace_state}')
  printf '%s' "$root" > '{plugin_state}'
  sed -n 's/.*"version": "\([^"]*\)".*/\1/p' "$root/plugins/codex-session-control/.codex-plugin/plugin.json" > '{plugin_version_state}'
  version=$(cat '{plugin_version_state}')
  printf '{{"pluginId":"codex-session-control@codex-session-control-local","name":"codex-session-control","marketplaceName":"codex-session-control-local","version":"%s","installedPath":"%s/cache"}}\n' "$version" "$CODEX_HOME"
  exit 0
fi
exit 64
"#,
            codex_log = codex_log.display(),
            codex_fail = codex_fail.display(),
            codex_version = codex_version.display(),
            marketplace_state = marketplace_state.display(),
            plugin_state = plugin_state.display(),
            plugin_version_state = plugin_version_state.display(),
            version = env!("CARGO_PKG_VERSION"),
        );
        super::write_executable_fixture(&codex, codex_script);

        let systemctl = fake_bin.join("systemctl");
        let systemctl_script = format!(
            r#"#!/bin/sh
printf '%s
' "$*" >> '{systemctl_log}'
if [ -f '{systemctl_fail}' ] && [ "$(cat '{systemctl_fail}')" = "$*" ]; then
  exit 1
fi
if [ "$1" = "--user" ] && [ "$2" = "daemon-reload" ]; then
  exit 0
fi
if [ "$1" = "--user" ] && [ "$2" = "enable" ] && [ "$3" = "--now" ]; then
  if [ -f '{required_descriptor}' ] && [ ! -f "$(cat '{required_descriptor}')" ]; then
    exit 69
  fi
  printf enabled > '{enabled}'
  printf active > '{active}'
  if [ -f '{wait_for_socket}' ]; then
    count=0
    while [ ! -S '{socket}' ]; do
      count=$((count + 1))
      [ "$count" -lt 1000000 ] || exit 70
    done
  fi
  exit 0
fi
if [ "$1" = "--user" ] && [ "$2" = "disable" ] && [ "$3" = "--now" ]; then
  if [ ! -f '{unit}' ]; then
    exit 5
  fi
  if [ ! -f '{preserve_service_state}' ]; then
    rm -f '{enabled}' '{active}' '{socket}'
  fi
  exit 0
fi
if [ "$1" = "--user" ] && [ "$2" = "restart" ]; then
  rm -f '{socket}'
  printf restart > '{restart_requested}'
  if [ -f '{wait_for_socket}' ]; then
    count=0
    while [ ! -S '{socket}' ]; do
      count=$((count + 1))
      [ "$count" -lt 1000000 ] || exit 70
    done
  fi
  exit 0
fi
if [ "$1" = "--user" ] && [ "$2" = "is-enabled" ]; then
  if [ ! -f '{unit}' ]; then
    [ "$3" = "--quiet" ] || printf 'not-found\n'
    exit 4
  fi
  if [ -f '{enabled}' ]; then
    [ "$3" = "--quiet" ] || printf 'enabled\n'
    exit 0
  fi
  [ "$3" = "--quiet" ] || printf 'disabled\n'
  exit 1
fi
if [ "$1" = "--user" ] && [ "$2" = "is-active" ]; then
  if [ -f '{active}' ]; then
    [ "$3" = "--quiet" ] || printf 'active\n'
    exit 0
  fi
  [ "$3" = "--quiet" ] || printf 'inactive\n'
  exit 3
fi
exit 64
"#,
            systemctl_log = systemctl_log.display(),
            systemctl_fail = systemctl_fail.display(),
            preserve_service_state = preserve_service_state.display(),
            unit = paths.unit.display(),
            enabled = enabled.display(),
            active = active.display(),
            wait_for_socket = wait_for_socket.display(),
            restart_requested = restart_requested.display(),
            required_descriptor = required_descriptor.display(),
            socket = paths.socket.display(),
        );
        super::write_executable_fixture(&systemctl, systemctl_script);

        fs::create_dir_all(&paths.codex_home).unwrap();
        fs::create_dir_all(&paths.data_root).unwrap();
        fs::set_permissions(&paths.data_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&paths.codex_home, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _root: root,
            paths,
            fake_bin,
            candidate,
            codex_log,
            systemctl_log,
            codex_version,
            marketplace_state,
            plugin_state,
            plugin_version_state,
            codex_fail,
            systemctl_fail,
            preserve_service_state,
            enabled,
            active,
            wait_for_socket,
            restart_requested,
            required_descriptor,
        }
    }

    pub(super) fn context(&self, include_install_bin: bool) -> SetupContext {
        let mut path = vec![self.fake_bin.clone()];
        if include_install_bin {
            path.push(self.paths.home.join(".local/bin"));
        }
        SetupContext {
            target: LifecycleTarget::suffixed(self.paths.clone(), "Setup1"),
            candidate: CandidateRelease {
                executable: self.candidate.clone(),
                product_version: env!("CARGO_PKG_VERSION").to_owned(),
                target: test_target().to_owned(),
            },
            path_environment: std::env::join_paths(path).unwrap(),
            desktop_environment: BTreeMap::from([
                (
                    OsString::from("HOME"),
                    self.paths.home.as_os_str().to_owned(),
                ),
                (OsString::from("PATH"), self.fake_bin.as_os_str().to_owned()),
                (
                    OsString::from("XDG_DATA_HOME"),
                    self.paths.home.join(".local/share").into_os_string(),
                ),
                (
                    OsString::from("XDG_DATA_DIRS"),
                    self._root.path().join("xdg-data-dirs").into_os_string(),
                ),
            ]),
            desktop_launcher: None,
            cwd: self._root.path().to_path_buf(),
        }
    }

    pub(super) fn codex_log(&self) -> String {
        fs::read_to_string(&self.codex_log).unwrap_or_default()
    }

    pub(super) fn systemctl_log(&self) -> String {
        fs::read_to_string(&self.systemctl_log).unwrap_or_default()
    }

    pub(super) fn clear_logs(&self) {
        fs::write(&self.codex_log, b"").unwrap();
        fs::write(&self.systemctl_log, b"").unwrap();
    }
}

pub(super) struct FakeAuthority {
    task: tokio::task::JoinHandle<()>,
}

impl FakeAuthority {
    pub(super) async fn start(paths: &ResolvedUserPaths, version: &str) -> Self {
        Self::start_reporting(paths, version, paths.codex_home.clone()).await
    }

    pub(super) async fn start_reporting(
        paths: &ResolvedUserPaths,
        version: &str,
        reported_home: PathBuf,
    ) -> Self {
        create_product_dir(&paths.runtime_dir, paths.euid).unwrap();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600)).unwrap();
        let codex_home = reported_home;
        let version = Arc::<str>::from(version);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let codex_home = codex_home.clone();
                let version = Arc::clone(&version);
                tokio::spawn(async move {
                    let mut websocket = accept_async(stream).await.unwrap();
                    let Message::Text(initialize) = websocket.next().await.unwrap().unwrap() else {
                        panic!("expected initialize")
                    };
                    let initialize: Value = serde_json::from_str(initialize.as_str()).unwrap();
                    websocket
                        .send(Message::text(
                            json!({
                                "id": initialize["id"],
                                "result": {
                                    "codexHome": codex_home,
                                    "userAgent": format!("codex-cli {version}")
                                }
                            })
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
                        if request.get("method").and_then(Value::as_str) == Some("thread/list") {
                            websocket
                                .send(Message::text(
                                    json!({
                                        "id": request["id"],
                                        "result": {
                                            "data": [],
                                            "nextCursor": null
                                        }
                                    })
                                    .to_string(),
                                ))
                                .await
                                .unwrap();
                        } else {
                            panic!("unexpected fake authority request: {request}");
                        }
                    }
                });
            }
        });
        Self { task }
    }
}

impl Drop for FakeAuthority {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) fn assert_installed_modes(paths: &ResolvedUserPaths) {
    for (path, expected) in [
        (&paths.binary, 0o755),
        (&paths.config, 0o600),
        (&paths.unit, 0o644),
        (&paths.manifest, 0o600),
        (
            &paths.marketplace.join(".agents/plugins/marketplace.json"),
            0o644,
        ),
        (
            &paths
                .marketplace
                .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
            0o644,
        ),
        (
            &paths
                .marketplace
                .join("plugins/codex-session-control/.mcp.json"),
            0o644,
        ),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            expected,
            "{}",
            path.display()
        );
    }
}
