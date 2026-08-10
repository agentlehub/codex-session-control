use std::{
    collections::BTreeSet,
    error::Error,
    fs::{self, File},
    io,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use assert_cmd::cargo::cargo_bin;
use futures_util::SinkExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    net::UnixStream,
    process::{Child, Command},
};
use tokio_tungstenite::{client_async, tungstenite::Message};

use crate::normal_home_paths::{CONFIG_TEMPLATE, DisposablePaths, atomic_write};
use crate::protocol_support::{NativeConnection, ResponsesEndpoint};

pub(super) const EXPECTED_CODEX_VERSION: &str = concat!(
    "codex-cli ",
    env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION")
);
pub(super) const SESSION_CONTROL_TOOLS: [&str; 13] = [
    "thread_create",
    "thread_fork",
    "threads_list",
    "thread_read",
    "threads_wait",
    "thread_message_send",
    "thread_title_set",
    "thread_goal_get",
    "thread_goal_set",
    "thread_goal_pause",
    "thread_goal_resume",
    "thread_goal_clear",
    "thread_interrupt",
];
pub(super) const ALL_SOURCE_KINDS: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProcessIdentity {
    pub(super) pid: u32,
    pub(super) start_time: u64,
}

pub(super) struct LiveHarness {
    pub(super) root: tempfile::TempDir,
    pub(super) codex: PathBuf,
    pub(super) codex_version: String,
    pub(super) codex_home: PathBuf,
    pub(super) runtime: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) socket: PathBuf,
    pub(super) stderr_log: PathBuf,
    pub(super) process_home: PathBuf,
    pub(super) process_runtime: Option<PathBuf>,
    pub(super) disposable_paths: Option<DisposablePaths>,
    pub(super) endpoint: ResponsesEndpoint,
    pub(super) child: Option<Child>,
}
impl LiveHarness {
    pub(super) async fn start() -> Result<Self, Box<dyn Error>> {
        let codex = configured_codex()?;
        let root = crate::test_support::private_tempdir();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
        let codex_home = root.path().join(".codex");
        let runtime = root.path().join("runtime");
        let workspace = root.path().join("workspace");
        for directory in [&codex_home, &runtime, &workspace] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let endpoint = ResponsesEndpoint::start().await?;
        let config = CONFIG_TEMPLATE.replace("__PORT__", &endpoint.address.port().to_string());
        assert_eq!(
            config,
            format!(
                r#"model = "session-control-test"
model_provider = "session-control-local"

[model_providers.session-control-local]
name = "Session control local test"
base_url = "http://127.0.0.1:{}/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 10000

[analytics]
enabled = false
"#,
                endpoint.address.port()
            )
        );
        atomic_write(&codex_home.join("config.toml"), config.as_bytes(), 0o600)?;
        let socket = runtime.join("app-server.sock");
        let stderr_log = root.path().join("app-server.stderr");
        let codex_version = run_codex_in_home(&codex, &codex_home, root.path(), ["--version"])
            .await?
            .trim()
            .to_owned();
        if codex_version != EXPECTED_CODEX_VERSION {
            return Err(format!(
                "live compatibility requires {EXPECTED_CODEX_VERSION}, found {codex_version}"
            )
            .into());
        }
        let process_home = root.path().to_path_buf();
        let mut harness = Self {
            root,
            codex,
            codex_version,
            codex_home,
            runtime,
            workspace,
            socket,
            stderr_log,
            process_home,
            process_runtime: None,
            disposable_paths: None,
            endpoint,
            child: None,
        };
        harness.launch().await?;
        Ok(harness)
    }

    pub(super) async fn start_disposable_ci() -> Result<Self, Box<dyn Error>> {
        let mut harness = Self::start().await?;
        harness.stop().await?;
        let paths = DisposablePaths::claim(&harness.codex, harness.endpoint.address.port())?;
        harness.codex_home.clone_from(&paths.codex_home);
        harness.runtime.clone_from(&paths.runtime_dir);
        harness.socket.clone_from(&paths.socket);
        harness.process_home.clone_from(&paths.home);
        harness.process_runtime = Some(paths.runtime.clone());
        harness.disposable_paths = Some(paths);
        harness.launch().await?;
        Ok(harness)
    }

    pub(super) async fn prepare_disposable_product_ci() -> Result<Self, Box<dyn Error>> {
        let mut harness = Self::start().await?;
        harness.stop().await?;
        let paths =
            DisposablePaths::claim_for_product(&harness.codex, harness.endpoint.address.port())?;
        harness.codex_home.clone_from(&paths.codex_home);
        harness.runtime.clone_from(&paths.runtime_dir);
        harness.socket.clone_from(&paths.socket);
        harness.process_home.clone_from(&paths.home);
        harness.process_runtime = Some(paths.runtime.clone());
        harness.disposable_paths = Some(paths);

        let native_bin = harness.root.path().join("native-bin");
        fs::create_dir(&native_bin)?;
        fs::set_permissions(&native_bin, fs::Permissions::from_mode(0o700))?;
        let native_codex = native_bin.join("codex");
        fs::copy(&harness.codex, &native_codex)?;
        fs::set_permissions(&native_codex, fs::Permissions::from_mode(0o700))?;
        Ok(harness)
    }

    pub(super) async fn launch(&mut self) -> Result<(), Box<dyn Error>> {
        assert!(self.child.is_none());
        assert!(self.runtime.is_dir());
        let stderr = File::create(&self.stderr_log)?;
        let mut command = Command::new(&self.codex);
        scrub_command(
            &mut command,
            &self.codex_home,
            &self.process_home,
            self.process_runtime.as_deref(),
        );
        command
            .args(["app-server", "--listen"])
            .arg(format!("unix://{}", self.socket.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        // SAFETY: the child-only hook invokes only the async-signal-safe umask syscall.
        unsafe {
            command.as_std_mut().pre_exec(|| {
                rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
                Ok(())
            });
        }
        self.child = Some(command.spawn()?);
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if self.socket.exists() {
                    let mode = fs::symlink_metadata(&self.socket)?.permissions().mode() & 0o777;
                    if !matches!(mode, 0o600 | 0o700) {
                        return Err(io::Error::other(format!(
                            "live socket mode {mode:04o} is not owner-only read/write"
                        )));
                    }
                    return Ok(());
                }
                if let Some(status) = self.child.as_mut().unwrap().try_wait()? {
                    return Err(io::Error::other(format!(
                        "app-server exited before readiness: {status}: {}",
                        fs::read_to_string(&self.stderr_log).unwrap_or_default()
                    )));
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "app-server socket readiness timed out")??;
        Ok(())
    }

    pub(super) async fn connect(&self) -> Result<NativeConnection, Box<dyn Error>> {
        self.connect_named(
            "codex_session_control_live_test",
            "Codex Session Control Live Test",
        )
        .await
    }

    pub(super) async fn connect_named(
        &self,
        name: &str,
        title: &str,
    ) -> Result<NativeConnection, Box<dyn Error>> {
        let stream = UnixStream::connect(&self.socket).await?;
        let (websocket, _) = client_async("ws://localhost/", stream).await?;
        let mut native = NativeConnection {
            websocket,
            next_id: 1,
        };
        let initialized = native
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": name,
                        "title": title,
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "experimentalApi": true,
                        "mcpServerOpenaiFormElicitation": false,
                        "requestAttestation": false,
                        "optOutNotificationMethods": []
                    }
                }),
            )
            .await?;
        assert_eq!(
            Path::new(initialized["codexHome"].as_str().unwrap()),
            self.codex_home
        );
        assert!(
            initialized["userAgent"]
                .as_str()
                .unwrap()
                .contains(env!("CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION"))
        );
        native
            .websocket
            .send(Message::text(json!({"method": "initialized"}).to_string()))
            .await?;
        Ok(native)
    }

    pub(super) async fn start_thread(
        &self,
        native: &mut NativeConnection,
    ) -> Result<String, Box<dyn Error>> {
        let response = native
            .request(
                "thread/start",
                json!({
                    "model": "session-control-test",
                    "modelProvider": "session-control-local",
                    "cwd": self.workspace,
                    "approvalPolicy": "never",
                    "sandbox": "read-only"
                }),
            )
            .await?;
        response["thread"]["id"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| "thread/start omitted thread id".into())
    }

    pub(super) async fn wait_for_thread_list(
        &self,
        native: &mut NativeConnection,
        thread_id: &str,
    ) -> Result<Value, Box<dyn Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let listed = native
                    .request(
                        "thread/list",
                        json!({"limit": 10, "sourceKinds": ALL_SOURCE_KINDS}),
                    )
                    .await?;
                if listed["data"].as_array().is_some_and(|threads| {
                    threads
                        .iter()
                        .any(|thread| thread["id"].as_str() == Some(thread_id))
                }) {
                    return Ok::<Value, Box<dyn Error>>(listed);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| format!("thread/list did not converge for {thread_id}"))?
    }

    pub(super) async fn start_turn(
        &self,
        native: &mut NativeConnection,
        thread_id: &str,
        prompt: &str,
    ) -> Result<String, Box<dyn Error>> {
        let response = native
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "input": [{"type": "text", "text": prompt}]
                }),
            )
            .await?;
        response["turn"]["id"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| "turn/start omitted turn id".into())
    }

    pub(super) async fn wait_for_turn_status(
        &self,
        native: &mut NativeConnection,
        thread_id: &str,
        turn_id: &str,
        expected: &str,
    ) -> Result<(), Box<dyn Error>> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let turns = native
                    .request(
                        "thread/turns/list",
                        json!({
                            "threadId": thread_id,
                            "limit": 10,
                            "itemsView": "notLoaded",
                            "sortDirection": "desc"
                        }),
                    )
                    .await?;
                if turns["data"].as_array().is_some_and(|turns| {
                    turns.iter().any(|turn| {
                        turn["id"].as_str() == Some(turn_id)
                            && turn["status"].as_str() == Some(expected)
                    })
                }) {
                    return Ok::<(), Box<dyn Error>>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| format!("turn {turn_id} did not reach {expected}"))?
    }

    pub(super) async fn restart(&mut self) -> Result<(), Box<dyn Error>> {
        self.stop().await?;
        self.launch().await
    }

    pub(super) async fn stop(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(mut child) = self.child.take() {
            let pid = child.id().ok_or("app-server pid is unavailable")?;
            signal_process_group(pid, rustix::process::Signal::TERM)?;
            match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
                Ok(wait_result) => {
                    wait_result?;
                }
                Err(_) => {
                    signal_process_group(pid, rustix::process::Signal::KILL)?;
                    child.wait().await?;
                }
            }
        }
        if self.socket.exists() {
            if UnixStream::connect(&self.socket).await.is_ok() {
                return Err("app-server socket still accepts connections after exit".into());
            }
            fs::remove_file(&self.socket)?;
        }
        Ok(())
    }

    pub(super) async fn stop_and_clean_disposable(&mut self) -> Result<(), Box<dyn Error>> {
        self.stop().await?;
        let paths = self
            .disposable_paths
            .take()
            .ok_or("disposable path ownership was unavailable")?;
        let absent = paths.absent_paths().map(Path::to_path_buf);
        drop(paths);
        for path in absent {
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    return Err(
                        format!("disposable path survived cleanup: {}", path.display()).into(),
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub(super) fn identity(&self) -> Result<ProcessIdentity, Box<dyn Error>> {
        let pid = self
            .child
            .as_ref()
            .ok_or("app-server is stopped")?
            .id()
            .ok_or("app-server pid is unavailable")?;
        process_identity(pid)
    }

    pub(super) fn endpoint(&self) -> &ResponsesEndpoint {
        &self.endpoint
    }

    pub(super) fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub(super) fn codex_version(&self) -> &str {
        &self.codex_version
    }

    pub(super) async fn schema_digest(&self) -> Result<String, Box<dyn Error>> {
        let output = self.root.path().join("schema");
        fs::create_dir(&output)?;
        fs::set_permissions(&output, fs::Permissions::from_mode(0o700))?;
        let result = run_codex_in_home_output(
            &self.codex,
            &self.codex_home,
            self.root.path(),
            [
                "app-server",
                "generate-json-schema",
                "--experimental",
                "--out",
                output.to_str().unwrap(),
            ],
        )
        .await?;
        if !result.status.success() {
            return Err(format!(
                "schema generation failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )
            .into());
        }
        aggregate_schema_digest(&output)
    }

    pub(super) fn codex_home_contains_session(&self) -> Result<bool, Box<dyn Error>> {
        fn contains_file(path: &Path) -> io::Result<bool> {
            if !path.exists() {
                return Ok(false);
            }
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_file()
                    || (entry.file_type()?.is_dir() && contains_file(&entry.path())?)
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        contains_file(&self.codex_home.join("sessions")).map_err(Into::into)
    }

    pub(super) fn disposable_paths(&self) -> Result<&DisposablePaths, Box<dyn Error>> {
        self.disposable_paths
            .as_ref()
            .ok_or_else(|| "disposable path ownership was unavailable".into())
    }
}

impl LiveHarness {
    pub(super) async fn spawn_remote(&self, prompt: &str) -> Result<RemoteClient, Box<dyn Error>> {
        self.spawn_pty(
            [
                shell_quote(&self.codex),
                "--remote".to_owned(),
                shell_quote_text(&format!("unix://{}", self.socket.display())),
                "--no-alt-screen".to_owned(),
                "-C".to_owned(),
                shell_quote(&self.workspace),
                "-a".to_owned(),
                "never".to_owned(),
                "-s".to_owned(),
                "read-only".to_owned(),
                shell_quote_text(prompt),
            ]
            .join(" "),
        )
        .await
    }

    pub(super) async fn spawn_wrapper_resume(
        &self,
        controller: &Path,
        thread: &str,
        prompt: &str,
    ) -> Result<RemoteClient, Box<dyn Error>> {
        self.spawn_pty(
            [
                shell_quote(controller),
                "codex".to_owned(),
                "resume".to_owned(),
                "--no-alt-screen".to_owned(),
                "-C".to_owned(),
                shell_quote(&self.workspace),
                "-a".to_owned(),
                "never".to_owned(),
                "-s".to_owned(),
                "read-only".to_owned(),
                shell_quote_text(thread),
                shell_quote_text(prompt),
            ]
            .join(" "),
        )
        .await
    }

    pub(super) async fn spawn_pty(
        &self,
        command_line: String,
    ) -> Result<RemoteClient, Box<dyn Error>> {
        let script = ["/usr/bin/script", "/bin/script"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .ok_or("script(1) is required for remote CLI PTY coverage")?;
        let mut command = Command::new(script);
        scrub_command(
            &mut command,
            &self.codex_home,
            &self.process_home,
            self.process_runtime.as_deref(),
        );
        command
            .args(["-qefc", &command_line, "/dev/null"])
            .env("TERM", "xterm-256color")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        let child = command.spawn()?;
        let pid = child.id().ok_or("remote CLI pid unavailable")?;
        Ok(RemoteClient {
            child: Some(child),
            process_group: pid,
        })
    }

    pub(super) async fn install_projection(&self) -> Result<(), Box<dyn Error>> {
        let marketplace = self.root.path().join("marketplace");
        let plugin = marketplace.join("plugins/codex-session-control");
        fs::create_dir_all(marketplace.join(".agents/plugins"))?;
        fs::create_dir_all(&plugin)?;
        for directory in [
            &marketplace,
            &marketplace.join(".agents"),
            &marketplace.join(".agents/plugins"),
            &marketplace.join("plugins"),
            &plugin,
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        atomic_write(
            &marketplace.join(".agents/plugins/marketplace.json"),
            include_bytes!("../../assets/marketplace/.agents/plugins/marketplace.json"),
            0o644,
        )?;
        let plugin_manifest = include_str!(
            "../../assets/marketplace/plugins/codex-session-control/.codex-plugin/plugin.json"
        )
        .replace("__PRODUCT_VERSION__", env!("CARGO_PKG_VERSION"));
        fs::create_dir_all(plugin.join(".codex-plugin"))?;
        fs::set_permissions(
            plugin.join(".codex-plugin"),
            fs::Permissions::from_mode(0o700),
        )?;
        atomic_write(
            &plugin.join(".codex-plugin/plugin.json"),
            plugin_manifest.as_bytes(),
            0o644,
        )?;
        let mcp = include_str!("../../assets/marketplace/plugins/codex-session-control/.mcp.json")
            .replace(
                "__INSTALLED_EXECUTABLE__",
                configured_controller()?.to_str().unwrap(),
            );
        atomic_write(&plugin.join(".mcp.json"), mcp.as_bytes(), 0o644)?;
        for args in [
            vec![
                "plugin".to_owned(),
                "marketplace".to_owned(),
                "add".to_owned(),
                marketplace.display().to_string(),
                "--json".to_owned(),
            ],
            vec![
                "plugin".to_owned(),
                "add".to_owned(),
                "codex-session-control@codex-session-control-local".to_owned(),
                "--json".to_owned(),
            ],
        ] {
            let output = self
                .run_codex_output(args.iter().map(String::as_str))
                .await?;
            if !output.status.success() {
                return Err(format!(
                    "projection registration failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
        }
        let installed = self.run_codex_output(["plugin", "list", "--json"]).await?;
        if !installed.status.success() {
            return Err(format!(
                "projection verification failed: {}",
                String::from_utf8_lossy(&installed.stderr)
            )
            .into());
        }
        let installed: Value = serde_json::from_slice(&installed.stdout)?;
        if !installed["installed"].as_array().is_some_and(|plugins| {
            plugins.iter().any(|plugin| {
                plugin["pluginId"].as_str()
                    == Some("codex-session-control@codex-session-control-local")
                    && plugin["installed"].as_bool() == Some(true)
                    && plugin["enabled"].as_bool() == Some(true)
            })
        }) {
            return Err(format!("native plugin state did not converge: {installed}").into());
        }
        Ok(())
    }

    pub(super) async fn run_codex_output(
        &self,
        args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
    ) -> Result<std::process::Output, Box<dyn Error>> {
        let mut command = Command::new(&self.codex);
        scrub_command(
            &mut command,
            &self.codex_home,
            &self.process_home,
            self.process_runtime.as_deref(),
        );
        Ok(command.args(args).output().await?)
    }

    pub(super) fn assert_fixture_clean(&self) -> Result<(), Box<dyn Error>> {
        self.assert_fixture_clean_with_projection(false)
    }

    pub(super) fn assert_projected_fixture_clean(&self) -> Result<(), Box<dyn Error>> {
        self.assert_fixture_clean_with_projection(true)
    }

    pub(super) fn assert_fixture_clean_with_projection(
        &self,
        projection_installed: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.endpoint.assert_clean()?;
        let config = fs::read_to_string(self.codex_home.join("config.toml"))?;
        let expected =
            CONFIG_TEMPLATE.replace("__PORT__", &self.endpoint.address.port().to_string());
        if projection_installed {
            let mut parsed: toml::Value = toml::from_str(&config)?;
            let root = parsed
                .as_table_mut()
                .ok_or("configured Codex configuration is not a table")?;
            let marketplaces = root
                .remove("marketplaces")
                .ok_or("native marketplace state is missing")?;
            let plugins = root
                .remove("plugins")
                .ok_or("native plugin state is missing")?;
            if parsed != toml::from_str::<toml::Value>(&expected)?
                || marketplaces["codex-session-control-local"]["source_type"].as_str()
                    != Some("local")
                || marketplaces["codex-session-control-local"]["source"].as_str()
                    != Some(self.root.path().join("marketplace").to_str().unwrap())
                || marketplaces["codex-session-control-local"]["last_updated"]
                    .as_str()
                    .is_none()
                || plugins["codex-session-control@codex-session-control-local"]["enabled"].as_bool()
                    != Some(true)
            {
                return Err("configured Codex plugin configuration drifted".into());
            }
        } else if config != expected {
            return Err("configured Codex configuration drifted".into());
        }
        for forbidden in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "FIXTURE_CREDENTIAL_SENTINEL",
        ] {
            if self
                .endpoint
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.to_string().contains(forbidden))
            {
                return Err(format!("loopback request exposed {forbidden}").into());
            }
        }
        Ok(())
    }
}
impl Drop for LiveHarness {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if let Some(pid) = child.id() {
                let _ = signal_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.start_kill();
        }
    }
}

pub(super) struct RemoteClient {
    child: Option<Child>,
    process_group: u32,
}

impl RemoteClient {
    pub(super) async fn stop(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(mut child) = self.child.take() {
            signal_process_group(self.process_group, rustix::process::Signal::TERM)?;
            if tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .is_err()
            {
                signal_process_group(self.process_group, rustix::process::Signal::KILL)?;
                child.wait().await?;
                return Err("remote CLI did not exit after SIGTERM".into());
            }
        }
        Ok(())
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = signal_process_group(self.process_group, rustix::process::Signal::KILL);
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

fn signal_process_group(process_group: u32, signal: rustix::process::Signal) -> io::Result<()> {
    let pid = rustix::process::Pid::from_raw(process_group as i32)
        .ok_or_else(|| io::Error::other("process group id is zero"))?;
    rustix::process::kill_process_group(pid, signal).map_err(io::Error::from)
}

pub(super) fn process_identity(pid: u32) -> Result<ProcessIdentity, Box<dyn Error>> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = stat
        .split_once(") ")
        .ok_or("malformed proc stat")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    Ok(ProcessIdentity {
        pid,
        start_time: fields
            .get(19)
            .ok_or("proc stat lacks start time")?
            .parse()?,
    })
}

pub(super) fn require_command_success(
    operation: &str,
    output: &std::process::Output,
) -> Result<(), Box<dyn Error>> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{operation} failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

pub(super) fn assert_shutdown_precedes_descriptor_removal(
    operation: &str,
    output: &std::process::Output,
) -> Result<(), Box<dyn Error>> {
    let stages = String::from_utf8_lossy(&output.stderr);
    let shutdown_stage = match operation {
        "disable" => "[verbose] disable: completed service-disable\n",
        "uninstall" => "[verbose] uninstall: completed service-stop\n",
        _ => return Err(format!("unsupported shutdown operation: {operation}").into()),
    };
    let stop = stages
        .find(shutdown_stage)
        .ok_or_else(|| format!("{operation} did not complete service shutdown"))?;
    let descriptor_stage = format!("[verbose] {operation}: completed descriptor-remove\n");
    let remove = stages
        .find(&descriptor_stage)
        .ok_or_else(|| format!("{operation} did not complete descriptor removal"))?;
    if stop >= remove {
        return Err(format!("{operation} removed the descriptor before service shutdown").into());
    }
    Ok(())
}

fn configured_codex() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(configured) = std::env::var_os("CODEX_SESSION_CONTROL_CODEX_BIN") {
        let configured = PathBuf::from(configured);
        if configured.is_absolute() && configured.is_file() {
            return Ok(configured);
        }
        return Err("CODEX_SESSION_CONTROL_CODEX_BIN must be an absolute file".into());
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").ok_or("PATH is missing")?) {
        let candidate = directory.join("codex");
        if candidate.is_file() {
            return Ok(candidate.canonicalize()?);
        }
    }
    Err("configured installed Codex CLI was not found".into())
}

pub(super) fn configured_controller() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(configured) = std::env::var_os("CODEX_SESSION_CONTROL_CONTROLLER_BIN") {
        let configured = PathBuf::from(configured);
        if configured.is_absolute() && configured.is_file() {
            return Ok(configured);
        }
        return Err("CODEX_SESSION_CONTROL_CONTROLLER_BIN must be an absolute file".into());
    }
    Ok(cargo_bin("codex-session-control"))
}

pub(super) fn scrub_command(
    command: &mut Command,
    codex_home: &Path,
    home: &Path,
    runtime: Option<&Path>,
) {
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("CODEX_HOME", codex_home);
    if let Some(runtime) = runtime {
        command.env("XDG_RUNTIME_DIR", runtime);
    }
}

async fn run_codex_in_home<const N: usize>(
    codex: &Path,
    codex_home: &Path,
    home: &Path,
    args: [&str; N],
) -> Result<String, Box<dyn Error>> {
    let output = run_codex_in_home_output(codex, codex_home, home, args).await?;
    if !output.status.success() {
        return Err(format!(
            "configured Codex command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    String::from_utf8(output.stdout).map_err(Into::into)
}

async fn run_codex_in_home_output(
    codex: &Path,
    codex_home: &Path,
    home: &Path,
    args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> Result<std::process::Output, Box<dyn Error>> {
    let mut command = Command::new(codex);
    scrub_command(&mut command, codex_home, home, None);
    Ok(command.args(args).output().await?)
}

fn aggregate_schema_digest(root: &Path) -> Result<String, Box<dyn Error>> {
    fn ordered(value: Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries = object.into_iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, ordered(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.into_iter().map(ordered).collect()),
            scalar => scalar,
        }
    }
    fn collect(
        root: &Path,
        directory: &Path,
        files: &mut Vec<(String, PathBuf)>,
    ) -> Result<(), Box<dyn Error>> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, files)?;
            } else if entry.file_type()?.is_file() {
                files.push((
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    path,
                ));
            } else {
                return Err("schema bundle contains a non-file entry".into());
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (relative, path) in files {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(serde_json::to_vec(&ordered(serde_json::from_slice(
            &fs::read(path)?,
        )?))?);
        digest.update([0]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub(super) fn shell_quote(path: &Path) -> String {
    shell_quote_text(&path.to_string_lossy())
}

pub(super) fn shell_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(super) fn request_session_control_tool_names(request: &Value) -> BTreeSet<String> {
    request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tool| {
            tool["type"].as_str() == Some("namespace")
                && tool["name"].as_str() == Some("mcp__codex_session_control")
        })
        .and_then(|namespace| namespace["tools"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(ToOwned::to_owned))
        .collect()
}

pub(super) fn request_has_session_control_tools(request: &Value) -> bool {
    !request_session_control_tool_names(request).is_empty()
}

pub(super) fn request_has_exact_session_control_tools(request: &Value) -> bool {
    request_session_control_tool_names(request)
        == BTreeSet::from(SESSION_CONTROL_TOOLS.map(str::to_owned))
}
