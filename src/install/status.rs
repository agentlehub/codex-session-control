use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use semver::Version;
use sha2::{Digest, Sha256};

use crate::{
    app_server::{AppServerClient, socket_mode_is_owner_only},
    cli_output::{IntegrationState, ServiceSummary, StatusProblem, StatusResult, StatusState},
    desktop::{
        DescriptorState, DesktopStructure, inspect_descriptor, inspect_desktop_structure,
        render_descriptor,
    },
    diagnostics::{DiagnosticCause, Diagnostics},
    error::ControllerError,
    model::{DesktopAttachmentIdentity, InstalledRelease},
};

use super::{
    evidence::{InstalledEvidenceCase, ResolvedUserPaths, classify_selected_home_evidence},
    native::{
        plugin_matches, product_plugins, read_codex_version, resolve_named_executable,
        run_codex_json,
    },
    paths::{StatusFileError, read_status_file},
    product_target,
    render::{RenderedProjection, render_projection},
    service::{
        LifecycleTarget, SystemctlActivityState, SystemctlEnablementState,
        query_systemctl_activity, query_systemctl_enablement,
    },
    sha256_bytes,
};

#[derive(Clone, Debug)]
pub(super) struct StatusContext {
    pub(super) target: LifecycleTarget,
    pub(super) path_environment: Option<OsString>,
    pub(super) desktop_environment: BTreeMap<OsString, OsString>,
    pub(super) cwd: Option<PathBuf>,
}

struct InstalledStatusState {
    case: InstalledEvidenceCase,
    manifest: Option<InstalledRelease>,
    codex: Option<PathBuf>,
    codex_version: Option<(String, String)>,
    projection: ProjectionEvidence,
}

struct ServiceStatusState {
    enabled: Option<bool>,
    active: Option<bool>,
    socket_present: bool,
    socket_safe: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppServerHealthState {
    Healthy,
    Unhealthy,
    Unverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopConfigurationState {
    Ready,
    NotReady,
    Unverified,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DescriptorStatusEvidence {
    Expected,
    NotReady,
    Unverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectionEvidence {
    Ready,
    Fault,
    CouldNotVerify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeRegistrationEvidence {
    Ready,
    Fault,
    CouldNotVerify,
}

#[derive(Clone, Copy)]
enum StatusStage {
    Preflight,
    InstalledState,
    NativeRegistration,
    Service,
    AppServer,
    Desktop,
}

impl StatusStage {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::InstalledState => "installed-state",
            Self::NativeRegistration => "native-registration",
            Self::Service => "service",
            Self::AppServer => "app-server",
            Self::Desktop => "desktop",
        }
    }
}

pub(crate) async fn status_from_paths(
    paths: Result<ResolvedUserPaths, ControllerError>,
    diagnostics: &mut Diagnostics,
) -> StatusResult {
    let paths = match paths {
        Ok(paths) => paths,
        Err(_) => {
            diagnostics.failed(
                StatusStage::Preflight.diagnostic_name(),
                DiagnosticCause::Validation,
            );
            return StatusResult::new(
                StatusState::Unhealthy,
                None,
                Some(ServiceSummary::CouldNotVerify),
                IntegrationState::CouldNotVerify,
                IntegrationState::CouldNotVerify,
                vec![StatusProblem::InstalledStateCouldNotBeVerified],
            );
        }
    };
    diagnostics.completed(StatusStage::Preflight.diagnostic_name());
    status_with_context(
        StatusContext {
            target: LifecycleTarget::production(paths),
            path_environment: std::env::var_os("PATH"),
            desktop_environment: std::env::vars_os().collect(),
            cwd: std::env::current_dir().ok(),
        },
        diagnostics,
    )
    .await
}

pub(super) async fn status_with_context(
    context: StatusContext,
    diagnostics: &mut Diagnostics,
) -> StatusResult {
    let paths = &context.target.paths;
    let mut problems = Vec::new();

    let installed = inspect_installed_artifacts(paths, &mut problems);
    diagnostics.completed(StatusStage::InstalledState.diagnostic_name());
    let native = inspect_native_registration(paths, &installed, &mut problems);
    diagnostics.completed(StatusStage::NativeRegistration.diagnostic_name());
    let service = inspect_service_and_socket(&context, &installed, &mut problems);
    diagnostics.completed(StatusStage::Service.diagnostic_name());
    let app_server_health =
        inspect_app_server_health(paths, &installed, &service, &mut problems).await;
    diagnostics.completed(StatusStage::AppServer.diagnostic_name());
    let desktop_configuration = inspect_desktop_configuration(
        installed.manifest.as_ref(),
        &context,
        &service,
        app_server_health,
        &mut problems,
    );
    diagnostics.completed(StatusStage::Desktop.diagnostic_name());

    project_status_result(
        &installed,
        native,
        &service,
        app_server_health,
        desktop_configuration,
        problems,
    )
}

fn inspect_installed_artifacts(
    paths: &ResolvedUserPaths,
    problems: &mut Vec<StatusProblem>,
) -> InstalledStatusState {
    let evidence = classify_selected_home_evidence(paths);
    match evidence.case {
        InstalledEvidenceCase::Contradictory
        | InstalledEvidenceCase::PartialArtifactsWithoutIdentity
        | InstalledEvidenceCase::InvalidConfiguration
        | InstalledEvidenceCase::InvalidManifest => {
            push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
        }
        InstalledEvidenceCase::Coherent
        | InstalledEvidenceCase::ConfigurationOnly
        | InstalledEvidenceCase::ManifestOnly
        | InstalledEvidenceCase::FirstInstall => {}
    }

    let manifest = evidence.manifest.clone();
    if manifest.is_none() && evidence.case != InstalledEvidenceCase::FirstInstall {
        push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
    }

    if let Some(manifest) = &manifest {
        match read_status_file(&paths.binary, paths.euid, 0o755) {
            Ok(bytes) if sha256_bytes(&bytes) == manifest.binary_sha256 => {}
            Ok(_) | Err(_) => {
                push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
            }
        }
    }

    let configuration = evidence.configuration.clone();
    if configuration.is_none() && evidence.case != InstalledEvidenceCase::FirstInstall {
        push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
    }

    let projection =
        manifest
            .as_ref()
            .map_or(ProjectionEvidence::CouldNotVerify, |manifest| {
                match (
                    render_projection(&paths.binary, &manifest.product_version),
                    read_projection(paths),
                ) {
                    (Ok(expected), Ok(actual))
                        if expected.sha256 == manifest.projection_sha256
                            && expected.marketplace == actual.marketplace
                            && expected.plugin == actual.plugin
                            && expected.mcp == actual.mcp =>
                    {
                        ProjectionEvidence::Ready
                    }
                    (Ok(_), Ok(_)) => {
                        push_problem(problems, StatusProblem::ProjectionFault);
                        ProjectionEvidence::Fault
                    }
                    _ => {
                        push_problem(problems, StatusProblem::ProjectionCouldNotBeVerified);
                        ProjectionEvidence::CouldNotVerify
                    }
                }
            });

    let codex = configuration
        .as_ref()
        .map(|config| config.codex_executable.clone())
        .or_else(|| {
            manifest
                .as_ref()
                .map(|manifest| manifest.codex_executable.clone())
        });
    let codex_identity_matches = match (&configuration, &manifest) {
        (Some(configuration), Some(manifest)) => {
            configuration.codex_executable == manifest.codex_executable
                && configuration.codex_home == manifest.codex_home
                && configuration.socket_path == manifest.socket_path
        }
        _ => true,
    };
    if !codex_identity_matches {
        push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
    }

    let codex_version =
        codex
            .as_ref()
            .and_then(|codex| match read_codex_version(codex, &paths.codex_home) {
                Ok(version) => Some(version),
                Err(_) => {
                    push_problem(
                        problems,
                        StatusProblem::NativeRegistrationCouldNotBeVerified,
                    );
                    None
                }
            });

    InstalledStatusState {
        case: evidence.case,
        manifest,
        codex,
        codex_version,
        projection,
    }
}

fn inspect_native_registration(
    paths: &ResolvedUserPaths,
    installed: &InstalledStatusState,
    problems: &mut Vec<StatusProblem>,
) -> NativeRegistrationEvidence {
    let (Some(codex), Some(manifest)) = (&installed.codex, &installed.manifest) else {
        return NativeRegistrationEvidence::CouldNotVerify;
    };
    let expected_source = paths
        .marketplace
        .join("plugins/codex-session-control")
        .to_string_lossy()
        .into_owned();
    let value = match run_codex_json(
        codex,
        &paths.codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    ) {
        Ok(value) => value,
        Err(_) => {
            push_problem(
                problems,
                StatusProblem::NativeRegistrationCouldNotBeVerified,
            );
            return NativeRegistrationEvidence::CouldNotVerify;
        }
    };
    let product = match product_plugins(&value) {
        Ok(product) => product,
        Err(_) => {
            push_problem(
                problems,
                StatusProblem::NativeRegistrationCouldNotBeVerified,
            );
            return NativeRegistrationEvidence::CouldNotVerify;
        }
    };
    if product.len() == 1 && plugin_matches(product[0], &manifest.plugin_version, &expected_source)
    {
        NativeRegistrationEvidence::Ready
    } else {
        push_problem(problems, StatusProblem::NativeRegistrationFault);
        NativeRegistrationEvidence::Fault
    }
}

fn inspect_service_and_socket(
    context: &StatusContext,
    installed: &InstalledStatusState,
    problems: &mut Vec<StatusProblem>,
) -> ServiceStatusState {
    let paths = &context.target.paths;
    if let Some(manifest) = &installed.manifest {
        match read_status_file(&paths.unit, paths.euid, 0o644) {
            Ok(bytes) if sha256_bytes(&bytes) == manifest.service_unit_sha256 => {}
            Ok(_) | Err(_) => {
                push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
            }
        }
    }

    if context.path_environment.is_none() || context.cwd.is_none() {
        push_problem(problems, StatusProblem::InvocationContextCouldNotBeVerified);
    }
    let systemctl = context
        .path_environment
        .as_deref()
        .zip(context.cwd.as_deref())
        .and_then(|(path_environment, cwd)| {
            resolve_named_executable(path_environment, cwd, "systemctl").ok()
        });
    let enabled = systemctl.as_ref().and_then(|systemctl| {
        match query_systemctl_enablement(systemctl, &context.target.unit_name) {
            Ok(SystemctlEnablementState::Enabled) => Some(true),
            Ok(SystemctlEnablementState::Disabled | SystemctlEnablementState::NotFound) => {
                Some(false)
            }
            Err(_) => {
                push_problem(problems, StatusProblem::ServiceEnablementCouldNotBeVerified);
                None
            }
        }
    });
    let active = systemctl.as_ref().and_then(|systemctl| {
        match query_systemctl_activity(systemctl, &context.target.unit_name) {
            Ok(SystemctlActivityState::Active) => Some(true),
            Ok(SystemctlActivityState::Inactive) => Some(false),
            Err(_) => {
                push_problem(problems, StatusProblem::ServiceActivityCouldNotBeVerified);
                None
            }
        }
    });
    if systemctl.is_none() {
        push_problem(problems, StatusProblem::ServiceEnablementCouldNotBeVerified);
        push_problem(problems, StatusProblem::ServiceActivityCouldNotBeVerified);
    }
    if let (Some(enabled), Some(active)) = (enabled, active) {
        if enabled && !active {
            push_problem(problems, StatusProblem::ServiceConfiguredButStopped);
        } else if !enabled && active {
            push_problem(problems, StatusProblem::ServiceActivityCouldNotBeVerified);
        }
    }

    let socket_metadata = fs::symlink_metadata(&paths.socket);
    let socket_present = socket_metadata.is_ok();
    let socket_safe = match &socket_metadata {
        Ok(metadata) => Some(
            !metadata.file_type().is_symlink()
                && metadata.file_type().is_socket()
                && metadata.uid() == paths.euid
                && socket_mode_is_owner_only(metadata.mode()),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    };
    match (enabled, active, &socket_metadata) {
        (Some(true), _, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            push_problem(problems, StatusProblem::SocketMissing);
        }
        (Some(false), Some(false), Ok(_)) => {
            push_problem(problems, StatusProblem::SocketUnsafe);
        }
        (_, _, Ok(metadata))
            if metadata.file_type().is_symlink()
                || !metadata.file_type().is_socket()
                || metadata.uid() != paths.euid
                || !socket_mode_is_owner_only(metadata.mode()) =>
        {
            push_problem(problems, StatusProblem::SocketUnsafe);
        }
        (_, _, Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            push_problem(problems, StatusProblem::SocketUnsafe);
        }
        _ => {}
    }

    ServiceStatusState {
        enabled,
        active,
        socket_present,
        socket_safe,
    }
}

async fn inspect_app_server_health(
    paths: &ResolvedUserPaths,
    installed: &InstalledStatusState,
    service: &ServiceStatusState,
    problems: &mut Vec<StatusProblem>,
) -> AppServerHealthState {
    let mut health = AppServerHealthState::Unverified;
    if service.active == Some(true)
        && service.socket_present
        && service.socket_safe == Some(true)
        && let Some((_, expected_running_version)) = &installed.codex_version
    {
        let client = AppServerClient::new(
            paths.socket.clone(),
            paths.codex_home.clone(),
            env!("CARGO_PKG_VERSION").to_owned(),
            expected_running_version.clone(),
        );
        match client.connect_initialized().await {
            Ok(connection) if connection.compatibility_warning().is_none() => {
                health = AppServerHealthState::Healthy;
            }
            Ok(_) => {
                health = AppServerHealthState::Unhealthy;
                push_problem(problems, StatusProblem::AppServerUnavailable);
            }
            Err(_) => {
                health = AppServerHealthState::Unhealthy;
                push_problem(problems, StatusProblem::AppServerUnavailable);
            }
        }
    } else if service.active == Some(true)
        && service.socket_present
        && service.socket_safe == Some(true)
        && installed.codex_version.is_none()
    {
        push_problem(problems, StatusProblem::AppServerCouldNotBeVerified);
    }

    if let Some(manifest) = &installed.manifest
        && (manifest.product_version != env!("CARGO_PKG_VERSION")
            || manifest.target != product_target())
    {
        push_problem(problems, StatusProblem::InstalledStateCouldNotBeVerified);
    }
    health
}

fn inspect_desktop_configuration(
    manifest: Option<&InstalledRelease>,
    context: &StatusContext,
    service: &ServiceStatusState,
    app_server_health: AppServerHealthState,
    problems: &mut Vec<StatusProblem>,
) -> DesktopConfigurationState {
    if let Some(identity) = manifest.and_then(|manifest| manifest.desktop_attachment.as_ref()) {
        let descriptor =
            inspect_status_descriptor(identity, &context.target.paths.socket, problems);
        if descriptor == DescriptorStatusEvidence::NotReady
            && service.enabled == Some(true)
            && service.active == Some(true)
            && !problems.contains(&StatusProblem::DesktopDescriptorFault)
        {
            push_problem(problems, StatusProblem::DesktopDescriptorFault);
        }
        classify_desktop_configuration(descriptor, service, app_server_health)
    } else {
        match inspect_desktop_structure(None, &context.desktop_environment) {
            DesktopStructure::Detected => DesktopConfigurationState::Unverified,
            DesktopStructure::Unavailable => DesktopConfigurationState::Unavailable,
        }
    }
}

fn classify_desktop_configuration(
    descriptor: DescriptorStatusEvidence,
    service: &ServiceStatusState,
    app_server_health: AppServerHealthState,
) -> DesktopConfigurationState {
    if descriptor == DescriptorStatusEvidence::NotReady
        || service.enabled == Some(false)
        || service.active == Some(false)
        || service.socket_safe == Some(false)
        || app_server_health == AppServerHealthState::Unhealthy
    {
        return DesktopConfigurationState::NotReady;
    }
    if descriptor == DescriptorStatusEvidence::Unverified
        || service.enabled.is_none()
        || service.active.is_none()
        || service.socket_safe.is_none()
        || app_server_health == AppServerHealthState::Unverified
    {
        return DesktopConfigurationState::Unverified;
    }
    DesktopConfigurationState::Ready
}

fn project_status_result(
    installed: &InstalledStatusState,
    native: NativeRegistrationEvidence,
    service_state: &ServiceStatusState,
    app_server_health: AppServerHealthState,
    desktop_configuration: DesktopConfigurationState,
    problems: Vec<StatusProblem>,
) -> StatusResult {
    if installed.case == InstalledEvidenceCase::FirstInstall {
        return StatusResult::new(
            StatusState::NotInstalled,
            None,
            None,
            IntegrationState::Unavailable,
            IntegrationState::Unavailable,
            Vec::new(),
        );
    }

    let version = installed
        .manifest
        .as_ref()
        .and_then(|manifest| Version::parse(&manifest.product_version).ok());
    let service = match (service_state.enabled, service_state.active) {
        (Some(true), Some(true)) => ServiceSummary::RunningAutomatic,
        (Some(false), Some(false)) => ServiceSummary::StoppedAutomaticOff,
        (Some(true), Some(false)) => ServiceSummary::StoppedUnexpectedAutomaticOn,
        _ => ServiceSummary::CouldNotVerify,
    };
    let shared_fault = matches!(
        (service_state.enabled, service_state.active),
        (Some(true), Some(false)) | (Some(false), Some(true))
    ) || (service_state.socket_present
        && service_state.socket_safe == Some(false))
        || app_server_health == AppServerHealthState::Unhealthy;
    let shared_unverified = service_state.enabled.is_none()
        || service_state.active.is_none()
        || service_state.socket_safe.is_none()
        || (service_state.active == Some(true)
            && app_server_health == AppServerHealthState::Unverified);
    let disabled = service_state.enabled == Some(false)
        && service_state.active == Some(false)
        && !service_state.socket_present;

    let cli = if shared_fault
        || installed.projection == ProjectionEvidence::Fault
        || native == NativeRegistrationEvidence::Fault
    {
        IntegrationState::Unhealthy
    } else if shared_unverified
        || installed.projection == ProjectionEvidence::CouldNotVerify
        || native == NativeRegistrationEvidence::CouldNotVerify
    {
        IntegrationState::CouldNotVerify
    } else if disabled {
        IntegrationState::Unavailable
    } else {
        IntegrationState::Ready
    };
    let desktop = match desktop_configuration {
        DesktopConfigurationState::Unavailable => IntegrationState::Unavailable,
        DesktopConfigurationState::NotReady => {
            if disabled {
                IntegrationState::Unavailable
            } else {
                IntegrationState::Unhealthy
            }
        }
        DesktopConfigurationState::Unverified if shared_fault => IntegrationState::Unhealthy,
        DesktopConfigurationState::Unverified => IntegrationState::CouldNotVerify,
        DesktopConfigurationState::Ready if disabled => IntegrationState::Unavailable,
        DesktopConfigurationState::Ready => IntegrationState::Ready,
    };
    let state = if problems.is_empty() && disabled {
        StatusState::Disabled
    } else if problems.is_empty() && cli == IntegrationState::Ready {
        StatusState::Healthy
    } else {
        StatusState::Unhealthy
    };

    StatusResult::new(state, version, Some(service), cli, desktop, problems)
}

fn inspect_status_descriptor(
    identity: &DesktopAttachmentIdentity,
    socket: &Path,
    problems: &mut Vec<StatusProblem>,
) -> DescriptorStatusEvidence {
    let expected = match render_descriptor(socket) {
        Ok(expected) => expected,
        Err(_) => {
            push_problem(problems, StatusProblem::DesktopCouldNotBeVerified);
            return DescriptorStatusEvidence::Unverified;
        }
    };
    match inspect_descriptor(identity, &expected) {
        Ok(DescriptorState::Foreign) => {
            push_problem(problems, StatusProblem::DesktopDescriptorFault);
            DescriptorStatusEvidence::NotReady
        }
        Ok(DescriptorState::Absent) => DescriptorStatusEvidence::NotReady,
        Ok(DescriptorState::Expected) => DescriptorStatusEvidence::Expected,
        Err(error) => {
            let evidence = classify_descriptor_inspection_error(&error);
            push_problem(
                problems,
                if evidence == DescriptorStatusEvidence::NotReady {
                    StatusProblem::DesktopDescriptorFault
                } else {
                    StatusProblem::DesktopCouldNotBeVerified
                },
            );
            evidence
        }
    }
}

fn classify_descriptor_inspection_error(error: &ControllerError) -> DescriptorStatusEvidence {
    const PREFIX: &str = "Desktop descriptor safety error: ";

    match error {
        ControllerError::InvalidData { .. } => DescriptorStatusEvidence::NotReady,
        ControllerError::Operational(detail) => match detail.strip_prefix(PREFIX) {
            Some(
                "descriptor ancestor is unsafe"
                | "descriptor ancestor leaves the effective-user tree"
                | "descriptor parent is not owned by the effective user"
                | "descriptor is not an owner-only regular file"
                | "descriptor JSON is invalid"
                | "descriptor schema is unsupported"
                | "descriptor socket path must be UTF-8"
                | "descriptor socket path is not a normalized absolute path",
            ) => DescriptorStatusEvidence::NotReady,
            // Safe-open, metadata, read, race, and unknown inspection failures do not prove drift.
            _ => DescriptorStatusEvidence::Unverified,
        },
    }
}

fn push_problem(problems: &mut Vec<StatusProblem>, problem: StatusProblem) {
    if !problems.contains(&problem) {
        problems.push(problem);
    }
}

fn read_projection(paths: &ResolvedUserPaths) -> Result<RenderedProjection, StatusFileError> {
    let marketplace = read_status_file(
        &paths.marketplace.join(".agents/plugins/marketplace.json"),
        paths.euid,
        0o644,
    )?;
    let plugin = read_status_file(
        &paths
            .marketplace
            .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
        paths.euid,
        0o644,
    )?;
    let mcp = read_status_file(
        &paths
            .marketplace
            .join("plugins/codex-session-control/.mcp.json"),
        paths.euid,
        0o644,
    )?;
    let mut digest = Sha256::new();
    for (relative, bytes) in [
        (".agents/plugins/marketplace.json", marketplace.as_slice()),
        (
            "plugins/codex-session-control/.codex-plugin/plugin.json",
            plugin.as_slice(),
        ),
        ("plugins/codex-session-control/.mcp.json", mcp.as_slice()),
    ] {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(bytes);
        digest.update([0]);
    }
    Ok(RenderedProjection {
        marketplace,
        plugin,
        mcp,
        sha256: hex::encode(digest.finalize()),
    })
}
