use std::{
    any::Any,
    error::Error,
    ffi::OsString,
    fs::{self, File},
    io::Write,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::process::Command;

use crate::live_harness::{
    ALL_SOURCE_KINDS, EXPECTED_CODEX_VERSION, LiveHarness, ProcessIdentity,
    assert_shutdown_precedes_descriptor_removal, configured_controller, process_identity,
    request_has_exact_session_control_tools, require_command_success, scrub_command, shell_quote,
};
use crate::normal_home_paths::{DISPOSABLE_CLI_CANARY_OPT_IN, DisposablePaths, atomic_write};

const DISPOSABLE_SYSTEMD_USER_OPT_IN: &str = "CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER";

pub(super) fn require_normal_home_ci_opt_ins(
    ci: Option<&std::ffi::OsStr>,
    cli_canary: Option<&std::ffi::OsStr>,
    systemd_user: Option<&std::ffi::OsStr>,
) -> Result<(), &'static str> {
    let enabled = Some(std::ffi::OsStr::new("1"));
    if ci != enabled {
        return Err("normal-home lifecycle requires CI=1");
    }
    if cli_canary != enabled {
        return Err("normal-home lifecycle requires the disposable CLI canary opt-in");
    }
    if systemd_user != enabled {
        return Err("normal-home lifecycle requires the disposable systemd-user opt-in");
    }
    Ok(())
}

pub(super) fn combine_cleanup_results(
    uninstall: Result<(), String>,
    absence: Result<(), String>,
) -> (Result<(), String>, bool) {
    match (uninstall, absence) {
        (Ok(()), Ok(())) => (Ok(()), true),
        (Err(error), Ok(())) => (
            Err(format!("normal-home cleanup uninstall failed: {error}")),
            true,
        ),
        (Ok(()), Err(error)) => (
            Err(format!(
                "normal-home cleanup absence verification failed: {error}"
            )),
            false,
        ),
        (Err(uninstall_error), Err(absence_error)) => (
            Err(format!(
                "normal-home cleanup uninstall failed: {uninstall_error}; \
                 absence verification also failed: {absence_error}"
            )),
            false,
        ),
    }
}
pub(super) struct DisposableNormalHome {
    pub(super) live: Option<LiveHarness>,
}

struct PreservedNormalState {
    files: Vec<(PathBuf, Vec<u8>)>,
    model_endpoint: String,
    unrelated_socket: std::os::unix::net::UnixListener,
    unrelated_socket_path: PathBuf,
    unrelated_socket_inode: u64,
}

impl Drop for PreservedNormalState {
    fn drop(&mut self) {
        for (path, _) in &self.files {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_file(&self.unrelated_socket_path);
    }
}

impl DisposableNormalHome {
    pub(super) fn contract() -> Self {
        let root = crate::test_support::private_tempdir();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
            .expect("protect isolated normal-home contract root");
        let home = root.path().join("home/codex-session-control-ci");
        let runtime = root.path().join("run/user/4242");
        for directory in [
            home.as_path(),
            home.join(".config").as_path(),
            home.join(".local").as_path(),
            home.join(".local/share").as_path(),
            runtime.as_path(),
        ] {
            fs::create_dir_all(directory).expect("create isolated normal-home contract directory");
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("protect isolated normal-home contract directory");
        }
        let paths = DisposablePaths::normal_home(home, runtime);
        assert_eq!(paths.codex_home, paths.home.join(".codex"));
        assert_eq!(
            paths.socket,
            paths.runtime.join("codex-session-control/app-server.sock")
        );
        for path in [
            &paths.config_dir,
            &paths.data_root,
            &paths.codex_home,
            &paths.runtime_dir,
            &paths.socket,
        ] {
            assert!(path.starts_with(root.path()));
            assert!(!path.starts_with("/run/user"));
        }
        Self { live: None }
    }

    pub(super) async fn prepare_ci() -> Result<Self, Box<dyn Error>> {
        require_normal_home_ci_opt_ins(
            std::env::var_os("CI").as_deref(),
            std::env::var_os(DISPOSABLE_CLI_CANARY_OPT_IN).as_deref(),
            std::env::var_os(DISPOSABLE_SYSTEMD_USER_OPT_IN).as_deref(),
        )?;
        let live = LiveHarness::prepare_disposable_product_ci().await?;
        Ok(Self { live: Some(live) })
    }

    pub(super) async fn setup_ci(&self) -> Result<(), Box<dyn Error>> {
        let desktop_launcher = self.live_ref()?.write_compatible_desktop_launcher()?;
        let setup = self
            .run_initial_setup(configured_controller()?.as_path(), &desktop_launcher)
            .await?;
        require_command_success("normal-home setup", &setup)?;
        let stages = String::from_utf8_lossy(&setup.stderr);
        let descriptor_stage = stages
            .find("[verbose] setup: completed descriptor\n")
            .ok_or("setup did not execute the descriptor stage")?;
        let enable_stage = stages
            .find("[verbose] setup: completed service-enable\n")
            .ok_or("setup did not execute the service-enable stage")?;
        assert!(descriptor_stage < enable_stage);
        assert!(self.desktop_descriptor()?.is_file());
        assert!(self.service_active().await?);
        assert!(self.live_ref()?.socket_path().exists());
        self.authority_identity().await?;
        Ok(())
    }

    pub(super) async fn prove_shared_authority(&mut self) -> Result<(), Box<dyn Error>> {
        let live = self.live_ref()?;
        let mut native = live
            .connect_named("normal_home_seed", "Normal-home seed")
            .await?;
        let persisted_thread = live.start_thread(&mut native).await?;
        let persisted_turn = live
            .start_turn(
                &mut native,
                &persisted_thread,
                "PERSIST_BEFORE_SHARED_AUTHORITY",
            )
            .await?;
        live.endpoint().wait_for_requests(1).await?;
        live.wait_for_turn_status(&mut native, &persisted_thread, &persisted_turn, "completed")
            .await?;
        drop(native);

        let before_disable = self.authority_identity().await?;
        let disable = self.run_installed(["--verbose", "disable"]).await?;
        require_command_success("normal-home disable", &disable)?;
        assert_shutdown_precedes_descriptor_removal("disable", &disable)?;
        assert!(!self.service_active().await?);
        assert!(!self.live_ref()?.socket_path().exists());
        assert!(!self.desktop_descriptor()?.exists());
        let enable = self.run_installed(["enable"]).await?;
        require_command_success("normal-home enable", &enable)?;
        assert!(self.desktop_descriptor()?.is_file());
        let authority = self.authority_identity().await?;
        assert_ne!(authority, before_disable);
        assert!(!Path::new(&format!("/proc/{}", before_disable.pid)).exists());

        let live = self.live_ref()?;
        let mut desktop = live
            .connect_named("codex_desktop_linux", "Compatible Codex Desktop")
            .await?;
        assert_eq!(
            desktop
                .request(
                    "thread/read",
                    json!({"threadId": persisted_thread, "includeTurns": false}),
                )
                .await?["thread"]["id"],
            persisted_thread
        );
        let desktop_thread = live.start_thread(&mut desktop).await?;
        let desktop_turn = live
            .start_turn(
                &mut desktop,
                &desktop_thread,
                "CREATE_FROM_FAKE_COMPATIBLE_DESKTOP",
            )
            .await?;
        live.endpoint().wait_for_requests(2).await?;
        live.wait_for_turn_status(&mut desktop, &desktop_thread, &desktop_turn, "completed")
            .await?;

        let mut wrapper = live
            .spawn_wrapper_resume(
                &self.installed_binary()?,
                &persisted_thread,
                "RESUME_PERSISTED_TASK_THROUGH_INSTALLED_WRAPPER",
            )
            .await?;
        live.endpoint().wait_for_requests(3).await?;
        let turns = desktop
            .request(
                "thread/turns/list",
                json!({
                    "threadId": persisted_thread,
                    "limit": 10,
                    "itemsView": "notLoaded",
                    "sortDirection": "desc"
                }),
            )
            .await?;
        let wrapper_turn = turns["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|turn| {
                let id = turn["id"].as_str()?;
                (id != persisted_turn).then(|| id.to_owned())
            })
            .ok_or("installed wrapper did not resume the persisted task")?;
        live.wait_for_turn_status(&mut desktop, &persisted_thread, &wrapper_turn, "completed")
            .await?;
        wrapper.stop().await?;
        assert_eq!(
            desktop
                .request(
                    "thread/read",
                    json!({"threadId": persisted_thread, "includeTurns": false}),
                )
                .await?["thread"]["id"],
            persisted_thread
        );

        live.prove_wrapper_replacement(&self.installed_binary()?)
            .await?;
        assert_eq!(self.authority_identity().await?, authority);
        self.live_ref()?
            .prove_wrapper_replacement(&self.installed_binary()?)
            .await?;
        assert_eq!(self.authority_identity().await?, authority);

        let listed = desktop
            .request(
                "thread/list",
                json!({"limit": 10, "sourceKinds": ALL_SOURCE_KINDS}),
            )
            .await?;
        for thread_id in [&persisted_thread, &desktop_thread] {
            assert!(listed["data"].as_array().is_some_and(|threads| {
                threads
                    .iter()
                    .any(|thread| thread["id"].as_str() == Some(thread_id))
            }));
        }
        drop(desktop);
        Ok(())
    }

    pub(super) async fn prove_restart_boundaries(&mut self) -> Result<(), Box<dyn Error>> {
        let authority = self.authority_identity().await?;
        let socket_path = self.live_ref()?.socket_path().to_path_buf();
        let socket_inode = fs::symlink_metadata(&socket_path)?.ino();

        let desktop = self
            .live_ref()?
            .connect_named("codex_desktop_linux", "Compatible Codex Desktop")
            .await?;
        drop(desktop);
        assert_eq!(self.authority_identity().await?, authority);
        let reopened = self
            .live_ref()?
            .connect_named("codex_desktop_linux", "Compatible Codex Desktop")
            .await?;
        drop(reopened);
        assert_eq!(self.authority_identity().await?, authority);

        let mut native = self.live_ref()?.connect().await?;
        let persisted_thread = self.live_ref()?.start_thread(&mut native).await?;
        let persisted_turn = self
            .live_ref()?
            .start_turn(
                &mut native,
                &persisted_thread,
                "PERSIST_ACROSS_EXECUTABLE_AND_UNIT_UPDATE",
            )
            .await?;
        self.live_ref()?.endpoint().wait_for_requests(1).await?;
        self.live_ref()?
            .wait_for_turn_status(&mut native, &persisted_thread, &persisted_turn, "completed")
            .await?;
        drop(native);

        let (changed_controller, changed_codex_dir) = self.stage_changed_controller_and_native()?;
        let update = self
            .run_staged_candidate(
                &changed_controller,
                ["--verbose", "update"],
                Some(&changed_codex_dir),
            )
            .await?;
        require_command_success("normal-home changed-executable update", &update)?;
        let update_diagnostics = String::from_utf8_lossy(&update.stderr);
        let restart = update_diagnostics
            .find("[verbose] update/apply: completed service-restart\n")
            .ok_or("staged update did not report its service restart")?;
        let manifest = update_diagnostics
            .find("[verbose] update/apply: completed manifest\n")
            .ok_or("staged update did not report its manifest commit")?;
        assert!(restart < manifest);
        assert!(!update_diagnostics.contains("[verbose] update/outer:"));
        let replaced = self.authority_identity().await?;
        assert_ne!(replaced, authority);
        assert_eq!(self.live_ref()?.socket_path(), socket_path);
        assert_ne!(
            fs::symlink_metadata(self.live_ref()?.socket_path())?.ino(),
            socket_inode
        );
        assert!(!Path::new(&format!("/proc/{}", authority.pid)).exists());
        let unit = fs::read_to_string(self.product_unit()?)?;
        assert!(
            unit.contains(
                changed_codex_dir
                    .join("codex")
                    .to_str()
                    .ok_or("changed Codex path is not UTF-8")?
            ),
            "the real user unit did not select the changed compatible executable"
        );
        let mut reconnected = self.live_ref()?.connect().await?;
        assert_eq!(
            reconnected
                .request(
                    "thread/read",
                    json!({"threadId": persisted_thread, "includeTurns": false}),
                )
                .await?["thread"]["id"],
            persisted_thread
        );
        drop(reconnected);

        Ok(())
    }

    pub(super) async fn prove_projection_preservation(&mut self) -> Result<(), Box<dyn Error>> {
        let live = self.live_ref()?;
        let mut native = live.connect().await?;
        let persisted_thread = live.start_thread(&mut native).await?;
        let persisted_turn = live
            .start_turn(
                &mut native,
                &persisted_thread,
                "PERSIST_ACROSS_PROJECTION_RECONCILIATION",
            )
            .await?;
        live.endpoint().wait_for_requests(1).await?;
        live.wait_for_turn_status(&mut native, &persisted_thread, &persisted_turn, "completed")
            .await?;
        let sentinels = live.seed_preserved_normal_state()?;
        let authority = self.authority_identity().await?;
        let projection = self.product_projection_file()?;
        fs::remove_file(&projection)?;
        let same_byte_candidate = self.stage_same_byte_controller()?;

        let running_repair = self
            .run_staged_candidate(&same_byte_candidate, ["--verbose", "update"], None)
            .await?;
        require_command_success("running projection reconciliation", &running_repair)?;
        assert!(projection.is_file());
        assert_eq!(self.authority_identity().await?, authority);
        let live = self.live_ref()?;
        let projected_thread = live.start_thread(&mut native).await?;
        live.start_turn(&mut native, &projected_thread, "AFTER_RUNNING_PROJECTION")
            .await?;
        let projected = live.endpoint().wait_for_request(2).await?;
        assert!(request_has_exact_session_control_tools(&projected));
        live.assert_preserved_normal_state(&sentinels)?;

        drop(native);
        let disable = self.run_installed(["--verbose", "disable"]).await?;
        require_command_success("projection-case disable", &disable)?;
        assert!(!self.service_active().await?);
        fs::remove_file(&projection)?;
        let stopped_repair = self
            .run_staged_candidate(&same_byte_candidate, ["--verbose", "update"], None)
            .await?;
        require_command_success("stopped projection reconciliation", &stopped_repair)?;
        assert!(projection.is_file());
        assert!(!self.service_active().await?);
        self.live_ref()?.assert_preserved_normal_state(&sentinels)?;
        let enable = self.run_installed(["enable"]).await?;
        require_command_success("projection-case enable", &enable)?;
        let mut reopened = self.live_ref()?.connect().await?;
        assert_eq!(
            reopened
                .request(
                    "thread/read",
                    json!({"threadId": persisted_thread, "includeTurns": false}),
                )
                .await?["thread"]["id"],
            persisted_thread
        );
        drop(reopened);

        Ok(())
    }

    pub(super) async fn prove_uninstall_preservation(&mut self) -> Result<(), Box<dyn Error>> {
        let live = self.live_ref()?;
        let mut native = live.connect().await?;
        let persisted_thread = live.start_thread(&mut native).await?;
        let persisted_turn = live
            .start_turn(
                &mut native,
                &persisted_thread,
                "PERSIST_ACROSS_SERVICE_FIRST_UNINSTALL",
            )
            .await?;
        live.endpoint().wait_for_requests(1).await?;
        live.wait_for_turn_status(&mut native, &persisted_thread, &persisted_turn, "completed")
            .await?;
        let sentinels = live.seed_preserved_normal_state()?;
        drop(native);

        let uninstall = self.run_installed(["--verbose", "uninstall"]).await?;
        require_command_success("normal-home uninstall", &uninstall)?;
        assert_shutdown_precedes_descriptor_removal("uninstall", &uninstall)?;
        assert!(!self.service_active().await?);
        let paths = self.live_ref()?.disposable_paths()?;
        assert!(!paths.socket.exists());
        assert!(!self.desktop_descriptor()?.exists());
        assert!(!paths.data_root.exists());
        assert!(!paths.config_dir.exists());
        assert!(!paths.runtime_dir.exists());
        assert!(paths.codex_home.is_dir());
        assert!(!self.installed_binary()?.exists());
        assert!(!self.product_unit()?.exists());
        assert!(self.live_ref()?.codex_home_contains_session()?);
        self.live_ref()?.assert_preserved_normal_state(&sentinels)?;

        Ok(())
    }

    pub(super) fn live_ref(&self) -> Result<&LiveHarness, Box<dyn Error>> {
        self.live
            .as_ref()
            .ok_or_else(|| "normal-home live fixture is unavailable".into())
    }

    pub(super) fn live_mut(&mut self) -> Result<&mut LiveHarness, Box<dyn Error>> {
        self.live
            .as_mut()
            .ok_or_else(|| "normal-home live fixture is unavailable".into())
    }

    pub(super) fn installed_binary(&self) -> Result<PathBuf, Box<dyn Error>> {
        Ok(self
            .live_ref()?
            .disposable_paths()?
            .home
            .join(".local/bin/codex-session-control"))
    }

    pub(super) fn product_unit(&self) -> Result<PathBuf, Box<dyn Error>> {
        Ok(self
            .live_ref()?
            .disposable_paths()?
            .home
            .join(".config/systemd/user/codex-session-control.service"))
    }

    pub(super) fn product_projection_file(&self) -> Result<PathBuf, Box<dyn Error>> {
        Ok(self
            .live_ref()?
            .disposable_paths()?
            .data_root
            .join("marketplace/plugins/codex-session-control/.mcp.json"))
    }

    pub(super) fn desktop_descriptor(&self) -> Result<PathBuf, Box<dyn Error>> {
        Ok(self
            .live_ref()?
            .disposable_paths()?
            .home
            .join(".config/codex-desktop/app-server-attachment.json"))
    }

    pub(super) fn product_command(
        &self,
        executable: &Path,
        codex_path: Option<&Path>,
    ) -> Result<Command, Box<dyn Error>> {
        let live = self.live_ref()?;
        let paths = live.disposable_paths()?;
        let default_native_bin = live.root.path().join("native-bin");
        let native_bin = codex_path.unwrap_or(&default_native_bin);
        let path = std::env::join_paths(std::iter::once(native_bin.to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))?;
        let mut command = Command::new(executable);
        scrub_command(
            &mut command,
            &live.codex_home,
            &live.process_home,
            live.process_runtime.as_deref(),
        );
        command
            .env("PATH", path)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/bus", paths.runtime.display()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(command)
    }

    pub(super) async fn run_initial_setup(
        &self,
        controller: &Path,
        desktop_launcher: &Path,
    ) -> Result<std::process::Output, Box<dyn Error>> {
        Ok(self
            .product_command(controller, None)?
            .args(["--verbose", "setup", "--desktop-launcher"])
            .arg(desktop_launcher)
            .output()
            .await?)
    }

    pub(super) async fn run_installed<const N: usize>(
        &self,
        args: [&str; N],
    ) -> Result<std::process::Output, Box<dyn Error>> {
        Ok(self
            .product_command(&self.installed_binary()?, None)?
            .args(args)
            .output()
            .await?)
    }

    pub(super) async fn run_staged_candidate<const N: usize>(
        &self,
        candidate: &Path,
        args: [&str; N],
        codex_path: Option<&Path>,
    ) -> Result<std::process::Output, Box<dyn Error>> {
        assert_ne!(candidate, self.installed_binary()?);
        Ok(self
            .product_command(candidate, codex_path)?
            .env("CODEX_SESSION_CONTROL_STAGED_UPDATE", "1")
            .args(args)
            .output()
            .await?)
    }

    pub(super) async fn service_active(&self) -> Result<bool, Box<dyn Error>> {
        let output = self
            .run_systemctl(["is-active", "codex-session-control.service"])
            .await?;
        Ok(output.status.success() && output.stdout == b"active\n")
    }

    pub(super) async fn authority_identity(&self) -> Result<ProcessIdentity, Box<dyn Error>> {
        let output = self
            .run_systemctl([
                "show",
                "--property=MainPID",
                "--value",
                "codex-session-control.service",
            ])
            .await?;
        require_command_success("systemd MainPID query", &output)?;
        let pid = std::str::from_utf8(&output.stdout)?.trim().parse::<u32>()?;
        if pid == 0 {
            return Err("normal-home authority MainPID is zero".into());
        }
        process_identity(pid)
    }

    pub(super) async fn run_systemctl<const N: usize>(
        &self,
        args: [&str; N],
    ) -> Result<std::process::Output, Box<dyn Error>> {
        let live = self.live_ref()?;
        let paths = live.disposable_paths()?;
        let mut command = Command::new("/usr/bin/systemctl");
        scrub_command(
            &mut command,
            &live.codex_home,
            &live.process_home,
            live.process_runtime.as_deref(),
        );
        Ok(command
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/bus", paths.runtime.display()),
            )
            .args(["--user"])
            .args(args)
            .output()
            .await?)
    }

    pub(super) fn stage_same_byte_controller(&self) -> Result<PathBuf, Box<dyn Error>> {
        let installed = self.installed_binary()?;
        let candidate = self.live_ref()?.root.path().join("same-byte-controller");
        fs::copy(&installed, &candidate)?;
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700))?;
        assert_eq!(fs::read(&candidate)?, fs::read(installed)?);
        Ok(candidate)
    }

    pub(super) fn stage_changed_controller_and_native(
        &self,
    ) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
        let controller = self.live_ref()?.root.path().join("changed-controller");
        fs::copy(self.installed_binary()?, &controller)?;
        File::options()
            .append(true)
            .open(&controller)?
            .write_all(b"\0task9-controller")?;
        fs::set_permissions(&controller, fs::Permissions::from_mode(0o700))?;
        let directory = self.live_ref()?.root.path().join("changed-native-bin");
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let candidate = directory.join("codex");
        fs::copy(&self.live_ref()?.codex, &candidate)?;
        File::options()
            .append(true)
            .open(&candidate)?
            .write_all(b"\0task9-native-codex")?;
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700))?;
        Ok((controller, directory))
    }

    pub(super) async fn finish_proof(
        &mut self,
        proof: Result<Result<(), Box<dyn Error>>, Box<dyn Any + Send>>,
    ) -> Result<(), Box<dyn Error>> {
        let proof = match proof {
            Ok(result) => result,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic payload");
                Err(format!("normal-home proof panicked: {message}").into())
            }
        };
        let (cleanup, absence_established) = self.guarded_product_cleanup().await;
        let result = match (proof, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Err(proof_error), Err(cleanup_error)) => Err(format!(
                "normal-home proof failed: {proof_error}; cleanup also failed: {cleanup_error}"
            )
            .into()),
        };
        if !absence_established {
            let error = result
                .as_ref()
                .expect_err("failed absence verification must produce an error");
            eprintln!(
                "normal-home cleanup could not establish disposable product absence; \
                 terminating the live test process: {error}"
            );
            std::process::exit(1);
        }
        result
    }

    pub(super) async fn guarded_product_cleanup(&mut self) -> (Result<(), Box<dyn Error>>, bool) {
        let uninstall: Result<(), Box<dyn Error>> = async {
            if self.installed_binary()?.exists() {
                let uninstall = self.run_installed(["--verbose", "uninstall"]).await?;
                require_command_success("normal-home cleanup uninstall", &uninstall)?;
                assert_shutdown_precedes_descriptor_removal("uninstall", &uninstall)?;
            }
            Ok(())
        }
        .await;
        let absence = self.finish_cleanup().await;
        let (result, absence_established) = combine_cleanup_results(
            uninstall.map_err(|error| error.to_string()),
            absence.map_err(|error| error.to_string()),
        );
        (
            result.map_err(|error| -> Box<dyn Error> { error.into() }),
            absence_established,
        )
    }

    pub(super) async fn finish_cleanup(&mut self) -> Result<(), Box<dyn Error>> {
        if self.service_active().await? {
            return Err("normal-home cleanup left the service active".into());
        }
        for path in [
            self.live_ref()?.socket_path().to_path_buf(),
            self.installed_binary()?,
            self.product_unit()?,
            self.desktop_descriptor()?,
        ] {
            if path.exists() {
                return Err(
                    format!("normal-home cleanup left product state: {}", path.display()).into(),
                );
            }
        }
        self.live_mut()?.stop_and_clean_disposable().await
    }
}

impl LiveHarness {
    pub(super) fn write_compatible_desktop_launcher(&self) -> Result<PathBuf, Box<dyn Error>> {
        let launcher = self.root.path().join("compatible-codex-desktop");
        atomic_write(
            &launcher,
            b"#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then\n  printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'\n  exit 0\nfi\nexit 64\n",
            0o700,
        )?;
        Ok(launcher)
    }

    fn seed_preserved_normal_state(&self) -> Result<PreservedNormalState, Box<dyn Error>> {
        let paths = self.disposable_paths()?;
        let files = [
            (
                self.codex_home.join("auth.json"),
                br#"{"fixture":"authentication-preserved"}"#.as_slice(),
            ),
            (
                self.codex_home.join("rollouts/preservation.jsonl"),
                br#"{"fixture":"rollout-preserved"}"#.as_slice(),
            ),
            (
                self.codex_home.join("plugins/unrelated/plugin-state.json"),
                br#"{"fixture":"unrelated-plugin-preserved"}"#.as_slice(),
            ),
            (
                paths.home.join(".config/systemd/user/pre-existing.service"),
                b"[Service]\nExecStart=/bin/true\n".as_slice(),
            ),
            (
                paths
                    .home
                    .join(".config/codex-desktop/profile-pre-existing.json"),
                br#"{"fixture":"unrelated-desktop-profile-preserved"}"#.as_slice(),
            ),
        ]
        .into_iter()
        .map(|(path, bytes)| {
            fs::create_dir_all(path.parent().unwrap())?;
            atomic_write(&path, bytes, 0o600)?;
            Ok((path, bytes.to_vec()))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
        let unrelated_socket_path = paths.runtime.join("unrelated-pre-existing.sock");
        let unrelated_socket = std::os::unix::net::UnixListener::bind(&unrelated_socket_path)?;
        fs::set_permissions(&unrelated_socket_path, fs::Permissions::from_mode(0o600))?;
        let unrelated_socket_inode = fs::symlink_metadata(&unrelated_socket_path)?.ino();
        Ok(PreservedNormalState {
            files,
            model_endpoint: format!(
                "base_url = \"http://127.0.0.1:{}/v1\"",
                self.endpoint.address.port()
            ),
            unrelated_socket,
            unrelated_socket_path,
            unrelated_socket_inode,
        })
    }

    fn assert_preserved_normal_state(
        &self,
        sentinels: &PreservedNormalState,
    ) -> Result<(), Box<dyn Error>> {
        for (path, expected) in &sentinels.files {
            assert_eq!(
                fs::read(path)?,
                *expected,
                "normal-home state changed: {}",
                path.display()
            );
        }
        assert!(
            fs::read_to_string(self.codex_home.join("config.toml"))?
                .contains(&sentinels.model_endpoint),
            "the selected normal home's model endpoint changed"
        );
        let metadata = fs::symlink_metadata(&sentinels.unrelated_socket_path)?;
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.ino(), sentinels.unrelated_socket_inode);
        assert!(
            sentinels.unrelated_socket.local_addr()?.as_pathname()
                == Some(sentinels.unrelated_socket_path.as_path())
        );
        Ok(())
    }

    pub(super) async fn prove_wrapper_replacement(
        &self,
        controller: &Path,
    ) -> Result<(), Box<dyn Error>> {
        let paths = self.disposable_paths()?;
        let config_path = paths.config_dir.join("config.toml");
        let manifest_path = paths.data_root.join("installed-release.json");
        let original_config = fs::read(&config_path)?;
        let original_manifest = fs::read(&manifest_path)?;
        let fake_codex = self.root.path().join("wrapper-native-codex");
        let captured_pid = self.root.path().join("wrapper.pid");
        let captured_home = self.root.path().join("wrapper.codex-home");
        let captured_argv = self.root.path().join("wrapper.argv");
        let shell_marker = self.root.path().join("shell-marker-must-not-exist");
        let shell_marker_argument = format!("$(touch {})", shell_marker.display()).into_bytes();
        let script = format!(
            "#!/bin/sh\nif [ \"${{1-}}\" = \"--version\" ]; then\n\
             printf '%s\\n' '{}'\nexit 0\nfi\nprintf '%s\\n' \"$$\" > {}\n\
             printf '%s' \"$CODEX_HOME\" > {}\n: > {}\n\
             for argument do printf '%s\\0' \"$argument\" >> {}; done\nexit 73\n",
            EXPECTED_CODEX_VERSION,
            shell_quote(&captured_pid),
            shell_quote(&captured_home),
            shell_quote(&captured_argv),
            shell_quote(&captured_argv),
        );
        atomic_write(&fake_codex, script.as_bytes(), 0o700)?;

        let caller_cwd = std::env::current_dir()?;
        let probe = async {
            let mut product_config: toml::Value =
                toml::from_str(std::str::from_utf8(&original_config)?)?;
            product_config["codex_executable"] =
                toml::Value::String(fake_codex.to_string_lossy().into_owned());
            atomic_write(
                &config_path,
                toml::to_string(&product_config)?.as_bytes(),
                0o600,
            )?;
            let mut manifest: Value = serde_json::from_slice(&original_manifest)?;
            manifest["codexExecutable"] = json!(fake_codex);
            atomic_write(
                &manifest_path,
                serde_json::to_vec_pretty(&manifest)?.as_slice(),
                0o600,
            )?;

            let raw_user_args = vec![
                OsString::from("--model"),
                OsString::from("two words"),
                OsString::from("--remote"),
                OsString::from("unix://user-supplied"),
                OsString::from_vec(b"--raw-\xff".to_vec()),
                OsString::from_vec(shell_marker_argument.clone()),
            ];
            let mut command = Command::new(controller);
            scrub_command(
                &mut command,
                &self.codex_home,
                &self.process_home,
                self.process_runtime.as_deref(),
            );
            command
                .arg("codex")
                .args(&raw_user_args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = command.spawn()?;
            let wrapper_pid = child.id().ok_or("wrapper pid is unavailable")?;
            let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
                .await
                .map_err(|_| "wrapper process timed out")??;
            Ok::<_, Box<dyn Error>>((wrapper_pid, status))
        }
        .await;
        let restore_config = atomic_write(&config_path, &original_config, 0o600);
        let restore_manifest = atomic_write(&manifest_path, &original_manifest, 0o600);
        match (restore_config, restore_manifest) {
            (Ok(()), Ok(())) => {}
            (Err(config_error), Ok(())) => {
                return Err(format!("wrapper config restoration failed: {config_error}").into());
            }
            (Ok(()), Err(manifest_error)) => {
                return Err(
                    format!("wrapper manifest restoration failed: {manifest_error}").into(),
                );
            }
            (Err(config_error), Err(manifest_error)) => {
                return Err(format!(
                    "wrapper evidence restoration failed: config: {config_error}; manifest: {manifest_error}"
                )
                .into());
            }
        }
        let (wrapper_pid, status) = probe?;
        assert_eq!(
            fs::read_to_string(&captured_pid)?.trim().parse::<u32>()?,
            wrapper_pid,
            "the controller process was not replaced by the configured native executable"
        );
        let argv: Vec<Vec<u8>> = fs::read(&captured_argv)?
            .split(|byte| *byte == 0)
            .filter(|argument| !argument.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        assert_eq!(status.code().ok_or("wrapper exit was not a code")?, 73);
        assert_eq!(
            fs::read(captured_home)?,
            self.codex_home.as_os_str().as_bytes()
        );
        assert_eq!(
            argv,
            vec![
                b"--remote".to_vec(),
                format!("unix://{}", self.socket.display()).into_bytes(),
                b"--cd".to_vec(),
                caller_cwd.as_os_str().as_bytes().to_vec(),
                b"--model".to_vec(),
                b"two words".to_vec(),
                b"--remote".to_vec(),
                b"unix://user-supplied".to_vec(),
                b"--raw-\xff".to_vec(),
                shell_marker_argument,
            ]
        );
        assert!(!shell_marker.exists());
        Ok(())
    }
}
