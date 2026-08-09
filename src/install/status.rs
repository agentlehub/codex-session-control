use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    app_server::{AppServerClient, TESTED_CODEX_VERSION, socket_mode_is_owner_only},
    desktop::{
        DescriptorState, DesktopStructure, inspect_descriptor, inspect_desktop_structure,
        render_descriptor,
    },
    error::ControllerError,
    model::{DesktopAttachmentIdentity, InstalledRelease},
};

use super::{
    display_command_for_paths,
    evidence::{
        EvidenceSourceState, InstalledEvidenceCase, ResolvedUserPaths,
        classify_selected_home_evidence,
    },
    native::{
        plugin_matches, product_plugins, read_codex_version, resolve_named_executable,
        run_codex_json,
    },
    paths::{SOCKET_SECURITY_REQUIREMENT, StatusFileError, read_status_file},
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
    pub(super) path_environment: OsString,
    pub(super) desktop_environment: BTreeMap<OsString, OsString>,
    pub(super) cwd: PathBuf,
}

#[derive(Debug)]
pub(crate) struct StatusReport {
    pub stdout: String,
    pub healthy: bool,
}

#[derive(Debug)]
struct StatusFailure {
    check: &'static str,
    detail: String,
    action: String,
}

struct InstalledStatusState {
    manifest: Option<InstalledRelease>,
    codex: Option<PathBuf>,
    codex_version: Option<(String, String)>,
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

pub(crate) async fn status(target: LifecycleTarget) -> Result<StatusReport, ControllerError> {
    let path_environment = std::env::var_os("PATH").ok_or(ControllerError::InvalidData {
        field: "PATH",
        reason: "is unavailable",
    })?;
    let cwd = std::env::current_dir().map_err(|_| ControllerError::InvalidData {
        field: "cwd",
        reason: "is unavailable",
    })?;
    status_with_context(StatusContext {
        target,
        path_environment,
        desktop_environment: std::env::vars_os().collect(),
        cwd,
    })
    .await
}

pub(super) async fn status_with_context(
    context: StatusContext,
) -> Result<StatusReport, ControllerError> {
    let paths = &context.target.paths;
    let display_command = display_command_for_paths(paths, &context.path_environment);
    let setup_action = format!("{display_command} setup");
    let update_action = format!("{display_command} update");
    let journal_action = format!("journalctl --user -u {}", context.target.unit_name);
    let mut failures = Vec::new();

    let installed =
        inspect_installed_artifacts(paths, &setup_action, &update_action, &mut failures);
    inspect_native_registration(paths, &installed, &setup_action, &mut failures);
    let service = inspect_service_and_socket(
        &context,
        &installed,
        &setup_action,
        &journal_action,
        &mut failures,
    );
    let app_server_health = inspect_app_server_health(
        paths,
        &installed,
        &service,
        &update_action,
        &journal_action,
        &mut failures,
    )
    .await;
    let desktop_configuration = inspect_desktop_configuration(
        installed.manifest.as_ref(),
        &context,
        &service,
        app_server_health,
        &setup_action,
        &update_action,
        &mut failures,
    );

    Ok(render_status_report(
        &display_command,
        &installed,
        &service,
        desktop_configuration,
        failures,
    ))
}

fn inspect_installed_artifacts(
    paths: &ResolvedUserPaths,
    setup_action: &str,
    update_action: &str,
    failures: &mut Vec<StatusFailure>,
) -> InstalledStatusState {
    let evidence = classify_selected_home_evidence(paths);
    match evidence.case {
        InstalledEvidenceCase::Contradictory => failures.push(StatusFailure {
            check: "configuration",
            detail: "stored identity differs from installed manifest".to_owned(),
            action: update_action.to_owned(),
        }),
        InstalledEvidenceCase::PartialArtifactsWithoutIdentity => failures.push(StatusFailure {
            check: "manifest",
            detail: "partial product artifacts have no selected-home identity".to_owned(),
            action: setup_action.to_owned(),
        }),
        InstalledEvidenceCase::Coherent
        | InstalledEvidenceCase::ConfigurationOnly
        | InstalledEvidenceCase::ManifestOnly
        | InstalledEvidenceCase::FirstInstall
        | InstalledEvidenceCase::InvalidConfiguration
        | InstalledEvidenceCase::InvalidManifest => {}
    }

    let manifest = match evidence.manifest_source {
        EvidenceSourceState::Valid => evidence.manifest.clone(),
        EvidenceSourceState::Missing => {
            failures.push(status_file_failure(
                "manifest",
                &paths.manifest,
                StatusFileError::Missing,
                setup_action,
            ));
            None
        }
        EvidenceSourceState::InvalidFile(error) => {
            failures.push(status_file_failure(
                "manifest",
                &paths.manifest,
                error,
                setup_action,
            ));
            None
        }
        EvidenceSourceState::InvalidContent => {
            failures.push(StatusFailure {
                check: "manifest",
                detail: "invalid installed release manifest".to_owned(),
                action: setup_action.to_owned(),
            });
            None
        }
    };

    if let Some(manifest) = &manifest {
        match read_status_file(&paths.binary, paths.euid, 0o755) {
            Ok(bytes) if sha256_bytes(&bytes) == manifest.binary_sha256 => {}
            Ok(_) => failures.push(StatusFailure {
                check: "executable",
                detail: "digest does not match installed manifest".to_owned(),
                action: setup_action.to_owned(),
            }),
            Err(error) => failures.push(status_file_failure(
                "executable",
                &paths.binary,
                error,
                setup_action,
            )),
        }
    }

    let configuration = match evidence.configuration_source {
        EvidenceSourceState::Valid => evidence.configuration.clone(),
        EvidenceSourceState::Missing => {
            failures.push(status_file_failure(
                "configuration",
                &paths.config,
                StatusFileError::Missing,
                setup_action,
            ));
            None
        }
        EvidenceSourceState::InvalidFile(error) => {
            failures.push(status_file_failure(
                "configuration",
                &paths.config,
                error,
                setup_action,
            ));
            None
        }
        EvidenceSourceState::InvalidContent => {
            failures.push(StatusFailure {
                check: "configuration",
                detail: "invalid installed configuration".to_owned(),
                action: setup_action.to_owned(),
            });
            None
        }
    };

    if let Some(manifest) = &manifest {
        let expected = render_projection(&paths.binary, &manifest.product_version);
        let actual = read_projection(paths);
        let matches = match (expected.as_ref(), actual.as_ref()) {
            (Ok(expected), Ok(actual)) => {
                expected.sha256 == manifest.projection_sha256
                    && expected.marketplace == actual.marketplace
                    && expected.plugin == actual.plugin
                    && expected.mcp == actual.mcp
            }
            _ => false,
        };
        if !matches {
            failures.push(StatusFailure {
                check: "projection",
                detail: "digest does not match installed manifest".to_owned(),
                action: setup_action.to_owned(),
            });
        }
    }

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
        failures.push(StatusFailure {
            check: "configuration",
            detail: "stored identity differs from installed manifest".to_owned(),
            action: update_action.to_owned(),
        });
    }

    let codex_version =
        codex
            .as_ref()
            .and_then(|codex| match read_codex_version(codex, &paths.codex_home) {
                Ok(version) => Some(version),
                Err(error) => {
                    failures.push(StatusFailure {
                        check: "codex-version",
                        detail: error.to_string(),
                        action: update_action.to_owned(),
                    });
                    None
                }
            });

    InstalledStatusState {
        manifest,
        codex,
        codex_version,
    }
}

fn inspect_native_registration(
    paths: &ResolvedUserPaths,
    installed: &InstalledStatusState,
    setup_action: &str,
    failures: &mut Vec<StatusFailure>,
) {
    if let (Some(codex), Some(manifest)) = (&installed.codex, &installed.manifest) {
        let expected_source = paths
            .marketplace
            .join("plugins/codex-session-control")
            .to_string_lossy()
            .into_owned();
        let plugin_is_current = run_codex_json(
            codex,
            &paths.codex_home,
            &[
                OsStr::new("plugin"),
                OsStr::new("list"),
                OsStr::new("--json"),
            ],
        )
        .ok()
        .is_some_and(|value| {
            product_plugins(&value).is_ok_and(|product| {
                product.len() == 1
                    && plugin_matches(product[0], &manifest.plugin_version, &expected_source)
            })
        });
        if !plugin_is_current {
            failures.push(StatusFailure {
                check: "plugin",
                detail: "native registration does not match installed manifest".to_owned(),
                action: setup_action.to_owned(),
            });
        }
    }
}

fn inspect_service_and_socket(
    context: &StatusContext,
    installed: &InstalledStatusState,
    setup_action: &str,
    journal_action: &str,
    failures: &mut Vec<StatusFailure>,
) -> ServiceStatusState {
    let paths = &context.target.paths;
    if let Some(manifest) = &installed.manifest {
        match read_status_file(&paths.unit, paths.euid, 0o644) {
            Ok(bytes) if sha256_bytes(&bytes) == manifest.service_unit_sha256 => {}
            Ok(_) => failures.push(StatusFailure {
                check: "service-unit",
                detail: "digest does not match installed manifest".to_owned(),
                action: setup_action.to_owned(),
            }),
            Err(error) => failures.push(status_file_failure(
                "service-unit",
                &paths.unit,
                error,
                setup_action,
            )),
        }
    }

    let systemctl =
        resolve_named_executable(&context.path_environment, &context.cwd, "systemctl").ok();
    let enabled = systemctl.as_ref().and_then(|systemctl| {
        match query_systemctl_enablement(systemctl, &context.target.unit_name) {
            Ok(SystemctlEnablementState::Enabled) => Some(true),
            Ok(SystemctlEnablementState::Disabled | SystemctlEnablementState::NotFound) => {
                Some(false)
            }
            Err(error) => {
                failures.push(StatusFailure {
                    check: "service-state",
                    detail: error.to_string(),
                    action: journal_action.to_owned(),
                });
                None
            }
        }
    });
    let active = systemctl.as_ref().and_then(|systemctl| {
        match query_systemctl_activity(systemctl, &context.target.unit_name) {
            Ok(SystemctlActivityState::Active) => Some(true),
            Ok(SystemctlActivityState::Inactive) => Some(false),
            Err(error) => {
                failures.push(StatusFailure {
                    check: "service-state",
                    detail: error.to_string(),
                    action: journal_action.to_owned(),
                });
                None
            }
        }
    });
    if systemctl.is_none() {
        failures.push(StatusFailure {
            check: "service-state",
            detail: "systemctl is unavailable".to_owned(),
            action: journal_action.to_owned(),
        });
    }
    if let (Some(enabled), Some(active)) = (enabled, active) {
        if enabled && !active {
            failures.push(StatusFailure {
                check: "service-state",
                detail: "enabled service is not active".to_owned(),
                action: journal_action.to_owned(),
            });
        } else if !enabled && active {
            failures.push(StatusFailure {
                check: "service-state",
                detail: "disabled service is active".to_owned(),
                action: journal_action.to_owned(),
            });
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
            failures.push(StatusFailure {
                check: "socket",
                detail: "enabled service socket is missing".to_owned(),
                action: journal_action.to_owned(),
            });
        }
        (Some(false), Some(false), Ok(_)) => {
            failures.push(StatusFailure {
                check: "socket",
                detail: "disabled service socket is present".to_owned(),
                action: journal_action.to_owned(),
            });
        }
        (_, _, Ok(metadata))
            if metadata.file_type().is_symlink()
                || !metadata.file_type().is_socket()
                || metadata.uid() != paths.euid
                || !socket_mode_is_owner_only(metadata.mode()) =>
        {
            failures.push(StatusFailure {
                check: "socket",
                detail: format!("{}: {SOCKET_SECURITY_REQUIREMENT}", paths.socket.display()),
                action: unsafe_path_action(),
            });
        }
        (_, _, Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            failures.push(StatusFailure {
                check: "socket",
                detail: format!("{}: unreadable", paths.socket.display()),
                action: unsafe_path_action(),
            });
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
    update_action: &str,
    journal_action: &str,
    failures: &mut Vec<StatusFailure>,
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
                failures.push(StatusFailure {
                    check: "app-server-initialize",
                    detail: "running Codex version differs from executable".to_owned(),
                    action: update_action.to_owned(),
                });
            }
            Err(_) => {
                health = AppServerHealthState::Unhealthy;
                failures.push(StatusFailure {
                    check: "app-server-initialize",
                    detail: "app-server initialize failed".to_owned(),
                    action: journal_action.to_owned(),
                });
            }
        }
    }

    if let Some(manifest) = &installed.manifest
        && (manifest.product_version != env!("CARGO_PKG_VERSION")
            || manifest.target != product_target())
    {
        failures.push(StatusFailure {
            check: "manifest",
            detail: format!(
                "installed release {} ({}) differs from controller {} ({})",
                manifest.product_version,
                manifest.target,
                env!("CARGO_PKG_VERSION"),
                product_target()
            ),
            action: update_action.to_owned(),
        });
    }
    health
}

fn inspect_desktop_configuration(
    manifest: Option<&InstalledRelease>,
    context: &StatusContext,
    service: &ServiceStatusState,
    app_server_health: AppServerHealthState,
    setup_action: &str,
    update_action: &str,
    failures: &mut Vec<StatusFailure>,
) -> DesktopConfigurationState {
    if let Some(identity) = manifest.and_then(|manifest| manifest.desktop_attachment.as_ref()) {
        let descriptor = inspect_status_descriptor(
            identity,
            &context.target.paths.socket,
            failures,
            setup_action,
        );
        if descriptor == DescriptorStatusEvidence::NotReady
            && service.enabled == Some(true)
            && service.active == Some(true)
            && !failures
                .iter()
                .any(|failure| failure.check == "desktop-descriptor")
        {
            failures.push(StatusFailure {
                check: "desktop-descriptor",
                detail: format!("{} is missing", identity.descriptor_path.display()),
                action: update_action.to_owned(),
            });
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

fn render_status_report(
    display_command: &str,
    installed: &InstalledStatusState,
    service_state: &ServiceStatusState,
    desktop_configuration: DesktopConfigurationState,
    failures: Vec<StatusFailure>,
) -> StatusReport {
    let compatibility_warning = installed.codex_version.as_ref().and_then(|(display, expected)| {
        (expected != TESTED_CODEX_VERSION).then(|| {
            format!(
                "Compatibility warning: Codex app-server {display} has not been tested with codex-session-control {}; native results remain authoritative.\n",
                env!("CARGO_PKG_VERSION")
            )
        })
    });
    let installed_version = installed
        .manifest
        .as_ref()
        .map(|manifest| manifest.product_version.as_str())
        .unwrap_or("not_installed");
    let service = match (service_state.enabled, service_state.active) {
        (Some(enabled), Some(active)) => format!(
            "{}, {}",
            if enabled { "enabled" } else { "disabled" },
            if active { "active" } else { "inactive" }
        ),
        _ => "unknown, unknown".to_owned(),
    };
    let healthy = failures.is_empty();
    let desktop_configuration = match desktop_configuration {
        DesktopConfigurationState::Ready => "ready",
        DesktopConfigurationState::NotReady => "not_ready",
        DesktopConfigurationState::Unverified => "unverified",
        DesktopConfigurationState::Unavailable => "unavailable",
    };
    let mut stdout = compatibility_warning.unwrap_or_default();
    stdout.push_str(&format!(
        "Status: {}\n\
Installed release: {installed_version}\n\
Codex app-server service: {service}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop configuration: {desktop_configuration}\n\
Loaded task state: not_verified\n",
        if healthy { "healthy" } else { "drifted" },
    ));
    if healthy && service_state.enabled == Some(false) {
        stdout.push_str(&format!("Availability: {display_command} enable\n"));
    }
    if !failures.is_empty() {
        stdout.push_str("Failed checks:\n");
        for failure in failures {
            stdout.push_str(&format!(
                "- {}: {}\n  action: {}\n",
                failure.check, failure.detail, failure.action
            ));
        }
    }
    StatusReport { stdout, healthy }
}

fn inspect_status_descriptor(
    identity: &DesktopAttachmentIdentity,
    socket: &Path,
    failures: &mut Vec<StatusFailure>,
    setup_action: &str,
) -> DescriptorStatusEvidence {
    let expected = match render_descriptor(socket) {
        Ok(expected) => expected,
        Err(error) => {
            failures.push(StatusFailure {
                check: "desktop-descriptor",
                detail: error.to_string(),
                action: unsafe_path_action(),
            });
            return DescriptorStatusEvidence::Unverified;
        }
    };
    match inspect_descriptor(identity, &expected) {
        Ok(DescriptorState::Foreign) => {
            failures.push(StatusFailure {
                check: "desktop-descriptor",
                detail: format!("{} is foreign", identity.descriptor_path.display()),
                action: setup_action.to_owned(),
            });
            DescriptorStatusEvidence::NotReady
        }
        Ok(DescriptorState::Absent) => DescriptorStatusEvidence::NotReady,
        Ok(DescriptorState::Expected) => DescriptorStatusEvidence::Expected,
        Err(error) => {
            let evidence = classify_descriptor_inspection_error(&error);
            failures.push(StatusFailure {
                check: "desktop-descriptor",
                detail: error.to_string(),
                action: unsafe_path_action(),
            });
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

fn status_file_failure(
    check: &'static str,
    path: &Path,
    error: StatusFileError,
    repair_action: &str,
) -> StatusFailure {
    match error {
        StatusFileError::Missing => StatusFailure {
            check,
            detail: "missing".to_owned(),
            action: repair_action.to_owned(),
        },
        StatusFileError::Unsafe => StatusFailure {
            check,
            detail: format!("{}: unsafe owner, type, or mode", path.display()),
            action: unsafe_path_action(),
        },
        StatusFileError::Unreadable => StatusFailure {
            check,
            detail: format!("{}: unreadable", path.display()),
            action: unsafe_path_action(),
        },
    }
}

fn unsafe_path_action() -> String {
    "inspect the path and restore its approved ownership, type, and mode".to_owned()
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
