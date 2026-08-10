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
    cli_output::{
        IndependentTerminal, ManagedPaths, OrdinaryFailure, RollbackIncomplete, RollbackPrimary,
        StopThenRetry, UpdateState, UpdateSuccess, UserFailure, UserNotice, UserSuccess,
    },
    desktop::{
        DescriptorPublicationFailure, DescriptorState, DesktopAvailability, inspect_descriptor,
        probe_persisted_desktop_capability, publish_descriptor, render_descriptor,
    },
    diagnostics::{DiagnosticEvent, Diagnostics},
    error::ControllerError,
    model::{InstalledRelease, ProductConfig, Thread, ThreadStatus},
};

use super::{
    CandidateRelease, LifecycleContext,
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
        ReleaseDownloadError, ReleaseEndpoints, build_release_client, discover_latest_release,
        download_verified_release, production_release_endpoints, release_target_for_arch,
    },
    render::{reconcile_projection, render_projection, render_unit},
    service::{
        CallerUnitInspection, LifecycleTarget, ServiceActivity, ServiceEnablement,
        inspect_caller_unit, query_service_activity, query_service_enablement, run_systemctl,
        verify_disabled_service, verify_enabled_service,
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
    failure: Option<RestartPromptTestFailure>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RestartPromptTestFailure {
    Write,
    Read,
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
pub(super) enum ActiveTurnGateFailure {
    Inspection,
    InteractiveRequired,
    Cancelled,
}

impl std::fmt::Display for ActiveTurnGateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Inspection => "active task inspection failed",
            Self::InteractiveRequired => "active tasks require interactive restart approval",
            Self::Cancelled => "active task restart approval declined",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CandidateApplyResult {
    Exit0,
    Exit1,
    SpawnFailed,
    CompletionUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateExit {
    Zero,
    One,
}

impl CandidateExit {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UpdateExecution {
    Render(UserSuccess),
    PropagateCandidateExit(CandidateExit),
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CandidateWaitHook {
    Real,
    FailAfterSuccessfulSpawn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateStage {
    ReleaseDiscovery,
    ReleaseDownload,
    Checksum,
    CandidatePreflight,
    ServiceSnapshot,
    RestartInspection,
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
    #[cfg(test)]
    const fn name(self) -> &'static str {
        match self {
            Self::ReleaseDiscovery => "release-discovery",
            Self::ReleaseDownload => "release-download",
            Self::Checksum => "checksum",
            Self::CandidatePreflight => "candidate-preflight",
            Self::ServiceSnapshot => "service-snapshot",
            Self::RestartInspection => "restart-inspection",
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

    fn completed_event(self) -> Option<DiagnosticEvent> {
        match self {
            Self::ReleaseDiscovery
            | Self::ReleaseDownload
            | Self::Checksum
            | Self::CandidatePreflight
            | Self::ServiceSnapshot
            | Self::RestartInspection
            | Self::ActiveTurnGate => None,
            Self::Binary => Some(DiagnosticEvent::CompletedBinary),
            Self::Configuration => Some(DiagnosticEvent::CompletedConfiguration),
            Self::Projection => Some(DiagnosticEvent::CompletedProjection),
            Self::PluginMarketplace => Some(DiagnosticEvent::CompletedPluginMarketplace),
            Self::PluginInstall => Some(DiagnosticEvent::CompletedPluginInstall),
            Self::DesktopDiscovery => Some(DiagnosticEvent::CompletedDesktopDiscovery),
            Self::Descriptor => Some(DiagnosticEvent::CompletedDescriptor),
            Self::ServiceUnit => Some(DiagnosticEvent::CompletedServiceUnit),
            Self::DaemonReload => Some(DiagnosticEvent::CompletedDaemonReload),
            Self::ServiceApply => None,
            Self::ServiceVerify => Some(DiagnosticEvent::CompletedServiceVerify),
            Self::Manifest => Some(DiagnosticEvent::CompletedManifest),
        }
    }

    fn failed_event(self, cause: crate::diagnostics::DiagnosticCause) -> DiagnosticEvent {
        match self {
            Self::ReleaseDiscovery
            | Self::ReleaseDownload
            | Self::Checksum
            | Self::CandidatePreflight
            | Self::ServiceSnapshot
            | Self::RestartInspection
            | Self::ActiveTurnGate => DiagnosticEvent::FailedPreflight(cause),
            Self::Binary => DiagnosticEvent::FailedBinary(cause),
            Self::Configuration => DiagnosticEvent::FailedConfiguration(cause),
            Self::Projection => DiagnosticEvent::FailedProjection(cause),
            Self::PluginMarketplace => DiagnosticEvent::FailedPluginMarketplace(cause),
            Self::PluginInstall => DiagnosticEvent::FailedPluginInstall(cause),
            Self::DesktopDiscovery => DiagnosticEvent::FailedDesktopDiscovery(cause),
            Self::Descriptor => DiagnosticEvent::FailedDescriptor(cause),
            Self::ServiceUnit => DiagnosticEvent::FailedServiceUnit(cause),
            Self::DaemonReload => DiagnosticEvent::FailedDaemonReload(cause),
            Self::ServiceApply => DiagnosticEvent::FailedServiceEnable(cause),
            Self::ServiceVerify => DiagnosticEvent::FailedServiceVerify(cause),
            Self::Manifest => DiagnosticEvent::FailedManifest(cause),
        }
    }
}

fn update_failed(
    diagnostics: &mut Diagnostics,
    stage: UpdateStage,
    cause: crate::diagnostics::DiagnosticCause,
    failure: UserFailure,
) -> UserFailure {
    diagnostics.emit(stage.failed_event(cause));
    failure
}

fn complete_update_stage(
    diagnostics: &mut Diagnostics,
    stage: UpdateStage,
    _target: &LifecycleTarget,
    injected_failure: UserFailure,
) -> Result<(), UserFailure> {
    if let Some(event) = stage.completed_event() {
        diagnostics.emit(event);
    }
    #[cfg(test)]
    if _target.test_hooks.fail_after_completed_stage == Some(stage.name()) {
        return Err(update_failed(
            diagnostics,
            stage,
            crate::diagnostics::DiagnosticCause::Unexpected,
            injected_failure,
        ));
    }
    #[cfg(not(test))]
    let _ = injected_failure;
    Ok(())
}

pub(super) fn active_turn_gate_failure(failure: ActiveTurnGateFailure) -> UserFailure {
    match failure {
        ActiveTurnGateFailure::Inspection => {
            UserFailure::Ordinary(OrdinaryFailure::UpdateActiveTasksRetry)
        }
        ActiveTurnGateFailure::InteractiveRequired => UserFailure::InteractiveTerminal,
        ActiveTurnGateFailure::Cancelled => UserFailure::Cancellation,
    }
}

pub(super) fn update_cli_reconciliation_failure(error: &ControllerError) -> UserFailure {
    UserFailure::Ordinary(match error {
        ControllerError::InvalidData { .. } => OrdinaryFailure::UpdateCliIntegrationCheckStatus,
        ControllerError::Operational(_) => OrdinaryFailure::UpdateCliIntegrationRetry,
    })
}

pub(super) fn update_descriptor_publication_failure(
    failure: DescriptorPublicationFailure,
) -> UserFailure {
    let _ = &failure.source;
    match failure.residue {
        None => UserFailure::Ordinary(OrdinaryFailure::UpdateDesktopIntegrationRetry),
        Some(residue) => UserFailure::RollbackIncomplete(RollbackIncomplete::new(
            RollbackPrimary::UpdateDesktopRetry,
            ManagedPaths::new(residue.into_path(), Vec::new()),
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RestartReason {
    RunningCodexVersion,
    CodexExecutablePath,
    ServiceUnit,
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

fn guard_restart_required_update_typed(
    systemctl: &Path,
    lifecycle: &LifecycleContext,
    snapshot: ServiceSnapshot,
    restart: &RestartInspection,
) -> Result<(), UserFailure> {
    if snapshot.active && restart.reasons().is_some() {
        match inspect_caller_unit(systemctl, &lifecycle.target) {
            CallerUnitInspection::Independent => {}
            CallerUnitInspection::SelfHosted(_) | CallerUnitInspection::Unknown { .. } => {
                return Err(UserFailure::IndependentTerminal(
                    IndependentTerminal::Update,
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
            failure: None,
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

    #[cfg(test)]
    pub(super) fn scripted_prompt_failure(
        failure: RestartPromptTestFailure,
    ) -> (Self, RestartPromptCapture) {
        let prompt = Box::leak(Box::new(ScriptedRestartPrompt {
            responses: std::sync::Mutex::new(std::collections::VecDeque::new()),
            output: std::sync::Mutex::new(String::new()),
            failure: Some(failure),
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
    verbose: bool,
    diagnostics: &mut Diagnostics,
) -> Result<UpdateExecution, UserFailure> {
    use crate::diagnostics::{DiagnosticCause, UpdatePhase};

    let lifecycle = super::lifecycle_context(target).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Unexpected,
            UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry),
        )
    })?;
    if staged {
        diagnostics.set_phase(UpdatePhase::Apply);
        diagnostics.emit(DiagnosticEvent::StagedMarkerAccepted);
        let current = std::env::current_exe().map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::Unexpected,
                UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry),
            )
        })?;
        if current == lifecycle.target.paths.binary {
            return Err(update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::Unexpected,
                UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry),
            ));
        }
        return staged_update_with_context_and_diagnostics(
            UpdateContext {
                lifecycle,
                candidate: current,
                terminal: TerminalState::production(),
            },
            diagnostics,
        )
        .await
        .map(UpdateExecution::Render);
    }
    diagnostics.set_phase(UpdatePhase::Outer);
    outer_update(lifecycle, verbose, diagnostics).await
}

async fn outer_update(
    lifecycle: LifecycleContext,
    verbose: bool,
    diagnostics: &mut Diagnostics,
) -> Result<UpdateExecution, UserFailure> {
    outer_update_with_endpoints_and_diagnostics(
        lifecycle,
        production_release_endpoints(),
        verbose,
        diagnostics,
    )
    .await
}

#[cfg(test)]
pub(super) async fn outer_update_with_endpoints(
    lifecycle: LifecycleContext,
    endpoints: ReleaseEndpoints,
) -> Result<UpdateExecution, UserFailure> {
    let mut diagnostics = Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Update);
    diagnostics.set_phase(crate::diagnostics::UpdatePhase::Outer);
    outer_update_with_endpoints_and_diagnostics(lifecycle, endpoints, false, &mut diagnostics).await
}

pub(super) async fn outer_update_with_endpoints_and_diagnostics(
    lifecycle: LifecycleContext,
    endpoints: ReleaseEndpoints,
    verbose: bool,
    diagnostics: &mut Diagnostics,
) -> Result<UpdateExecution, UserFailure> {
    use crate::diagnostics::DiagnosticCause;

    let paths = &lifecycle.target.paths;
    load_update_manifest(paths).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStateCheckStatus),
        )
    })?;
    let release_failure = || UserFailure::Ordinary(OrdinaryFailure::UpdateReleaseRetry);
    let checksum_failure = || UserFailure::Ordinary(OrdinaryFailure::UpdateChecksumRetry);
    let target = release_target_for_arch(std::env::consts::ARCH).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::ReleaseDiscovery,
            DiagnosticCause::ReleaseDownload,
            release_failure(),
        )
    })?;
    let client = build_release_client().map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::ReleaseDiscovery,
            DiagnosticCause::ReleaseDownload,
            release_failure(),
        )
    })?;
    let release = discover_latest_release(&client, &endpoints, target)
        .await
        .map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::ReleaseDiscovery,
                DiagnosticCause::ReleaseDownload,
                release_failure(),
            )
        })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::ReleaseDiscovery,
        &lifecycle.target,
        release_failure(),
    )?;

    let directory = tempfile::tempdir().map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::ReleaseDownload,
            DiagnosticCause::ReleaseDownload,
            release_failure(),
        )
    })?;
    let downloaded = download_verified_release(&client, &release, directory.path())
        .await
        .map_err(|error| {
            let (stage, cause, failure) = match &error {
                ReleaseDownloadError::Download(_) => (
                    UpdateStage::ReleaseDownload,
                    DiagnosticCause::ReleaseDownload,
                    release_failure(),
                ),
                ReleaseDownloadError::Integrity(_) => (
                    UpdateStage::Checksum,
                    DiagnosticCause::Checksum,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateChecksumRetry),
                ),
            };
            update_failed(diagnostics, stage, cause, failure)
        })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::ReleaseDownload,
        &lifecycle.target,
        release_failure(),
    )?;
    complete_update_stage(
        diagnostics,
        UpdateStage::Checksum,
        &lifecycle.target,
        checksum_failure(),
    )?;
    fs::set_permissions(&downloaded.binary_path, fs::Permissions::from_mode(0o700)).map_err(
        |_| {
            update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::Checksum,
                checksum_failure(),
            )
        },
    )?;
    let candidate = inspect_candidate(&downloaded.binary_path).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        )
    })?;
    if candidate.product_version != release.version.to_string()
        || candidate.target != release.target
    {
        return Err(update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        ));
    }
    diagnostics.emit(DiagnosticEvent::CandidateVerified {
        version: release.version,
    });
    complete_update_stage(
        diagnostics,
        UpdateStage::CandidatePreflight,
        &lifecycle.target,
        checksum_failure(),
    )?;

    outer_restart_preflight(&lifecycle, &candidate, diagnostics).await?;
    let exit = run_outer_candidate(&candidate, verbose, diagnostics)?;
    Ok(UpdateExecution::PropagateCandidateExit(exit))
}

async fn outer_restart_preflight(
    lifecycle: &LifecycleContext,
    candidate: &CandidateRelease,
    diagnostics: &mut Diagnostics,
) -> Result<(), UserFailure> {
    use crate::diagnostics::DiagnosticCause;

    let paths = &lifecycle.target.paths;
    let manifest = load_update_manifest(paths).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStateCheckStatus),
        )
    })?;
    let installed = semver::Version::parse(&manifest.product_version).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStateCheckStatus),
        )
    })?;
    let candidate_version = semver::Version::parse(&candidate.product_version).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            UserFailure::Ordinary(OrdinaryFailure::UpdateChecksumRetry),
        )
    })?;
    if candidate_version < installed {
        return Err(update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            UserFailure::Ordinary(OrdinaryFailure::UpdateChecksumRetry),
        ));
    }
    let codex =
        resolve_codex_executable(&lifecycle.path_environment, &lifecycle.cwd).map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::RestartInspection,
                DiagnosticCause::CliIntegration,
                UserFailure::Ordinary(OrdinaryFailure::UpdateCliIntegrationRetry),
            )
        })?;
    let (_, expected_running_version) =
        read_codex_version(&codex, &paths.codex_home).map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::RestartInspection,
                DiagnosticCause::CliIntegration,
                UserFailure::Ordinary(OrdinaryFailure::UpdateCliIntegrationRetry),
            )
        })?;
    let desired_unit_sha256 = sha256_bytes(&render_unit(paths, &codex).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::RestartInspection,
            DiagnosticCause::ServiceConfiguration,
            UserFailure::Ordinary(OrdinaryFailure::UpdateServiceConfigurationRetry),
        )
    })?);
    let systemctl =
        resolve_named_executable(&lifecycle.path_environment, &lifecycle.cwd, "systemctl")
            .map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::ServiceSnapshot,
                    DiagnosticCause::ServiceState,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateCheckStatus),
                )
            })?;
    let snapshot = service_snapshot(&systemctl, &lifecycle.target).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::ServiceSnapshot,
            DiagnosticCause::ServiceState,
            UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateCheckStatus),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::ServiceSnapshot,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateCheckStatus),
    )?;
    let restart = inspect_restart(
        paths,
        &manifest,
        snapshot,
        &codex,
        &expected_running_version,
        &desired_unit_sha256,
    )
    .await;
    if matches!(restart, RestartInspection::Unknown { .. }) {
        return Err(update_failed(
            diagnostics,
            UpdateStage::RestartInspection,
            DiagnosticCause::ServiceState,
            UserFailure::StopThenRetry(StopThenRetry::UpdateServiceStateDisableUpdateEnable),
        ));
    }
    guard_restart_required_update_typed(&systemctl, lifecycle, snapshot, &restart).map_err(
        |failure| {
            update_failed(
                diagnostics,
                UpdateStage::RestartInspection,
                DiagnosticCause::ServiceState,
                failure,
            )
        },
    )?;
    complete_update_stage(
        diagnostics,
        UpdateStage::RestartInspection,
        &lifecycle.target,
        UserFailure::StopThenRetry(StopThenRetry::UpdateServiceStateDisableUpdateEnable),
    )?;
    Ok(())
}

#[cfg(test)]
pub(super) async fn staged_update_with_context(
    context: UpdateContext,
) -> Result<crate::cli_output::RenderedCli, UserFailure> {
    let mut diagnostics = Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Update);
    diagnostics.set_phase(crate::diagnostics::UpdatePhase::Apply);
    diagnostics.emit(DiagnosticEvent::StagedMarkerAccepted);
    staged_update_with_context_and_diagnostics(context, &mut diagnostics)
        .await
        .map(|success| success.render())
}

pub(super) async fn staged_update_with_context_and_diagnostics(
    context: UpdateContext,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    use crate::diagnostics::DiagnosticCause;

    let lifecycle = &context.lifecycle;
    let paths = &lifecycle.target.paths;
    let checksum_failure = || UserFailure::Ordinary(OrdinaryFailure::UpdateChecksumRetry);
    let installed_state_failure =
        || UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStateCheckStatus);
    let cli_failure = || UserFailure::Ordinary(OrdinaryFailure::UpdateCliIntegrationRetry);
    let service_configuration_failure =
        || UserFailure::Ordinary(OrdinaryFailure::UpdateServiceConfigurationRetry);
    let service_state_failure =
        || UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateCheckStatus);

    let candidate = inspect_candidate(&context.candidate).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        )
    })?;
    if candidate.target != product_target() {
        return Err(update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        ));
    }
    let candidate_bytes = fs::read(&candidate.executable).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        )
    })?;
    let candidate_sha256 = sha256_bytes(&candidate_bytes);
    let manifest = load_update_manifest(paths).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Validation,
            installed_state_failure(),
        )
    })?;
    let installed_version = semver::Version::parse(&manifest.product_version).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Validation,
            installed_state_failure(),
        )
    })?;
    let candidate_version = semver::Version::parse(&candidate.product_version).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        )
    })?;
    if candidate_version < installed_version {
        return Err(update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::Checksum,
            checksum_failure(),
        ));
    }
    diagnostics.emit(DiagnosticEvent::CandidateVerified {
        version: candidate_version.clone(),
    });
    complete_update_stage(
        diagnostics,
        UpdateStage::CandidatePreflight,
        &lifecycle.target,
        checksum_failure(),
    )?;

    let codex =
        resolve_codex_executable(&lifecycle.path_environment, &lifecycle.cwd).map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::CliIntegration,
                cli_failure(),
            )
        })?;
    let (codex_version, expected_running_version) = read_codex_version(&codex, &paths.codex_home)
        .map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::CliIntegration,
            cli_failure(),
        )
    })?;
    let desired_config = ProductConfig {
        schema_version: 2,
        codex_executable: codex.clone(),
        codex_home: paths.codex_home.clone(),
        socket_path: paths.socket.clone(),
    };
    let desired_config_bytes = toml::to_string(&desired_config)
        .map(String::into_bytes)
        .map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::CliIntegration,
                cli_failure(),
            )
        })?;
    let desired_projection =
        render_projection(&paths.binary, &candidate.product_version).map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::CliIntegration,
                cli_failure(),
            )
        })?;
    let desired_unit = render_unit(paths, &codex).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::CandidatePreflight,
            DiagnosticCause::ServiceConfiguration,
            service_configuration_failure(),
        )
    })?;
    let desired_unit_sha256 = sha256_bytes(&desired_unit);
    let systemctl =
        resolve_named_executable(&lifecycle.path_environment, &lifecycle.cwd, "systemctl")
            .map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::ServiceSnapshot,
                    DiagnosticCause::ServiceState,
                    service_state_failure(),
                )
            })?;
    let snapshot = service_snapshot(&systemctl, &lifecycle.target).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::ServiceSnapshot,
            DiagnosticCause::ServiceState,
            service_state_failure(),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::ServiceSnapshot,
        &lifecycle.target,
        service_state_failure(),
    )?;

    let restart = inspect_restart(
        paths,
        &manifest,
        snapshot,
        &codex,
        &expected_running_version,
        &desired_unit_sha256,
    )
    .await;
    if matches!(restart, RestartInspection::Unknown { .. }) {
        return Err(update_failed(
            diagnostics,
            UpdateStage::RestartInspection,
            DiagnosticCause::ServiceState,
            UserFailure::StopThenRetry(StopThenRetry::UpdateServiceStateDisableUpdateEnable),
        ));
    }
    guard_restart_required_update_typed(&systemctl, lifecycle, snapshot, &restart).map_err(
        |failure| {
            update_failed(
                diagnostics,
                UpdateStage::RestartInspection,
                DiagnosticCause::ServiceState,
                failure,
            )
        },
    )?;
    complete_update_stage(
        diagnostics,
        UpdateStage::RestartInspection,
        &lifecycle.target,
        UserFailure::StopThenRetry(StopThenRetry::UpdateServiceStateDisableUpdateEnable),
    )?;

    if candidate_version == installed_version && candidate_sha256 == manifest.binary_sha256 {
        let status = status_with_context(StatusContext {
            target: lifecycle.target.clone(),
            path_environment: lifecycle.path_environment.clone(),
            desktop_environment: lifecycle.desktop_environment.clone(),
            cwd: lifecycle.cwd.clone(),
        })
        .await
        .map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::CandidatePreflight,
                DiagnosticCause::Validation,
                installed_state_failure(),
            )
        })?;
        if status.healthy {
            return Ok(UserSuccess::Update(UpdateSuccess::new(
                UpdateState::AlreadyCurrent,
                candidate_version,
                snapshot.enabled,
                false,
                Vec::new(),
            )));
        }
    }

    if snapshot.active && restart.reasons().is_some() {
        baseline_active_turn_gate(paths, &expected_running_version, context.terminal)
            .await
            .map_err(|failure| {
                update_failed(
                    diagnostics,
                    UpdateStage::ActiveTurnGate,
                    DiagnosticCause::ActiveTasks,
                    active_turn_gate_failure(failure),
                )
            })?;
        complete_update_stage(
            diagnostics,
            UpdateStage::ActiveTurnGate,
            &lifecycle.target,
            UserFailure::Ordinary(OrdinaryFailure::UpdateActiveTasksRetry),
        )?;
    }

    reconcile_file(&paths.binary, &candidate_bytes, 0o755, paths.euid).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::Binary,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstallationFilesRetry),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::Binary,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateInstallationFilesRetry),
    )?;

    reconcile_file(&paths.config, &desired_config_bytes, 0o600, paths.euid).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::Configuration,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstallationFilesRetry),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::Configuration,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateInstallationFilesRetry),
    )?;

    let projection_changed = reconcile_projection(paths, &desired_projection).map_err(|error| {
        let failure = update_cli_reconciliation_failure(&error);
        update_failed(
            diagnostics,
            UpdateStage::Projection,
            DiagnosticCause::CliIntegration,
            failure,
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::Projection,
        &lifecycle.target,
        cli_failure(),
    )?;

    let marketplace_changed = reconcile_marketplace(&codex, &paths.codex_home, &paths.marketplace)
        .map_err(|error| {
            let failure = update_cli_reconciliation_failure(&error);
            update_failed(
                diagnostics,
                UpdateStage::PluginMarketplace,
                DiagnosticCause::CliIntegration,
                failure,
            )
        })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::PluginMarketplace,
        &lifecycle.target,
        cli_failure(),
    )?;

    let plugin_changed = reconcile_plugin(
        &codex,
        &paths.codex_home,
        &paths.marketplace,
        &candidate.product_version,
    )
    .map_err(|error| {
        let failure = update_cli_reconciliation_failure(&error);
        update_failed(
            diagnostics,
            UpdateStage::PluginInstall,
            DiagnosticCause::CliIntegration,
            failure,
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::PluginInstall,
        &lifecycle.target,
        cli_failure(),
    )?;

    let desktop_warning = match manifest.desktop_attachment.as_ref() {
        None => None,
        Some(identity) => {
            match probe_persisted_desktop_capability(identity, &lifecycle.desktop_environment)
                .await
                .map_err(|_| {
                    update_failed(
                        diagnostics,
                        UpdateStage::DesktopDiscovery,
                        DiagnosticCause::DesktopIntegration,
                        UserFailure::Ordinary(OrdinaryFailure::UpdateDesktopIntegrationCheckStatus),
                    )
                })? {
                DesktopAvailability::Verified(_) => None,
                DesktopAvailability::Unavailable { warning } => Some(warning),
            }
        }
    };
    complete_update_stage(
        diagnostics,
        UpdateStage::DesktopDiscovery,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry),
    )?;

    let desktop_published = match manifest.desktop_attachment.as_ref() {
        Some(identity) => {
            let expected = render_descriptor(&paths.socket).map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::Descriptor,
                    DiagnosticCause::DesktopIntegration,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateDesktopIntegrationCheckStatus),
                )
            })?;
            match inspect_descriptor(identity, &expected).map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::Descriptor,
                    DiagnosticCause::DesktopIntegration,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateDesktopIntegrationCheckStatus),
                )
            })? {
                DescriptorState::Foreign => {
                    return Err(update_failed(
                        diagnostics,
                        UpdateStage::Descriptor,
                        DiagnosticCause::DesktopIntegration,
                        UserFailure::Ordinary(OrdinaryFailure::UpdateDesktopIntegrationCheckStatus),
                    ));
                }
                DescriptorState::Absent if snapshot.enabled => {
                    publish_descriptor(identity, &expected).map_err(|failure| {
                        let failure = update_descriptor_publication_failure(failure);
                        update_failed(
                            diagnostics,
                            UpdateStage::Descriptor,
                            DiagnosticCause::DesktopIntegration,
                            failure,
                        )
                    })?
                }
                DescriptorState::Absent | DescriptorState::Expected => false,
            }
        }
        None => false,
    };
    complete_update_stage(
        diagnostics,
        UpdateStage::Descriptor,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateUnexpectedRetry),
    )?;

    reconcile_file(&paths.unit, &desired_unit, 0o644, paths.euid).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::ServiceUnit,
            DiagnosticCause::ServiceConfiguration,
            UserFailure::Ordinary(OrdinaryFailure::UpdateServiceConfigurationLogs),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::ServiceUnit,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateServiceConfigurationLogs),
    )?;

    run_systemctl(&systemctl, ["--user", "daemon-reload"]).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::DaemonReload,
            DiagnosticCause::ServiceConfiguration,
            UserFailure::Ordinary(OrdinaryFailure::UpdateServiceConfigurationLogs),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::DaemonReload,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateServiceConfigurationLogs),
    )?;

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
            .map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::ServiceApply,
                    DiagnosticCause::ServiceStart,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStartLogs),
                )
            })?;
            diagnostics.emit(DiagnosticEvent::CompletedServiceRestart);
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
            .map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::ServiceApply,
                    DiagnosticCause::ServiceStart,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStartLogs),
                )
            })?;
            diagnostics.emit(DiagnosticEvent::CompletedServiceEnable);
        }
        _ => {}
    }
    complete_update_stage(
        diagnostics,
        UpdateStage::ServiceApply,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStartLogs),
    )?;

    if snapshot.enabled {
        verify_enabled_service(&systemctl, &lifecycle.target, &expected_running_version)
            .await
            .map_err(|_| {
                update_failed(
                    diagnostics,
                    UpdateStage::ServiceVerify,
                    DiagnosticCause::ServiceState,
                    UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateLogs),
                )
            })?;
    } else {
        verify_disabled_service(&systemctl, &lifecycle.target).map_err(|_| {
            update_failed(
                diagnostics,
                UpdateStage::ServiceVerify,
                DiagnosticCause::ServiceState,
                UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateLogs),
            )
        })?;
    }
    complete_update_stage(
        diagnostics,
        UpdateStage::ServiceVerify,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateServiceStateLogs),
    )?;

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
        update_failed(
            diagnostics,
            UpdateStage::Manifest,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStatePostMutationCheckStatus),
        )
    })?;
    manifest_bytes.push(b'\n');
    reconcile_file(&paths.manifest, &manifest_bytes, 0o600, paths.euid).map_err(|_| {
        update_failed(
            diagnostics,
            UpdateStage::Manifest,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStatePostMutationCheckStatus),
        )
    })?;
    complete_update_stage(
        diagnostics,
        UpdateStage::Manifest,
        &lifecycle.target,
        UserFailure::Ordinary(OrdinaryFailure::UpdateInstalledStatePostMutationCheckStatus),
    )?;

    let mut notices = Vec::new();
    if expected_running_version != TESTED_CODEX_VERSION {
        notices.push(UserNotice::Compatibility {
            codex: semver::Version::parse(&codex_version).unwrap_or_else(|_| {
                semver::Version::parse(super::UNKNOWN_CODEX_VERSION)
                    .expect("unknown Codex version sentinel is semantic")
            }),
            product: candidate_version.clone(),
        });
    }
    if desktop_warning.is_some() {
        notices.push(UserNotice::DesktopLauncherUnavailable);
    }
    let _projection_changed = projection_changed || marketplace_changed || plugin_changed;
    Ok(UserSuccess::Update(UpdateSuccess::new(
        UpdateState::Applied,
        candidate_version,
        snapshot.enabled,
        desktop_published,
        notices,
    )))
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
) -> Result<(), ActiveTurnGateFailure> {
    let mut disclosed = list_active_threads(paths, running_codex_version)
        .await
        .map_err(|_| ActiveTurnGateFailure::Inspection)?;
    if !disclosed.is_empty() {
        require_restart_approval(&disclosed, terminal)?;
    }

    loop {
        let final_check = list_active_threads(paths, running_codex_version)
            .await
            .map_err(|_| ActiveTurnGateFailure::Inspection)?;
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
) -> Result<(), ActiveTurnGateFailure> {
    if !(terminal.stdin && terminal.stderr) {
        return Err(ActiveTurnGateFailure::InteractiveRequired);
    }
    let prompt = active_restart_prompt(active);
    let response = restart_prompt_response(&prompt, terminal)?;
    if matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(ActiveTurnGateFailure::Cancelled)
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
) -> Result<String, ActiveTurnGateFailure> {
    #[cfg(test)]
    if let Some(script) = _terminal.restart_prompt {
        if script.failure == Some(RestartPromptTestFailure::Write) {
            return Err(ActiveTurnGateFailure::InteractiveRequired);
        }
        script.output.lock().unwrap().push_str(prompt);
        if script.failure == Some(RestartPromptTestFailure::Read) {
            return Err(ActiveTurnGateFailure::InteractiveRequired);
        }
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
        .map_err(|_| ActiveTurnGateFailure::InteractiveRequired)?;
    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .map_err(|_| ActiveTurnGateFailure::InteractiveRequired)?;
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

pub(super) fn classify_candidate_wait(
    status: Result<std::process::ExitStatus, std::io::Error>,
) -> CandidateApplyResult {
    match status {
        Ok(status) if status.code() == Some(0) => CandidateApplyResult::Exit0,
        Ok(status) if status.code() == Some(1) => CandidateApplyResult::Exit1,
        Ok(_) | Err(_) => CandidateApplyResult::CompletionUnknown,
    }
}

fn spawn_candidate_apply(
    candidate: &CandidateRelease,
    verbose: bool,
) -> Result<std::process::Child, std::io::Error> {
    let mut command = Command::new(&candidate.executable);
    if verbose {
        command.arg("--verbose");
    }
    command
        .arg("update")
        .env(STAGED_UPDATE_ENV, "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

pub(super) fn run_candidate_apply(
    candidate: &CandidateRelease,
    verbose: bool,
) -> CandidateApplyResult {
    let Ok(mut child) = spawn_candidate_apply(candidate, verbose) else {
        return CandidateApplyResult::SpawnFailed;
    };
    classify_candidate_wait(child.wait())
}

#[cfg(test)]
pub(super) fn run_candidate_apply_with_wait_hook(
    candidate: &CandidateRelease,
    verbose: bool,
    hook: CandidateWaitHook,
) -> CandidateApplyResult {
    let Ok(mut child) = spawn_candidate_apply(candidate, verbose) else {
        return CandidateApplyResult::SpawnFailed;
    };
    match hook {
        CandidateWaitHook::Real => classify_candidate_wait(child.wait()),
        CandidateWaitHook::FailAfterSuccessfulSpawn => {
            let _ = child.wait();
            classify_candidate_wait(Err(std::io::Error::other("injected wait failure")))
        }
    }
}

pub(super) fn run_outer_candidate(
    candidate: &CandidateRelease,
    verbose: bool,
    diagnostics: &mut Diagnostics,
) -> Result<CandidateExit, UserFailure> {
    diagnostics.emit(DiagnosticEvent::StartingStagedCandidate);
    diagnostics.flush();
    match run_candidate_apply(candidate, verbose) {
        CandidateApplyResult::Exit0 => {
            diagnostics.emit(DiagnosticEvent::StagedCandidateExitedSuccessfully);
            Ok(CandidateExit::Zero)
        }
        CandidateApplyResult::Exit1 => Ok(CandidateExit::One),
        CandidateApplyResult::SpawnFailed => Err(UserFailure::Ordinary(
            OrdinaryFailure::UpdateInstallationFilesRetry,
        )),
        CandidateApplyResult::CompletionUnknown => Err(UserFailure::UpdateCompletionUnknown),
    }
}
