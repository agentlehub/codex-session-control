use std::{
    collections::BTreeSet,
    fs,
    io::{IsTerminal, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    app_server::{AppServerClient, TESTED_CODEX_VERSION},
    desktop::{
        DescriptorState, DesktopAvailability, inspect_descriptor, publish_descriptor,
        render_descriptor, verify_persisted_desktop,
    },
    error::ControllerError,
    model::{InstalledRelease, ProductConfig, Thread, ThreadStatus},
};

use super::{
    CandidateRelease, DesktopAttachmentStatus, LifecycleContext, LifecycleReceipt,
    display_command_for_paths,
    evidence::{ResolvedUserPaths, SelectedHomeOperation, require_selected_home_evidence},
    native::{
        read_codex_version, reconcile_marketplace, reconcile_plugin, resolve_named_executable,
    },
    paths::{
        lifecycle_file_error, read_product_evidence_file, read_status_file, reconcile_file,
        resolve_codex_executable,
    },
    product_target,
    release::{
        ReleaseEndpoints, build_release_client, discover_latest_release, download_verified_release,
        production_release_endpoints, release_target_for_arch,
    },
    render::{reconcile_projection, render_projection, render_unit},
    service::{
        CallerUnitEvidence, CallerUnitInspection, LifecycleTarget, ServiceActivity,
        ServiceEnablement, inspect_caller_unit, query_service_activity, query_service_enablement,
        run_systemctl, verify_disabled_service, verify_enabled_service,
    },
    sha256_bytes,
    status::{StatusContext, status_with_context},
};

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalState {
    pub(super) stdin: bool,
    pub(super) stderr: bool,
    #[cfg(test)]
    pub(super) restart_prompt: Option<&'static ScriptedRestartPrompt>,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct ScriptedRestartPrompt {
    responses: std::sync::Mutex<std::collections::VecDeque<String>>,
    output: std::sync::Mutex<String>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct RestartPromptCapture(&'static ScriptedRestartPrompt);

#[cfg(test)]
impl RestartPromptCapture {
    pub(super) fn contents(self) -> String {
        self.0.output.lock().unwrap().clone()
    }
}

#[derive(Clone, Debug)]
pub(super) struct UpdateContext {
    pub(super) lifecycle: LifecycleContext,
    pub(super) candidate: PathBuf,
    pub(super) terminal: TerminalState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UpdateStage {
    ReleaseDiscovery,
    ReleaseDownload,
    Checksum,
    CandidatePreflight,
    ServiceSnapshot,
    RestartInspection,
    CandidateApply,
    ActiveTurnGate,
    Binary,
    Configuration,
    Projection,
    PluginMarketplace,
    PluginInstall,
    DesktopDiscovery,
    Descriptor,
    ServiceUnit,
    DaemonReload,
    ServiceApply,
    ServiceVerify,
    Manifest,
}

impl UpdateStage {
    const fn name(self) -> &'static str {
        match self {
            Self::ReleaseDiscovery => "release-discovery",
            Self::ReleaseDownload => "release-download",
            Self::Checksum => "checksum",
            Self::CandidatePreflight => "candidate-preflight",
            Self::ServiceSnapshot => "service-snapshot",
            Self::RestartInspection => "restart-inspection",
            Self::CandidateApply => "candidate-apply",
            Self::ActiveTurnGate => "active-turn-gate",
            Self::Binary => "binary",
            Self::Configuration => "configuration",
            Self::Projection => "projection",
            Self::PluginMarketplace => "plugin-marketplace",
            Self::PluginInstall => "plugin-install",
            Self::DesktopDiscovery => "desktop-discovery",
            Self::Descriptor => "descriptor",
            Self::ServiceUnit => "service-unit",
            Self::DaemonReload => "daemon-reload",
            Self::ServiceApply => "service-apply",
            Self::ServiceVerify => "service-verify",
            Self::Manifest => "manifest",
        }
    }
}

#[derive(Default)]
pub(super) struct UpdateProgress {
    completed: Vec<UpdateStage>,
}

impl UpdateProgress {
    fn complete(&mut self, stage: UpdateStage) {
        self.completed.push(stage);
    }

    fn stderr(&self) -> String {
        self.completed
            .iter()
            .map(|stage| format!("completed: {}\n", stage.name()))
            .collect()
    }

    fn fail(
        &self,
        stage: UpdateStage,
        cause: impl std::fmt::Display,
        recovery: &str,
    ) -> ControllerError {
        ControllerError::Operational(format!(
            "{}failed at {}: {cause}\n{recovery}",
            self.stderr(),
            stage.name()
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RestartReason {
    RunningCodexVersion,
    CodexExecutablePath,
    ServiceUnit,
}

impl RestartReason {
    const fn message(self) -> &'static str {
        match self {
            Self::RunningCodexVersion => "running Codex version differs",
            Self::CodexExecutablePath => "resolved Codex executable path differs",
            Self::ServiceUnit => "rendered systemd unit differs",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RestartInspection {
    ProvenUnchanged,
    ProvenChanged { reasons: Vec<RestartReason> },
    Unknown { evidence: String },
}

impl RestartInspection {
    fn reasons(&self) -> Option<&[RestartReason]> {
        match self {
            Self::ProvenChanged { reasons } => Some(reasons),
            Self::ProvenUnchanged | Self::Unknown { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ServiceSnapshot {
    enabled: bool,
    active: bool,
}

fn restart_reason_summary(reasons: &[RestartReason]) -> String {
    reasons
        .iter()
        .map(|reason| reason.message())
        .collect::<Vec<_>>()
        .join("; ")
}

fn self_hosted_update_refusal(reasons: &[RestartReason], display_command: &str) -> String {
    format!(
        "update requires restarting the managed app-server because {}. This command is running inside the managed app-server; run from an independent terminal: {display_command} update.\n",
        restart_reason_summary(reasons)
    )
}

fn unproven_update_caller_refusal(reasons: &[RestartReason], display_command: &str) -> String {
    format!(
        "update requires restarting the managed app-server because {}. A working systemctl --user whoami is required to prove this command is independent; repair or upgrade the systemd user environment, then run from an independent terminal: {display_command} update.\n",
        restart_reason_summary(reasons)
    )
}

fn guard_restart_required_update(
    systemctl: &Path,
    lifecycle: &LifecycleContext,
    snapshot: ServiceSnapshot,
    restart: &RestartInspection,
    display_command: &str,
    progress: &UpdateProgress,
) -> Result<(), ControllerError> {
    if snapshot.active
        && let Some(reasons) = restart.reasons()
    {
        match inspect_caller_unit(systemctl, &lifecycle.target) {
            CallerUnitInspection::Independent => {}
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::WhoAmI) => {
                return Err(progress.fail(
                    UpdateStage::RestartInspection,
                    self_hosted_update_refusal(reasons, display_command),
                    "",
                ));
            }
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::ControlGroup)
            | CallerUnitInspection::Unknown { .. } => {
                return Err(progress.fail(
                    UpdateStage::RestartInspection,
                    unproven_update_caller_refusal(reasons, display_command),
                    "",
                ));
            }
        }
    }
    Ok(())
}

impl TerminalState {
    fn production() -> Self {
        Self {
            stdin: std::io::stdin().is_terminal(),
            stderr: std::io::stderr().is_terminal(),
            #[cfg(test)]
            restart_prompt: None,
        }
    }

    #[cfg(test)]
    pub(super) fn noninteractive() -> Self {
        Self {
            stdin: false,
            stderr: false,
            restart_prompt: None,
        }
    }

    #[cfg(test)]
    pub(super) fn scripted<const N: usize>(responses: [&str; N]) -> (Self, RestartPromptCapture) {
        let prompt = Box::leak(Box::new(ScriptedRestartPrompt {
            responses: std::sync::Mutex::new(
                responses.into_iter().map(ToOwned::to_owned).collect(),
            ),
            output: std::sync::Mutex::new(String::new()),
        }));
        (
            Self {
                stdin: true,
                stderr: true,
                restart_prompt: Some(prompt),
            },
            RestartPromptCapture(prompt),
        )
    }
}

const STAGED_UPDATE_ENV: &str = "CODEX_SESSION_CONTROL_STAGED_UPDATE";

pub(crate) async fn update(
    target: LifecycleTarget,
    staged: bool,
) -> Result<LifecycleReceipt, ControllerError> {
    let lifecycle = super::lifecycle_context(target)?;
    if staged {
        let current = std::env::current_exe().map_err(|_| {
            ControllerError::Operational(
                "candidate-preflight cannot resolve current executable".to_owned(),
            )
        })?;
        if current == lifecycle.target.paths.binary {
            return Err(ControllerError::Operational(
                "candidate-preflight rejected staged marker on installed executable".to_owned(),
            ));
        }
        return staged_update_with_context(UpdateContext {
            lifecycle,
            candidate: current,
            terminal: TerminalState::production(),
        })
        .await;
    }
    outer_update(lifecycle).await
}

async fn outer_update(lifecycle: LifecycleContext) -> Result<LifecycleReceipt, ControllerError> {
    outer_update_with_endpoints(lifecycle, production_release_endpoints()).await
}

pub(super) async fn outer_update_with_endpoints(
    lifecycle: LifecycleContext,
    endpoints: ReleaseEndpoints,
) -> Result<LifecycleReceipt, ControllerError> {
    let mut progress = UpdateProgress::default();
    let paths = &lifecycle.target.paths;
    let display_command = display_command_for_paths(paths, &lifecycle.path_environment);
    let retry = format!("retry: {display_command} update\n");
    load_update_manifest(paths)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    let target = release_target_for_arch(std::env::consts::ARCH)
        .map_err(|error| progress.fail(UpdateStage::ReleaseDiscovery, error, &retry))?;
    let client = build_release_client()
        .map_err(|error| progress.fail(UpdateStage::ReleaseDiscovery, error, &retry))?;
    let release = discover_latest_release(&client, &endpoints, target)
        .await
        .map_err(|error| progress.fail(UpdateStage::ReleaseDiscovery, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::ReleaseDiscovery,
        lifecycle.target,
        &retry
    );

    let directory = tempfile::tempdir().map_err(|_| {
        progress.fail(
            UpdateStage::ReleaseDownload,
            "private release directory cannot be created",
            &retry,
        )
    })?;
    let downloaded = download_verified_release(&client, &release, directory.path())
        .await
        .map_err(|error| {
            let stage = if error.to_string().contains("checksum") {
                UpdateStage::Checksum
            } else {
                UpdateStage::ReleaseDownload
            };
            progress.fail(stage, error, &retry)
        })?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::ReleaseDownload,
        lifecycle.target,
        &retry
    );
    complete_lifecycle_stage!(progress, UpdateStage::Checksum, lifecycle.target, &retry);
    fs::set_permissions(&downloaded.binary_path, fs::Permissions::from_mode(0o700)).map_err(
        |_| {
            progress.fail(
                UpdateStage::CandidatePreflight,
                "candidate executable mode cannot be set",
                &retry,
            )
        },
    )?;
    let candidate = inspect_candidate(&downloaded.binary_path)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    if candidate.product_version != release.version.to_string()
        || candidate.target != release.target
    {
        return Err(progress.fail(
            UpdateStage::CandidatePreflight,
            "candidate identity differs from immutable release metadata",
            &retry,
        ));
    }
    complete_lifecycle_stage!(
        progress,
        UpdateStage::CandidatePreflight,
        lifecycle.target,
        &retry
    );

    outer_restart_preflight(&lifecycle, &candidate, &mut progress, &retry).await?;
    eprint!("{}", progress.stderr());
    run_candidate_apply(&candidate).map_err(|error| {
        ControllerError::Operational(format!("failed at candidate-apply: {error}\n{retry}"))
    })?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::CandidateApply,
        lifecycle.target,
        &retry
    );
    Ok(LifecycleReceipt {
        stdout: String::new(),
        stderr: "completed: candidate-apply\n".to_owned(),
    })
}

async fn outer_restart_preflight(
    lifecycle: &LifecycleContext,
    candidate: &CandidateRelease,
    progress: &mut UpdateProgress,
    retry: &str,
) -> Result<(), ControllerError> {
    let paths = &lifecycle.target.paths;
    let manifest = load_update_manifest(paths)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, retry))?;
    let installed = semver::Version::parse(&manifest.product_version).map_err(|_| {
        progress.fail(
            UpdateStage::CandidatePreflight,
            "installed manifest version is invalid",
            retry,
        )
    })?;
    let candidate_version = semver::Version::parse(&candidate.product_version).map_err(|_| {
        progress.fail(
            UpdateStage::CandidatePreflight,
            "candidate semantic version is invalid",
            retry,
        )
    })?;
    if candidate_version < installed {
        return Err(progress.fail(
            UpdateStage::CandidatePreflight,
            "candidate version is lower than installed release",
            retry,
        ));
    }
    let codex = resolve_codex_executable(&lifecycle.path_environment, &lifecycle.cwd)
        .map_err(|error| progress.fail(UpdateStage::RestartInspection, error, retry))?;
    let (_, expected_running_version) = read_codex_version(&codex, &paths.codex_home)
        .map_err(|error| progress.fail(UpdateStage::RestartInspection, error, retry))?;
    let desired_unit_sha256 = sha256_bytes(
        &render_unit(paths, &codex)
            .map_err(|error| progress.fail(UpdateStage::RestartInspection, error, retry))?,
    );
    let systemctl =
        resolve_named_executable(&lifecycle.path_environment, &lifecycle.cwd, "systemctl")
            .map_err(|error| progress.fail(UpdateStage::ServiceSnapshot, error, retry))?;
    let snapshot = service_snapshot(&systemctl, &lifecycle.target)
        .map_err(|error| progress.fail(UpdateStage::ServiceSnapshot, error, retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::ServiceSnapshot,
        lifecycle.target,
        retry
    );
    let restart = inspect_restart(
        paths,
        &manifest,
        snapshot,
        &codex,
        &expected_running_version,
        &desired_unit_sha256,
    )
    .await;
    if let RestartInspection::Unknown { evidence } = &restart {
        return Err(progress.fail(UpdateStage::RestartInspection, evidence, retry));
    }
    let display_command = display_command_for_paths(paths, &lifecycle.path_environment);
    guard_restart_required_update(
        &systemctl,
        lifecycle,
        snapshot,
        &restart,
        &display_command,
        progress,
    )?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::RestartInspection,
        lifecycle.target,
        retry
    );
    Ok(())
}

pub(super) async fn staged_update_with_context(
    context: UpdateContext,
) -> Result<LifecycleReceipt, ControllerError> {
    let mut progress = UpdateProgress::default();
    let lifecycle = &context.lifecycle;
    let paths = &lifecycle.target.paths;
    let display_command = display_command_for_paths(paths, &lifecycle.path_environment);
    let retry = format!("retry: {display_command} update\n");

    let candidate = inspect_candidate(&context.candidate)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    if candidate.target != product_target() {
        return Err(progress.fail(
            UpdateStage::CandidatePreflight,
            format!(
                "candidate target {} does not match selected target {}",
                candidate.target,
                product_target()
            ),
            &retry,
        ));
    }
    let candidate_bytes = fs::read(&candidate.executable).map_err(|_| {
        progress.fail(
            UpdateStage::CandidatePreflight,
            "candidate executable is unreadable",
            &retry,
        )
    })?;
    let candidate_sha256 = sha256_bytes(&candidate_bytes);
    let manifest = load_update_manifest(paths)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    let installed_version = semver::Version::parse(&manifest.product_version).map_err(|_| {
        progress.fail(
            UpdateStage::CandidatePreflight,
            "installed manifest version is invalid",
            &retry,
        )
    })?;
    let candidate_version = semver::Version::parse(&candidate.product_version).map_err(|_| {
        progress.fail(
            UpdateStage::CandidatePreflight,
            "candidate semantic version is invalid",
            &retry,
        )
    })?;
    if candidate_version < installed_version {
        return Err(progress.fail(
            UpdateStage::CandidatePreflight,
            format!(
                "candidate {} is lower than installed release {}",
                candidate.product_version, manifest.product_version
            ),
            &retry,
        ));
    }
    complete_lifecycle_stage!(
        progress,
        UpdateStage::CandidatePreflight,
        lifecycle.target,
        &retry
    );

    let codex = resolve_codex_executable(&lifecycle.path_environment, &lifecycle.cwd)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    let (codex_version, expected_running_version) =
        read_codex_version(&codex, &paths.codex_home)
            .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    let desired_config = ProductConfig {
        schema_version: 2,
        codex_executable: codex.clone(),
        codex_home: paths.codex_home.clone(),
        socket_path: paths.socket.clone(),
    };
    let desired_config_bytes = toml::to_string(&desired_config)
        .map(String::into_bytes)
        .map_err(|_| {
            progress.fail(
                UpdateStage::CandidatePreflight,
                "configuration cannot be rendered",
                &retry,
            )
        })?;
    let desired_projection = render_projection(&paths.binary, &candidate.product_version)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    let desired_unit = render_unit(paths, &codex)
        .map_err(|error| progress.fail(UpdateStage::CandidatePreflight, error, &retry))?;
    let desired_unit_sha256 = sha256_bytes(&desired_unit);
    let systemctl =
        resolve_named_executable(&lifecycle.path_environment, &lifecycle.cwd, "systemctl")
            .map_err(|error| progress.fail(UpdateStage::ServiceSnapshot, error, &retry))?;
    let snapshot = service_snapshot(&systemctl, &lifecycle.target)
        .map_err(|error| progress.fail(UpdateStage::ServiceSnapshot, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::ServiceSnapshot,
        lifecycle.target,
        &retry
    );

    let restart = inspect_restart(
        paths,
        &manifest,
        snapshot,
        &codex,
        &expected_running_version,
        &desired_unit_sha256,
    )
    .await;
    if let RestartInspection::Unknown { evidence } = &restart {
        let recovery = format!(
            "{display_command} disable\n\
{display_command} update\n\
{display_command} enable\n\
disable stops running turns; the final enable is needed only when the service should be enabled again.\n"
        );
        return Err(progress.fail(UpdateStage::RestartInspection, evidence, &recovery));
    }
    guard_restart_required_update(
        &systemctl,
        lifecycle,
        snapshot,
        &restart,
        &display_command,
        &progress,
    )?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::RestartInspection,
        lifecycle.target,
        &retry
    );

    if candidate_version == installed_version && candidate_sha256 == manifest.binary_sha256 {
        let status = status_with_context(StatusContext {
            target: lifecycle.target.clone(),
            path_environment: lifecycle.path_environment.clone(),
            desktop_environment: lifecycle.desktop_environment.clone(),
            cwd: lifecycle.cwd.clone(),
        })
        .await?;
        if status.healthy {
            return Ok(LifecycleReceipt {
                stdout: format!(
                    "Already current: {}\nDurable and running state: coherent\n",
                    candidate.product_version
                ),
                stderr: String::new(),
            });
        }
    }

    if snapshot.active && restart.reasons().is_some() {
        baseline_active_turn_gate(paths, &expected_running_version, context.terminal)
            .await
            .map_err(|error| progress.fail(UpdateStage::ActiveTurnGate, error, &retry))?;
        complete_lifecycle_stage!(
            progress,
            UpdateStage::ActiveTurnGate,
            lifecycle.target,
            &retry
        );
    }

    reconcile_file(&paths.binary, &candidate_bytes, 0o755, paths.euid)
        .map_err(|error| progress.fail(UpdateStage::Binary, error, &retry))?;
    complete_lifecycle_stage!(progress, UpdateStage::Binary, lifecycle.target, &retry);

    reconcile_file(&paths.config, &desired_config_bytes, 0o600, paths.euid)
        .map_err(|error| progress.fail(UpdateStage::Configuration, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::Configuration,
        lifecycle.target,
        &retry
    );

    let projection_changed = reconcile_projection(paths, &desired_projection)
        .map_err(|error| progress.fail(UpdateStage::Projection, error, &retry))?;
    complete_lifecycle_stage!(progress, UpdateStage::Projection, lifecycle.target, &retry);

    let marketplace_changed = reconcile_marketplace(&codex, &paths.codex_home, &paths.marketplace)
        .map_err(|error| progress.fail(UpdateStage::PluginMarketplace, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::PluginMarketplace,
        lifecycle.target,
        &retry
    );

    let plugin_changed = reconcile_plugin(
        &codex,
        &paths.codex_home,
        &paths.marketplace,
        &candidate.product_version,
    )
    .map_err(|error| progress.fail(UpdateStage::PluginInstall, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::PluginInstall,
        lifecycle.target,
        &retry
    );

    let (desktop_status, desktop_warning) = match manifest.desktop_attachment.as_ref() {
        None => (DesktopAttachmentStatus::Unavailable, None),
        Some(identity) => {
            match verify_persisted_desktop(identity, &lifecycle.desktop_environment)
                .await
                .map_err(|error| progress.fail(UpdateStage::DesktopDiscovery, error, &retry))?
            {
                DesktopAvailability::Verified(_) => (DesktopAttachmentStatus::Available, None),
                DesktopAvailability::Unavailable { warning } => {
                    (DesktopAttachmentStatus::Unverified, Some(warning))
                }
            }
        }
    };
    complete_lifecycle_stage!(
        progress,
        UpdateStage::DesktopDiscovery,
        lifecycle.target,
        &retry
    );

    let desktop_published = match manifest.desktop_attachment.as_ref() {
        Some(identity) => {
            let expected = render_descriptor(&paths.socket)
                .map_err(|error| progress.fail(UpdateStage::Descriptor, error, &retry))?;
            match inspect_descriptor(identity, &expected)
                .map_err(|error| progress.fail(UpdateStage::Descriptor, error, &retry))?
            {
                DescriptorState::Foreign => {
                    return Err(progress.fail(
                        UpdateStage::Descriptor,
                        "Desktop descriptor is foreign",
                        &retry,
                    ));
                }
                DescriptorState::Absent if snapshot.enabled => {
                    publish_descriptor(identity, &expected)
                        .map_err(|error| progress.fail(UpdateStage::Descriptor, error, &retry))?
                }
                DescriptorState::Absent | DescriptorState::Expected => false,
            }
        }
        None => false,
    };
    complete_lifecycle_stage!(progress, UpdateStage::Descriptor, lifecycle.target, &retry);

    reconcile_file(&paths.unit, &desired_unit, 0o644, paths.euid)
        .map_err(|error| progress.fail(UpdateStage::ServiceUnit, error, &retry))?;
    complete_lifecycle_stage!(progress, UpdateStage::ServiceUnit, lifecycle.target, &retry);

    run_systemctl(&systemctl, ["--user", "daemon-reload"])
        .map_err(|error| progress.fail(UpdateStage::DaemonReload, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UpdateStage::DaemonReload,
        lifecycle.target,
        &retry
    );

    match (snapshot, &restart) {
        (
            ServiceSnapshot {
                enabled: true,
                active: true,
            },
            RestartInspection::ProvenChanged { .. },
        ) => {
            run_systemctl(
                &systemctl,
                ["--user", "restart", lifecycle.target.unit_name.as_str()],
            )
            .map_err(|error| progress.fail(UpdateStage::ServiceApply, error, &retry))?;
        }
        (
            ServiceSnapshot {
                enabled: true,
                active: false,
            },
            _,
        ) => {
            run_systemctl(
                &systemctl,
                [
                    "--user",
                    "enable",
                    "--now",
                    lifecycle.target.unit_name.as_str(),
                ],
            )
            .map_err(|error| progress.fail(UpdateStage::ServiceApply, error, &retry))?;
        }
        _ => {}
    }
    complete_lifecycle_stage!(
        progress,
        UpdateStage::ServiceApply,
        lifecycle.target,
        &retry
    );

    if snapshot.enabled {
        verify_enabled_service(&systemctl, &lifecycle.target, &expected_running_version)
            .await
            .map_err(|error| progress.fail(UpdateStage::ServiceVerify, error, &retry))?;
    } else {
        verify_disabled_service(&systemctl, &lifecycle.target)
            .map_err(|error| progress.fail(UpdateStage::ServiceVerify, error, &retry))?;
    }
    complete_lifecycle_stage!(
        progress,
        UpdateStage::ServiceVerify,
        lifecycle.target,
        &retry
    );

    let installed = InstalledRelease {
        schema_version: 3,
        product_version: candidate.product_version.clone(),
        target: candidate.target.clone(),
        binary_sha256: candidate_sha256,
        service_unit_sha256: desired_unit_sha256,
        projection_sha256: desired_projection.sha256.clone(),
        plugin_version: candidate.product_version.clone(),
        codex_executable: codex,
        codex_home: paths.codex_home.clone(),
        socket_path: paths.socket.clone(),
        desktop_attachment: manifest.desktop_attachment.clone(),
    };
    let mut manifest_bytes = serde_json::to_vec_pretty(&installed).map_err(|_| {
        progress.fail(
            UpdateStage::Manifest,
            "installed manifest cannot be rendered",
            &retry,
        )
    })?;
    manifest_bytes.push(b'\n');
    reconcile_file(&paths.manifest, &manifest_bytes, 0o600, paths.euid)
        .map_err(|error| progress.fail(UpdateStage::Manifest, error, &retry))?;
    complete_lifecycle_stage!(progress, UpdateStage::Manifest, lifecycle.target, &retry);

    let projection_changed = projection_changed || marketplace_changed || plugin_changed;
    let service_state = if snapshot.enabled {
        "enabled, active"
    } else {
        "disabled, inactive"
    };
    let mut stderr = progress.stderr();
    if expected_running_version != TESTED_CODEX_VERSION {
        stderr.push_str(&format!(
            "Compatibility warning: Codex app-server {codex_version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
            env!("CARGO_PKG_VERSION")
        ));
    }
    if let Some(warning) = desktop_warning {
        stderr.push_str(&warning);
        stderr.push('\n');
    }
    Ok(LifecycleReceipt {
        stdout: format!(
            "Installed release: {version}\n\
Codex app-server service: {service_state}\n\
Plugin: codex-session-control {version} at {plugin}\n\
Codex home: {home}\n\
Desktop attachment: {desktop}\n\
Desktop restart required: {desktop_restart}\n\
Durable plugin state: current\n\
Loaded task state: {loaded}\n\
Codex client restart required: no\n\
New task required for guaranteed plugin convergence: yes\n",
            version = candidate.product_version,
            plugin = paths
                .marketplace
                .join("plugins/codex-session-control")
                .display(),
            home = paths.codex_home.display(),
            desktop = desktop_status.receipt(),
            desktop_restart = if desktop_published { "yes" } else { "no" },
            loaded = if projection_changed {
                "may_be_stale"
            } else {
                "not_verified"
            },
        ),
        stderr,
    })
}

fn inspect_candidate(path: &Path) -> Result<CandidateRelease, ControllerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ControllerError::Operational("candidate executable is missing or inaccessible".to_owned())
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o022 != 0
    {
        return Err(ControllerError::Operational(
            "candidate executable has unsafe owner, type, or mode".to_owned(),
        ));
    }
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| ControllerError::Operational("candidate version command failed".to_owned()))?;
    if !output.status.success() {
        return Err(ControllerError::Operational(
            "candidate version command failed".to_owned(),
        ));
    }
    let output = String::from_utf8(output.stdout).map_err(|_| {
        ControllerError::Operational("candidate version output is invalid".to_owned())
    })?;
    let output = output.strip_suffix('\n').ok_or_else(|| {
        ControllerError::Operational("candidate version output is invalid".to_owned())
    })?;
    if output.contains('\n') {
        return Err(ControllerError::Operational(
            "candidate version output is invalid".to_owned(),
        ));
    }
    let rest = output
        .strip_prefix("codex-session-control ")
        .ok_or_else(|| {
            ControllerError::Operational("candidate product name is invalid".to_owned())
        })?;
    let (version, target) = rest.rsplit_once(" (").ok_or_else(|| {
        ControllerError::Operational("candidate version output is invalid".to_owned())
    })?;
    let target = target.strip_suffix(')').ok_or_else(|| {
        ControllerError::Operational("candidate version output is invalid".to_owned())
    })?;
    let version = semver::Version::parse(version).map_err(|_| {
        ControllerError::Operational("candidate semantic version is invalid".to_owned())
    })?;
    if target.is_empty() || target.chars().any(char::is_whitespace) {
        return Err(ControllerError::Operational(
            "candidate target is invalid".to_owned(),
        ));
    }
    Ok(CandidateRelease {
        executable: path.to_path_buf(),
        product_version: version.to_string(),
        target: target.to_owned(),
    })
}

fn load_update_manifest(paths: &ResolvedUserPaths) -> Result<InstalledRelease, ControllerError> {
    let evidence = require_selected_home_evidence(paths, SelectedHomeOperation::Update)?;
    let expected = evidence
        .manifest
        .ok_or_else(|| ControllerError::Operational("installed manifest is invalid".to_owned()))?;
    let bytes = read_product_evidence_file(&paths.home, paths.euid, &paths.manifest, 0o600)
        .map_err(|error| {
            ControllerError::Operational(lifecycle_file_error(&paths.manifest, error))
        })?;
    let manifest = serde_json::from_slice::<InstalledRelease>(&bytes)
        .map_err(|_| ControllerError::Operational("installed manifest is invalid".to_owned()))?;
    manifest.validate(&paths.codex_home, &paths.socket)?;
    if manifest != expected {
        return Err(ControllerError::Operational(
            "installed manifest changed during validation".to_owned(),
        ));
    }
    if manifest.target != product_target() {
        return Err(ControllerError::Operational(
            "installed manifest target differs from this controller".to_owned(),
        ));
    }
    Ok(manifest)
}

fn service_snapshot(
    systemctl: &Path,
    target: &LifecycleTarget,
) -> Result<ServiceSnapshot, ControllerError> {
    match (
        query_service_enablement(systemctl, &target.unit_name),
        query_service_activity(systemctl, &target.unit_name),
    ) {
        (ServiceEnablement::Enabled, ServiceActivity::Active) => Ok(ServiceSnapshot {
            enabled: true,
            active: true,
        }),
        (ServiceEnablement::Enabled, ServiceActivity::Inactive) => Ok(ServiceSnapshot {
            enabled: true,
            active: false,
        }),
        (ServiceEnablement::Disabled | ServiceEnablement::Absent, ServiceActivity::Inactive) => {
            Ok(ServiceSnapshot {
                enabled: false,
                active: false,
            })
        }
        (ServiceEnablement::Unproven, _) => Err(ControllerError::Operational(
            "service enablement cannot be proven".to_owned(),
        )),
        (_, ServiceActivity::Unproven) => Err(ControllerError::Operational(
            "service activity cannot be proven".to_owned(),
        )),
        _ => Err(ControllerError::Operational(
            "inactive/enablement service state is contradictory".to_owned(),
        )),
    }
}

async fn inspect_restart(
    paths: &ResolvedUserPaths,
    manifest: &InstalledRelease,
    snapshot: ServiceSnapshot,
    desired_codex: &Path,
    expected_running_version: &str,
    desired_unit_sha256: &str,
) -> RestartInspection {
    if !snapshot.active {
        return RestartInspection::ProvenUnchanged;
    }
    let installed_unit = match read_status_file(&paths.unit, paths.euid, 0o644) {
        Ok(bytes) => bytes,
        Err(error) => {
            return RestartInspection::Unknown {
                evidence: lifecycle_file_error(&paths.unit, error),
            };
        }
    };
    if sha256_bytes(&installed_unit) != manifest.service_unit_sha256 {
        return RestartInspection::Unknown {
            evidence: "installed service unit does not match last coherent manifest".to_owned(),
        };
    }
    let installed_config =
        match read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600)
            .ok()
            .and_then(|bytes| {
                std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|text| toml::from_str::<ProductConfig>(text).ok())
            }) {
            Some(config) => config,
            None => {
                return RestartInspection::Unknown {
                    evidence: "installed configuration is unavailable or invalid".to_owned(),
                };
            }
        };
    if installed_config.codex_executable != manifest.codex_executable
        || installed_config.codex_home != manifest.codex_home
        || installed_config.socket_path != manifest.socket_path
    {
        return RestartInspection::Unknown {
            evidence: "installed configuration contradicts last coherent manifest".to_owned(),
        };
    }
    let client = AppServerClient::new(
        paths.socket.clone(),
        paths.codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        expected_running_version.to_owned(),
    );
    let connection = match client.connect_initialized().await {
        Ok(connection) => connection,
        Err(_) => {
            return RestartInspection::Unknown {
                evidence: "running app-server identity cannot be proven".to_owned(),
            };
        }
    };
    let mut reasons = Vec::new();
    if connection.compatibility_warning().is_some() {
        reasons.push(RestartReason::RunningCodexVersion);
    }
    if desired_codex != manifest.codex_executable {
        reasons.push(RestartReason::CodexExecutablePath);
    }
    if desired_unit_sha256 != manifest.service_unit_sha256 {
        reasons.push(RestartReason::ServiceUnit);
    }
    if reasons.is_empty() {
        RestartInspection::ProvenUnchanged
    } else {
        RestartInspection::ProvenChanged { reasons }
    }
}

pub(super) async fn baseline_active_turn_gate(
    paths: &ResolvedUserPaths,
    running_codex_version: &str,
    terminal: TerminalState,
) -> Result<(), ControllerError> {
    let mut disclosed = list_active_threads(paths, running_codex_version).await?;
    if !disclosed.is_empty() {
        require_restart_approval(&disclosed, terminal)?;
    }

    loop {
        let final_check = list_active_threads(paths, running_codex_version).await?;
        let disclosed_ids = disclosed
            .iter()
            .map(|thread| thread.id.as_str())
            .collect::<BTreeSet<_>>();
        if final_check
            .iter()
            .all(|thread| disclosed_ids.contains(thread.id.as_str()))
        {
            return Ok(());
        }
        require_restart_approval(&final_check, terminal)?;
        disclosed = final_check;
    }
}

fn require_restart_approval(
    active: &[Thread],
    terminal: TerminalState,
) -> Result<(), ControllerError> {
    if !(terminal.stdin && terminal.stderr) {
        return Err(ControllerError::Operational(
            "active tasks require interactive restart approval".to_owned(),
        ));
    }
    let prompt = active_restart_prompt(active);
    let response = restart_prompt_response(&prompt, terminal)?;
    if matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(ControllerError::Operational(
            "active task restart approval declined".to_owned(),
        ))
    }
}

fn active_restart_prompt(active: &[Thread]) -> String {
    let mut prompt = format!(
        "Codex session control must restart its app-server to install this update.\n\
\n\
This will interrupt {} active tasks:\n",
        active.len()
    );
    for thread in active {
        let title = thread
            .name
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("Untitled");
        prompt.push_str(&format!("- {title} ({})\n", thread.id));
    }
    prompt.push_str(
        "\n\
Their running turns will stop and be recorded as interrupted.\n\
\n\
Goals will not be paused or cleared. Restart alone will not continue them, but\n\
an active goal will start a new turn when a Codex client resumes its task.\n\
This can happen immediately if the task is already open.\n\
\n\
Pause any goal you do not want to continue before updating.\n\
\n\
Continue and interrupt active work? [y/N]",
    );
    prompt
}

fn restart_prompt_response(
    prompt: &str,
    _terminal: TerminalState,
) -> Result<String, ControllerError> {
    #[cfg(test)]
    if let Some(script) = _terminal.restart_prompt {
        script.output.lock().unwrap().push_str(prompt);
        return Ok(script
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default());
    }

    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(prompt.as_bytes())
        .and_then(|()| stderr.flush())
        .map_err(|_| {
            ControllerError::Operational("cannot write active task restart prompt".to_owned())
        })?;
    let mut response = String::new();
    std::io::stdin().read_line(&mut response).map_err(|_| {
        ControllerError::Operational("cannot read active task restart approval".to_owned())
    })?;
    Ok(response)
}

pub(super) async fn list_active_threads(
    paths: &ResolvedUserPaths,
    running_codex_version: &str,
) -> Result<Vec<Thread>, ControllerError> {
    let client = AppServerClient::new(
        paths.socket.clone(),
        paths.codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        super::normalized_codex_version(running_codex_version),
    );
    let mut connection = client
        .connect_initialized()
        .await
        .map_err(|_| ControllerError::Operational("active task inspection failed".to_owned()))?;
    let mut cursor = None;
    let mut active = Vec::new();
    loop {
        let (threads, next_cursor) = connection
            .threads_list_for_update(cursor.as_deref())
            .await
            .map_err(|_| {
                ControllerError::Operational("active task inspection failed".to_owned())
            })?;
        active.extend(
            threads
                .into_iter()
                .filter(|thread| matches!(thread.status, ThreadStatus::Active { .. })),
        );
        match next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(active)
}

pub(super) fn run_candidate_apply(candidate: &CandidateRelease) -> Result<(), ControllerError> {
    let status = Command::new(&candidate.executable)
        .arg("update")
        .env(STAGED_UPDATE_ENV, "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|_| ControllerError::Operational("candidate-apply execution failed".to_owned()))?;
    if status.success() {
        Ok(())
    } else {
        Err(ControllerError::Operational(
            "candidate-apply command failed".to_owned(),
        ))
    }
}
