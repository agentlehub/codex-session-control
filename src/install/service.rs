use std::{
    ffi::OsStr,
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{app_server::AppServerClient, cli_output::RunningClientFacts, error::ControllerError};

use super::{evidence::ResolvedUserPaths, paths::validate_control_socket};

pub(super) const CONTROL_SOCKET_READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_SOCKET_READINESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub(crate) struct LifecycleTarget {
    pub(super) paths: ResolvedUserPaths,
    pub(super) unit_name: String,
    pub(super) client_process_source: ClientProcessSource,
    pub(super) caller_cgroup_source: CallerCgroupSource,
    #[cfg(test)]
    pub(super) test_hooks: LifecycleTestHooks,
}

#[derive(Clone, Debug)]
pub(super) enum ClientProcessSource {
    ProcRoot(PathBuf),
    #[cfg(test)]
    Snapshot(Vec<(u32, Vec<u8>)>),
}

#[derive(Clone, Debug)]
pub(super) enum CallerCgroupSource {
    ProcSelf,
    #[cfg(test)]
    Snapshot(Vec<u8>),
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
            caller_cgroup_source: CallerCgroupSource::ProcSelf,
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
            caller_cgroup_source: CallerCgroupSource::ProcSelf,
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

    #[cfg(test)]
    pub(super) fn with_caller_cgroup_snapshot(mut self, snapshot: Vec<u8>) -> Self {
        self.caller_cgroup_source = CallerCgroupSource::Snapshot(snapshot);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SystemctlEnablementState {
    Enabled,
    Disabled,
    NotFound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SystemctlActivityState {
    Active,
    Inactive,
}

pub(super) fn query_systemctl_enablement(
    systemctl: &Path,
    unit_name: &str,
) -> Result<SystemctlEnablementState, ControllerError> {
    match query_service_enablement(systemctl, unit_name) {
        ServiceEnablement::Enabled => Ok(SystemctlEnablementState::Enabled),
        ServiceEnablement::Disabled => Ok(SystemctlEnablementState::Disabled),
        ServiceEnablement::Absent => Ok(SystemctlEnablementState::NotFound),
        ServiceEnablement::Unproven => Err(systemctl_state_query_error("is-enabled")),
    }
}

pub(super) fn query_systemctl_activity(
    systemctl: &Path,
    unit_name: &str,
) -> Result<SystemctlActivityState, ControllerError> {
    match query_service_activity(systemctl, unit_name) {
        ServiceActivity::Active => Ok(SystemctlActivityState::Active),
        ServiceActivity::Inactive => Ok(SystemctlActivityState::Inactive),
        ServiceActivity::Unproven => Err(systemctl_state_query_error("is-active")),
    }
}

fn systemctl_state_query_error(operation: &str) -> ControllerError {
    ControllerError::Operational(format!(
        "systemctl {operation} could not provide trustworthy service state"
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ServiceEnablement {
    Enabled,
    Disabled,
    Absent,
    Unproven,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ServiceActivity {
    Active,
    Inactive,
    Unproven,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CallerUnitEvidence {
    WhoAmI,
    ControlGroup,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CallerUnitInspection {
    SelfHosted(CallerUnitEvidence),
    Independent,
    Unknown { evidence: String },
}

pub(super) fn classify_service_enablement(code: Option<i32>, stdout: &[u8]) -> ServiceEnablement {
    match (code, stdout) {
        (Some(0), b"enabled\n") => ServiceEnablement::Enabled,
        (Some(1), b"disabled\n") => ServiceEnablement::Disabled,
        (Some(4), b"not-found\n") => ServiceEnablement::Absent,
        _ => ServiceEnablement::Unproven,
    }
}

pub(super) fn classify_service_activity(code: Option<i32>, stdout: &[u8]) -> ServiceActivity {
    match (code, stdout) {
        (Some(0), b"active\n") => ServiceActivity::Active,
        (Some(3 | 4), b"inactive\n") => ServiceActivity::Inactive,
        _ => ServiceActivity::Unproven,
    }
}

pub(super) fn query_service_enablement(systemctl: &Path, unit_name: &str) -> ServiceEnablement {
    let Ok(output) = Command::new(systemctl)
        .args(["--user", "is-enabled", unit_name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return ServiceEnablement::Unproven;
    };
    classify_service_enablement(output.status.code(), &output.stdout)
}

pub(super) fn query_service_activity(systemctl: &Path, unit_name: &str) -> ServiceActivity {
    let Ok(output) = Command::new(systemctl)
        .args(["--user", "is-active", unit_name])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return ServiceActivity::Unproven;
    };
    classify_service_activity(output.status.code(), &output.stdout)
}

fn exact_unit_name(stdout: &[u8]) -> Option<&str> {
    let unit_name = std::str::from_utf8(stdout).ok()?.strip_suffix('\n')?;
    if unit_name.is_empty()
        || unit_name.contains(['\n', '\r', '\0', '/'])
        || unit_name.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(unit_name)
}

fn exact_non_root_absolute_cgroup_path(bytes: &[u8]) -> Option<&str> {
    let path = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
    if !path.starts_with('/') || path == "/" || path.contains(['\n', '\r', '\0']) {
        return None;
    }
    let mut components = path.split('/');
    if components.next() != Some("")
        || components.any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }
    Some(path)
}

fn exact_unified_cgroup_path(proc_self_cgroup: &[u8]) -> Option<&str> {
    let cgroup = std::str::from_utf8(proc_self_cgroup).ok()?;
    let path = cgroup.strip_prefix("0::")?;
    exact_non_root_absolute_cgroup_path(path.as_bytes())
}

pub(super) fn cgroup_proves_self_hosted(control_group: &[u8], proc_self_cgroup: &[u8]) -> bool {
    let Some(control_group) = exact_non_root_absolute_cgroup_path(control_group) else {
        return false;
    };
    let Some(caller_group) = exact_unified_cgroup_path(proc_self_cgroup) else {
        return false;
    };
    caller_group == control_group
        || caller_group
            .strip_prefix(control_group)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn caller_cgroup_snapshot(source: &CallerCgroupSource) -> Result<Vec<u8>, String> {
    match source {
        CallerCgroupSource::ProcSelf => {
            fs::read("/proc/self/cgroup").map_err(|_| "cannot read /proc/self/cgroup".to_owned())
        }
        #[cfg(test)]
        CallerCgroupSource::Snapshot(snapshot) => Ok(snapshot.clone()),
    }
}

pub(super) fn inspect_caller_unit(
    systemctl: &Path,
    target: &LifecycleTarget,
) -> CallerUnitInspection {
    let whoami = Command::new(systemctl)
        .args(["--user", "whoami"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    if let Ok(output) = whoami
        && output.status.code() == Some(0)
        && let Some(unit_name) = exact_unit_name(&output.stdout)
    {
        return if unit_name == target.unit_name {
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::WhoAmI)
        } else {
            CallerUnitInspection::Independent
        };
    }

    let control_group = Command::new(systemctl)
        .args([
            "--user",
            "show",
            "--property=ControlGroup",
            "--value",
            target.unit_name.as_str(),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(control_group) = control_group else {
        return CallerUnitInspection::Unknown {
            evidence: "target ControlGroup cannot be queried".to_owned(),
        };
    };
    if control_group.status.code() != Some(0) {
        return CallerUnitInspection::Unknown {
            evidence: "target ControlGroup cannot be proven".to_owned(),
        };
    }
    let caller_cgroup = match caller_cgroup_snapshot(&target.caller_cgroup_source) {
        Ok(caller_cgroup) => caller_cgroup,
        Err(evidence) => return CallerUnitInspection::Unknown { evidence },
    };
    if cgroup_proves_self_hosted(&control_group.stdout, &caller_cgroup) {
        CallerUnitInspection::SelfHosted(CallerUnitEvidence::ControlGroup)
    } else {
        CallerUnitInspection::Unknown {
            evidence: "caller cgroup does not prove self-hosting".to_owned(),
        }
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
    match query_systemctl_enablement(systemctl, &target.unit_name)? {
        SystemctlEnablementState::Enabled => {}
        SystemctlEnablementState::Disabled | SystemctlEnablementState::NotFound => {
            return Err(ControllerError::Operational(
                "service is not enabled".to_owned(),
            ));
        }
    }
    match query_systemctl_activity(systemctl, &target.unit_name)? {
        SystemctlActivityState::Active => {}
        SystemctlActivityState::Inactive => {
            return Err(ControllerError::Operational(
                "service is not active".to_owned(),
            ));
        }
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
    match query_service_enablement(systemctl, &target.unit_name) {
        ServiceEnablement::Disabled => {}
        ServiceEnablement::Enabled => {
            return Err(ControllerError::Operational(
                "service remains enabled".to_owned(),
            ));
        }
        ServiceEnablement::Absent | ServiceEnablement::Unproven => {
            return Err(ControllerError::Operational(
                "service enabled state cannot be proven".to_owned(),
            ));
        }
    }
    match query_service_activity(systemctl, &target.unit_name) {
        ServiceActivity::Inactive => {}
        ServiceActivity::Active => {
            return Err(ControllerError::Operational(
                "service remains active".to_owned(),
            ));
        }
        ServiceActivity::Unproven => {
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
    if query_service_enablement(systemctl, &target.unit_name) != ServiceEnablement::Absent {
        return Err(ControllerError::Operational(
            "missing service enabled state cannot be proven".to_owned(),
        ));
    }
    match query_service_activity(systemctl, &target.unit_name) {
        ServiceActivity::Inactive => {}
        ServiceActivity::Active => {
            return Err(ControllerError::Operational(
                "missing service remains active".to_owned(),
            ));
        }
        ServiceActivity::Unproven => {
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
    verify_enabled_service(systemctl, target, expected_codex_version).await
}

pub(super) fn detect_running_unattached_clients(
    source: &ClientProcessSource,
    euid: u32,
) -> RunningClientFacts {
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
) -> RunningClientFacts {
    let mut facts = RunningClientFacts::default();
    for (_, command_line) in snapshot.into_iter().filter(|(uid, _)| *uid == euid) {
        match classify_unattached_client(command_line) {
            Some(UnattachedClient::Cli) => facts.cli = true,
            Some(UnattachedClient::Desktop) => facts.desktop = true,
            None => {}
        }
    }
    facts
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnattachedClient {
    Cli,
    Desktop,
}

fn classify_unattached_client(command_line: &[u8]) -> Option<UnattachedClient> {
    let arguments: Vec<&[u8]> = command_line
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .collect();
    let executable = arguments.first()?;
    let executable = Path::new(OsStr::from_bytes(executable));
    match executable.file_name()?.as_bytes() {
        b"codex-desktop" | b"Codex" => Some(UnattachedClient::Desktop),
        b"codex"
            if arguments.get(1) == Some(&b"app-server".as_slice())
                || arguments.iter().skip(1).any(|argument| {
                    *argument == b"--remote" || argument.starts_with(b"--remote=")
                }) =>
        {
            None
        }
        b"codex" => Some(UnattachedClient::Cli),
        _ => None,
    }
}
