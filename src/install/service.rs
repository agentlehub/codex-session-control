use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{app_server::AppServerClient, error::ControllerError};

use super::{evidence::ResolvedUserPaths, paths::validate_control_socket};

pub(super) const CONTROL_SOCKET_READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_SOCKET_READINESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub(crate) struct LifecycleTarget {
    pub(super) paths: ResolvedUserPaths,
    pub(super) unit_name: String,
    pub(super) client_process_source: ClientProcessSource,
    #[cfg(test)]
    pub(super) test_hooks: LifecycleTestHooks,
}

#[derive(Clone, Debug)]
pub(super) enum ClientProcessSource {
    ProcRoot(PathBuf),
    #[cfg(test)]
    Snapshot(Vec<(u32, Vec<u8>)>),
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(super) struct LifecycleTestHooks {
    pub(super) fail_after_completed_stage: Option<&'static str>,
    pub(super) fail_service_unit_write: bool,
    pub(super) force_old_descriptor_removal_race: bool,
}

impl LifecycleTarget {
    pub(crate) fn production(paths: ResolvedUserPaths) -> Self {
        Self {
            paths,
            unit_name: "codex-session-control.service".to_owned(),
            client_process_source: ClientProcessSource::ProcRoot(PathBuf::from("/proc")),
            #[cfg(test)]
            test_hooks: LifecycleTestHooks::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn suffixed(mut paths: ResolvedUserPaths, nonce: &str) -> Self {
        assert!(
            nonce
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        );
        let unit_name = format!("codex-session-control-test-{nonce}.service");
        paths.unit.set_file_name(&unit_name);
        Self {
            paths,
            unit_name,
            client_process_source: ClientProcessSource::Snapshot(Vec::new()),
            test_hooks: LifecycleTestHooks::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn fail_after_completed_stage(mut self, stage: &'static str) -> Self {
        self.test_hooks.fail_after_completed_stage = Some(stage);
        self
    }

    #[cfg(test)]
    pub(super) fn force_old_descriptor_removal_race(mut self) -> Self {
        self.test_hooks.force_old_descriptor_removal_race = true;
        self
    }

    #[cfg(test)]
    pub(super) fn fail_service_unit_write(mut self) -> Self {
        self.test_hooks.fail_service_unit_write = true;
        self
    }

    #[cfg(test)]
    pub(super) fn with_client_process_snapshot(mut self, snapshot: Vec<(u32, Vec<u8>)>) -> Self {
        self.client_process_source = ClientProcessSource::Snapshot(snapshot);
        self
    }
}

pub(super) fn query_systemctl_state(
    systemctl: &Path,
    operation: &str,
    unit_name: &str,
) -> Result<bool, ControllerError> {
    Command::new(systemctl)
        .args(["--user", operation, unit_name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|_| ControllerError::Operational("systemctl command failed".to_owned()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CleanupServiceActivity {
    Active,
    Inactive,
    Unproven,
}

pub(super) fn query_cleanup_service_activity(
    systemctl: &Path,
    unit_name: &str,
) -> CleanupServiceActivity {
    let Ok(output) = Command::new(systemctl)
        .args(["--user", "is-active", unit_name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return CleanupServiceActivity::Unproven;
    };
    match (
        output.status.code(),
        std::str::from_utf8(&output.stdout).ok(),
    ) {
        (Some(0), Some("active\n")) => CleanupServiceActivity::Active,
        (Some(3), Some("inactive\n")) => CleanupServiceActivity::Inactive,
        _ => CleanupServiceActivity::Unproven,
    }
}

pub(super) async fn wait_for_control_socket(path: &Path, euid: u32) -> Result<(), ControllerError> {
    let deadline = tokio::time::Instant::now() + CONTROL_SOCKET_READINESS_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(super::missing_control_socket_error());
        }
        match fs::symlink_metadata(path) {
            Ok(_) => return validate_control_socket(path, euid),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(super::missing_control_socket_error());
                }
                tokio::time::sleep(
                    CONTROL_SOCKET_READINESS_POLL_INTERVAL
                        .min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(_) => return Err(super::missing_control_socket_error()),
        }
    }
}

pub(super) async fn verify_enabled_service(
    systemctl: &Path,
    target: &LifecycleTarget,
    expected_running_version: &str,
) -> Result<(), ControllerError> {
    if !query_systemctl_state(systemctl, "is-enabled", &target.unit_name)? {
        return Err(ControllerError::Operational(
            "service is not enabled".to_owned(),
        ));
    }
    if !query_systemctl_state(systemctl, "is-active", &target.unit_name)? {
        return Err(ControllerError::Operational(
            "service is not active".to_owned(),
        ));
    }
    wait_for_control_socket(&target.paths.socket, target.paths.euid).await?;
    let client = AppServerClient::new(
        target.paths.socket.clone(),
        target.paths.codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        expected_running_version.to_owned(),
    );
    let connection =
        client
            .connect_initialized()
            .await
            .map_err(|_| ControllerError::InvalidData {
                field: "service",
                reason: "app-server initialize failed",
            })?;
    if connection.compatibility_warning().is_some() {
        return Err(ControllerError::InvalidData {
            field: "service",
            reason: "running Codex version differs from executable",
        });
    }
    Ok(())
}

pub(super) fn verify_disabled_service(
    systemctl: &Path,
    target: &LifecycleTarget,
) -> Result<(), ControllerError> {
    let enabled = Command::new(systemctl)
        .args(["--user", "is-enabled", &target.unit_name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            ControllerError::Operational("service enabled state cannot be proven".to_owned())
        })?;
    match (
        enabled.status.code(),
        std::str::from_utf8(&enabled.stdout).ok(),
    ) {
        (Some(1), Some("disabled\n")) => {}
        (Some(0), Some("enabled\n")) => {
            return Err(ControllerError::Operational(
                "service remains enabled".to_owned(),
            ));
        }
        _ => {
            return Err(ControllerError::Operational(
                "service enabled state cannot be proven".to_owned(),
            ));
        }
    }
    match query_cleanup_service_activity(systemctl, &target.unit_name) {
        CleanupServiceActivity::Inactive => {}
        CleanupServiceActivity::Active => {
            return Err(ControllerError::Operational(
                "service remains active".to_owned(),
            ));
        }
        CleanupServiceActivity::Unproven => {
            return Err(ControllerError::Operational(
                "service activity cannot be proven".to_owned(),
            ));
        }
    }
    verify_absent_control_socket(target)
}

pub(super) fn verify_absent_managed_unit_stop(
    systemctl: &Path,
    target: &LifecycleTarget,
) -> Result<(), ControllerError> {
    match fs::symlink_metadata(&target.paths.unit) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ControllerError::Operational(
                "managed service unit absence cannot be proven".to_owned(),
            ));
        }
        Ok(_) => {
            return Err(ControllerError::Operational(
                "managed service unit remains present".to_owned(),
            ));
        }
    }
    let enabled = Command::new(systemctl)
        .args(["--user", "is-enabled", &target.unit_name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            ControllerError::Operational(
                "missing service enabled state cannot be proven".to_owned(),
            )
        })?;
    if !matches!(
        (
            enabled.status.code(),
            std::str::from_utf8(&enabled.stdout).ok()
        ),
        (Some(4), Some("not-found\n"))
    ) {
        return Err(ControllerError::Operational(
            "missing service enabled state cannot be proven".to_owned(),
        ));
    }
    match query_cleanup_service_activity(systemctl, &target.unit_name) {
        CleanupServiceActivity::Inactive => {}
        CleanupServiceActivity::Active => {
            return Err(ControllerError::Operational(
                "missing service remains active".to_owned(),
            ));
        }
        CleanupServiceActivity::Unproven => {
            return Err(ControllerError::Operational(
                "missing service activity cannot be proven".to_owned(),
            ));
        }
    }
    verify_absent_control_socket(target)
}

pub(super) fn verify_absent_control_socket(
    target: &LifecycleTarget,
) -> Result<(), ControllerError> {
    match fs::symlink_metadata(&target.paths.socket) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ControllerError::InvalidData {
            field: "socket",
            reason: "cannot verify absence",
        }),
        Ok(_) => Err(ControllerError::InvalidData {
            field: "socket",
            reason: "still exists",
        }),
    }
}

pub(super) fn run_systemctl<I, S>(systemctl: &Path, arguments: I) -> Result<(), ControllerError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(systemctl)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ControllerError::Operational("systemctl command failed".to_owned()))?;
    if status.success() {
        Ok(())
    } else {
        Err(ControllerError::Operational(
            "systemctl command failed".to_owned(),
        ))
    }
}

pub(super) async fn verify_setup_service(
    systemctl: &Path,
    target: &LifecycleTarget,
    expected_codex_version: &str,
) -> Result<(), ControllerError> {
    run_systemctl(
        systemctl,
        ["--user", "is-enabled", "--quiet", target.unit_name.as_str()],
    )?;
    run_systemctl(
        systemctl,
        ["--user", "is-active", "--quiet", target.unit_name.as_str()],
    )?;
    wait_for_control_socket(&target.paths.socket, target.paths.euid).await?;
    let client = AppServerClient::new(
        target.paths.socket.clone(),
        target.paths.codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        expected_codex_version.to_owned(),
    );
    let connection =
        client
            .connect_initialized()
            .await
            .map_err(|_| ControllerError::InvalidData {
                field: "service",
                reason: "app-server initialize failed",
            })?;
    if connection.compatibility_warning().is_some() {
        return Err(ControllerError::InvalidData {
            field: "service",
            reason: "running Codex version differs from executable",
        });
    }
    Ok(())
}

pub(super) fn detect_running_unattached_clients(
    source: &ClientProcessSource,
    euid: u32,
) -> BTreeSet<&'static str> {
    let snapshot = match source {
        ClientProcessSource::ProcRoot(root) => read_client_process_snapshot(root, euid),
        #[cfg(test)]
        ClientProcessSource::Snapshot(snapshot) => snapshot.clone(),
    };
    detect_running_unattached_clients_from_snapshot(
        euid,
        snapshot
            .iter()
            .map(|(uid, command_line)| (*uid, command_line.as_slice())),
    )
}

fn read_client_process_snapshot(proc_root: &Path, euid: u32) -> Vec<(u32, Vec<u8>)> {
    let Ok(processes) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut snapshot = Vec::new();
    for process in processes.flatten() {
        let name = process.file_name();
        if name.as_bytes().is_empty() || !name.as_bytes().iter().all(u8::is_ascii_digit) {
            continue;
        }
        let path = process.path();
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.uid() != euid {
            continue;
        }
        let Ok(command_line) = fs::read(path.join("cmdline")) else {
            continue;
        };
        snapshot.push((metadata.uid(), command_line));
    }
    snapshot
}

pub(super) fn detect_running_unattached_clients_from_snapshot<'a>(
    euid: u32,
    snapshot: impl IntoIterator<Item = (u32, &'a [u8])>,
) -> BTreeSet<&'static str> {
    snapshot
        .into_iter()
        .filter(|(uid, _)| *uid == euid)
        .filter_map(|(_, command_line)| classify_unattached_client(command_line))
        .collect()
}

pub(super) fn classify_unattached_client(command_line: &[u8]) -> Option<&'static str> {
    let arguments: Vec<&[u8]> = command_line
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .collect();
    let executable = arguments.first()?;
    let executable = Path::new(OsStr::from_bytes(executable));
    match executable.file_name()?.as_bytes() {
        b"codex-desktop" | b"Codex" => Some("Desktop"),
        b"codex"
            if arguments.get(1) == Some(&b"app-server".as_slice())
                || arguments.iter().skip(1).any(|argument| {
                    *argument == b"--remote" || argument.starts_with(b"--remote=")
                }) =>
        {
            None
        }
        b"codex" => Some("CLI"),
        _ => None,
    }
}

pub(super) fn append_unattached_client_guidance(
    stdout: &mut String,
    clients: &BTreeSet<&'static str>,
) {
    if clients.is_empty() {
        return;
    }
    stdout.push_str(&format!(
        "Unattached running clients: {}\n",
        clients.iter().copied().collect::<Vec<_>>().join(", ")
    ));
    stdout.push_str(
        "This running client was not attached or migrated.\n\
Desktop: fully exit and restart Desktop to use the shared app-server.\n\
CLI: exit and resume through codex-session-control codex.\n",
    );
}
