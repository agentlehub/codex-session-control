use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::os::unix::fs::PermissionsExt;

use serde_json::Value;

use crate::{
    app_server::TESTED_CODEX_VERSION,
    cli_output::{
        DesktopAvailability as OutputDesktopAvailability, ManagedPaths, OrdinaryFailure,
        RollbackIncomplete, RollbackPrimary, SetupSuccess, UserFailure, UserNotice, UserSuccess,
    },
    desktop::{
        DescriptorPublicationFailure, DescriptorPublicationResidue, DescriptorState,
        DesktopAvailability, DesktopTarget, inspect_descriptor, preflight_descriptor_switch,
        probe_desktop_capability, probe_persisted_desktop_capability, publish_descriptor,
        remove_expected_descriptor, render_descriptor,
    },
    diagnostics::{DiagnosticCause, DiagnosticEvent, DiagnosticTarget, Diagnostics},
    error::ControllerError,
    model::{DesktopAttachmentIdentity, InstalledRelease, ProductConfig},
};

use super::{
    CandidateRelease, DesktopAttachmentStatus, UNKNOWN_CODEX_VERSION,
    cleanup_changed_descriptor_after_start_failure,
    evidence::{
        InstalledEvidenceCase, NativeProductState, ResolvedUserPaths, SelectedHomeEvidence,
        SelectedHomeOperation, classify_selected_home_evidence, require_selected_home_evidence,
        resolve_setup_selected_home,
    },
    native::{
        plugin_matches, read_codex_version, read_installed_product_version, reconcile_marketplace,
        reconcile_plugin, resolve_named_executable,
    },
    paths::{
        FileKind, create_missing_selected_codex_home, create_product_dir, create_shared_dir,
        read_product_evidence_file, reconcile_file, resolve_codex_executable, validate_existing,
    },
    product_target,
    release::RELEASE_REPOSITORY,
    render::{RenderedProjection, reconcile_projection, render_projection, render_unit},
    service::{
        LifecycleTarget, detect_running_unattached_clients, run_systemctl, verify_setup_service,
    },
    sha256_bytes,
};

#[derive(Clone, Debug)]
pub(super) struct SetupContext {
    pub(super) target: LifecycleTarget,
    pub(super) candidate: CandidateRelease,
    pub(super) path_environment: std::ffi::OsString,
    pub(super) desktop_environment: BTreeMap<OsString, OsString>,
    pub(super) desktop_launcher: Option<PathBuf>,
    pub(super) cwd: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SetupStage {
    Preflight,
    Binary,
    Configuration,
    Projection,
    PluginMarketplace,
    PluginInstall,
    DesktopDiscovery,
    Descriptor,
    ServiceUnit,
    DaemonReload,
    ServiceEnable,
    ServiceVerify,
    Manifest,
}

impl SetupStage {
    #[cfg(test)]
    const fn name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::Binary => "binary",
            Self::Configuration => "configuration",
            Self::Projection => "projection",
            Self::PluginMarketplace => "plugin-marketplace",
            Self::PluginInstall => "plugin-install",
            Self::DesktopDiscovery => "desktop-discovery",
            Self::Descriptor => "descriptor",
            Self::ServiceUnit => "service-unit",
            Self::DaemonReload => "daemon-reload",
            Self::ServiceEnable => "service-enable",
            Self::ServiceVerify => "service-verify",
            Self::Manifest => "manifest",
        }
    }

    fn completed_event(self) -> DiagnosticEvent {
        match self {
            Self::Preflight => DiagnosticEvent::CompletedPreflight,
            Self::Binary => DiagnosticEvent::CompletedBinary,
            Self::Configuration => DiagnosticEvent::CompletedConfiguration,
            Self::Projection => DiagnosticEvent::CompletedProjection,
            Self::PluginMarketplace => DiagnosticEvent::CompletedPluginMarketplace,
            Self::PluginInstall => DiagnosticEvent::CompletedPluginInstall,
            Self::DesktopDiscovery => DiagnosticEvent::CompletedDesktopDiscovery,
            Self::Descriptor => DiagnosticEvent::CompletedDescriptor,
            Self::ServiceUnit => DiagnosticEvent::CompletedServiceUnit,
            Self::DaemonReload => DiagnosticEvent::CompletedDaemonReload,
            Self::ServiceEnable => DiagnosticEvent::CompletedServiceEnable,
            Self::ServiceVerify => DiagnosticEvent::CompletedServiceVerify,
            Self::Manifest => DiagnosticEvent::CompletedManifest,
        }
    }

    fn failed_event(self, cause: DiagnosticCause) -> DiagnosticEvent {
        match self {
            Self::Preflight => DiagnosticEvent::FailedPreflight { cause },
            Self::Binary => DiagnosticEvent::FailedBinary { cause },
            Self::Configuration => DiagnosticEvent::FailedConfiguration { cause },
            Self::Projection => DiagnosticEvent::FailedProjection { cause },
            Self::PluginMarketplace => DiagnosticEvent::FailedPluginMarketplace { cause },
            Self::PluginInstall => DiagnosticEvent::FailedPluginInstall { cause },
            Self::DesktopDiscovery => DiagnosticEvent::FailedDesktopDiscovery { cause },
            Self::Descriptor => DiagnosticEvent::FailedDescriptor { cause },
            Self::ServiceUnit => DiagnosticEvent::FailedServiceUnit { cause },
            Self::DaemonReload => DiagnosticEvent::FailedDaemonReload { cause },
            Self::ServiceEnable => DiagnosticEvent::FailedServiceEnable { cause },
            Self::ServiceVerify => DiagnosticEvent::FailedServiceVerify { cause },
            Self::Manifest => DiagnosticEvent::FailedManifest { cause },
        }
    }
}

fn setup_failed(
    diagnostics: &mut Diagnostics,
    stage: SetupStage,
    cause: DiagnosticCause,
    failure: UserFailure,
) -> UserFailure {
    diagnostics.emit(stage.failed_event(cause));
    failure
}

fn complete_setup_stage(
    diagnostics: &mut Diagnostics,
    stage: SetupStage,
    _target: &LifecycleTarget,
) -> Result<(), UserFailure> {
    diagnostics.emit(stage.completed_event());
    #[cfg(test)]
    if _target.test_hooks.fail_after_completed_stage == Some(stage.name()) {
        return Err(setup_failed(
            diagnostics,
            stage,
            DiagnosticCause::Unexpected,
            UserFailure::Ordinary(OrdinaryFailure::SetupUnexpectedRetry),
        ));
    }
    Ok(())
}

fn descriptor_residue_path(residue: DescriptorPublicationResidue) -> PathBuf {
    match residue {
        DescriptorPublicationResidue::Stage(path) | DescriptorPublicationResidue::Final(path) => {
            path
        }
    }
}

fn setup_desktop_rollback(paths: Vec<PathBuf>) -> UserFailure {
    let mut paths = paths.into_iter();
    let first = paths
        .next()
        .expect("Desktop rollback has at least one exact managed path");
    UserFailure::RollbackIncomplete(RollbackIncomplete::new(
        RollbackPrimary::SetupDesktopRetry,
        ManagedPaths::new(first, paths.collect()),
    ))
}

pub(super) fn setup_cli_reconciliation_failure(error: &ControllerError) -> UserFailure {
    UserFailure::Ordinary(match error {
        ControllerError::InvalidData { .. } => OrdinaryFailure::SetupCliIntegrationCheckStatus,
        ControllerError::Operational(_) => OrdinaryFailure::SetupCliIntegrationRetry,
    })
}

pub(super) fn setup_descriptor_publication_failure(
    failure: DescriptorPublicationFailure,
) -> UserFailure {
    match failure.residue {
        Some(residue) => setup_desktop_rollback(vec![descriptor_residue_path(residue)]),
        None => UserFailure::Ordinary(OrdinaryFailure::SetupDesktopIntegrationRetry),
    }
}

pub(super) fn setup_invocation_failure() -> UserFailure {
    UserFailure::Ordinary(OrdinaryFailure::SetupUnsafeTerminalRetry)
}

pub(super) struct SetupPreflight {
    codex: PathBuf,
    systemctl: PathBuf,
    expected_running_version: String,
    compatibility_codex_version: Option<semver::Version>,
    binary: Vec<u8>,
    binary_sha256: String,
    config: Vec<u8>,
    projection: RenderedProjection,
    unit: Vec<u8>,
    unit_sha256: String,
    desktop: SetupDesktopPlan,
}

#[derive(Clone, Debug)]
struct SetupDesktopPlan {
    attachment: Option<DesktopAttachmentIdentity>,
    previous_attachment: Option<DesktopAttachmentIdentity>,
    target: Option<DesktopTarget>,
    descriptor: Option<Vec<u8>>,
    status: DesktopAttachmentStatus,
    warning: Option<String>,
}

pub(crate) async fn setup(
    desktop_launcher: Option<&Path>,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    diagnostics.emit(DiagnosticEvent::ControllerStarted {
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("package version is semantic"),
        target: DiagnosticTarget::current(),
    });
    let paths = ResolvedUserPaths::from_effective_user().map_err(|_| {
        setup_failed(
            diagnostics,
            SetupStage::Preflight,
            DiagnosticCause::Validation,
            setup_invocation_failure(),
        )
    })?;
    let candidate = CandidateRelease {
        executable: std::env::current_exe().map_err(|_| {
            setup_failed(
                diagnostics,
                SetupStage::Preflight,
                DiagnosticCause::Unexpected,
                UserFailure::Ordinary(OrdinaryFailure::SetupUnexpectedRetry),
            )
        })?,
        product_version: env!("CARGO_PKG_VERSION").to_owned(),
        target: product_target().to_owned(),
    };
    let path_environment = std::env::var_os("PATH").ok_or_else(|| {
        setup_failed(
            diagnostics,
            SetupStage::Preflight,
            DiagnosticCause::Validation,
            setup_invocation_failure(),
        )
    })?;
    let cwd = std::env::current_dir().map_err(|_| {
        setup_failed(
            diagnostics,
            SetupStage::Preflight,
            DiagnosticCause::Validation,
            setup_invocation_failure(),
        )
    })?;
    setup_with_context_and_diagnostics(
        SetupContext {
            target: LifecycleTarget::production(paths),
            candidate,
            path_environment,
            desktop_environment: std::env::vars_os().collect(),
            desktop_launcher: desktop_launcher.map(Path::to_path_buf),
            cwd,
        },
        diagnostics,
    )
    .await
}

#[cfg(test)]
pub(super) async fn setup_with_context(context: SetupContext) -> Result<UserSuccess, UserFailure> {
    let mut diagnostics = Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Setup);
    setup_with_context_and_diagnostics(context, &mut diagnostics).await
}

pub(super) async fn setup_with_context_and_diagnostics(
    mut context: SetupContext,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    diagnostics.emit(DiagnosticEvent::ControllerStarted {
        version: semver::Version::parse(&context.candidate.product_version)
            .unwrap_or_else(|_| semver::Version::new(0, 0, 0)),
        target: DiagnosticTarget::current(),
    });
    let preflight = match setup_preflight(&mut context).await {
        Ok(preflight) => preflight,
        Err(failure) => {
            return Err(setup_failed(
                diagnostics,
                SetupStage::Preflight,
                DiagnosticCause::Validation,
                failure,
            ));
        }
    };
    diagnostics.emit(DiagnosticEvent::SelectedCodexHome {
        codex_home: context.target.paths.codex_home.clone(),
    });
    complete_setup_stage(diagnostics, SetupStage::Preflight, &context.target)?;

    let paths = &context.target.paths;
    if (|| {
        create_shared_dir(
            paths.binary.parent().ok_or(ControllerError::InvalidData {
                field: "binary",
                reason: "has no parent",
            })?,
            paths.euid,
        )?;
        reconcile_file(&paths.binary, &preflight.binary, 0o755, paths.euid)
    })()
    .is_err()
    {
        return Err(setup_failed(
            diagnostics,
            SetupStage::Binary,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::SetupInstallationFilesRetry),
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::Binary, &context.target)?;

    if (|| {
        create_product_dir(
            paths.config.parent().ok_or(ControllerError::InvalidData {
                field: "configuration",
                reason: "has no parent",
            })?,
            paths.euid,
        )?;
        create_product_dir(&paths.data_root, paths.euid)?;
        create_missing_selected_codex_home(
            &paths.codex_home,
            &paths.config,
            &paths.home,
            &paths.data_root,
            &paths.runtime_dir,
            paths.euid,
        )?;
        reconcile_file(&paths.config, &preflight.config, 0o600, paths.euid)
    })()
    .is_err()
    {
        return Err(setup_failed(
            diagnostics,
            SetupStage::Configuration,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::SetupInstallationFilesRetry),
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::Configuration, &context.target)?;

    let _projection_changed = match reconcile_projection(paths, &preflight.projection) {
        Ok(changed) => changed,
        Err(_) => {
            return Err(setup_failed(
                diagnostics,
                SetupStage::Projection,
                DiagnosticCause::CliIntegration,
                UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationRetry),
            ));
        }
    };
    complete_setup_stage(diagnostics, SetupStage::Projection, &context.target)?;

    let _marketplace_changed =
        match reconcile_marketplace(&preflight.codex, &paths.codex_home, &paths.marketplace) {
            Ok(changed) => changed,
            Err(error) => {
                return Err(setup_failed(
                    diagnostics,
                    SetupStage::PluginMarketplace,
                    DiagnosticCause::CliIntegration,
                    setup_cli_reconciliation_failure(&error),
                ));
            }
        };
    complete_setup_stage(diagnostics, SetupStage::PluginMarketplace, &context.target)?;

    let _plugin_changed = match reconcile_plugin(
        &preflight.codex,
        &paths.codex_home,
        &paths.marketplace,
        &context.candidate.product_version,
    ) {
        Ok(changed) => changed,
        Err(error) => {
            return Err(setup_failed(
                diagnostics,
                SetupStage::PluginInstall,
                DiagnosticCause::CliIntegration,
                setup_cli_reconciliation_failure(&error),
            ));
        }
    };
    complete_setup_stage(diagnostics, SetupStage::PluginInstall, &context.target)?;

    complete_setup_stage(diagnostics, SetupStage::DesktopDiscovery, &context.target)?;
    let (_desktop_published, desktop_intent_changed) = if let (Some(target), Some(descriptor)) =
        (&preflight.desktop.target, &preflight.desktop.descriptor)
    {
        let published = match publish_descriptor(&target.identity, descriptor) {
            Ok(published) => published,
            Err(failure) => {
                return Err(setup_failed(
                    diagnostics,
                    SetupStage::Descriptor,
                    DiagnosticCause::DesktopIntegration,
                    setup_descriptor_publication_failure(failure),
                ));
            }
        };
        let identity_changed = preflight
            .desktop
            .previous_attachment
            .as_ref()
            .is_some_and(|previous| previous != &target.identity);
        let descriptor_path_changed = preflight
            .desktop
            .previous_attachment
            .as_ref()
            .is_some_and(|previous| previous.descriptor_path != target.identity.descriptor_path);
        if identity_changed && descriptor_path_changed {
            let previous = preflight
                .desktop
                .previous_attachment
                .as_ref()
                .expect("replacement has a previous Desktop attachment");
            #[cfg(test)]
            if context.target.test_hooks.force_old_descriptor_removal_race
                && fs::write(
                    &previous.descriptor_path,
                    b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/descriptor-removal-race\"}",
                )
                .and_then(|()| {
                    fs::set_permissions(
                        &previous.descriptor_path,
                        fs::Permissions::from_mode(0o600),
                    )
                })
                .is_err()
            {
                let failure = if published {
                    setup_desktop_rollback(vec![target.identity.descriptor_path.clone()])
                } else {
                    UserFailure::Ordinary(OrdinaryFailure::SetupDesktopIntegrationRetry)
                };
                return Err(setup_failed(
                    diagnostics,
                    SetupStage::Descriptor,
                    DiagnosticCause::DesktopIntegration,
                    failure,
                ));
            }
            if remove_expected_descriptor(previous, descriptor).is_err() {
                let mut residue = vec![previous.descriptor_path.clone()];
                if published && remove_expected_descriptor(&target.identity, descriptor).is_err() {
                    residue.push(target.identity.descriptor_path.clone());
                }
                return Err(setup_failed(
                    diagnostics,
                    SetupStage::Descriptor,
                    DiagnosticCause::DesktopIntegration,
                    setup_desktop_rollback(residue),
                ));
            }
        }
        (published, published || descriptor_path_changed)
    } else {
        (false, false)
    };
    complete_setup_stage(diagnostics, SetupStage::Descriptor, &context.target)?;

    if (|| {
        create_shared_dir(
            paths.unit.parent().ok_or(ControllerError::InvalidData {
                field: "service_unit",
                reason: "has no parent",
            })?,
            paths.euid,
        )?;
        #[cfg(test)]
        if context.target.test_hooks.fail_service_unit_write {
            return Err(ControllerError::Operational(
                "injected service-unit write failure".to_owned(),
            ));
        }
        reconcile_file(&paths.unit, &preflight.unit, 0o644, paths.euid)
    })()
    .is_err()
    {
        return Err(fail_after_descriptor_publication(
            diagnostics,
            SetupStage::ServiceUnit,
            DiagnosticCause::ServiceConfiguration,
            RollbackPrimary::SetupServiceConfigurationRetry,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::ServiceUnit, &context.target)?;
    let running_clients =
        detect_running_unattached_clients(&context.target.client_process_source, paths.euid);

    if run_systemctl(&preflight.systemctl, ["--user", "daemon-reload"]).is_err() {
        return Err(fail_after_descriptor_publication(
            diagnostics,
            SetupStage::DaemonReload,
            DiagnosticCause::ServiceConfiguration,
            RollbackPrimary::SetupServiceConfigurationRetry,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::DaemonReload, &context.target)?;

    if run_systemctl(
        &preflight.systemctl,
        [
            "--user",
            "enable",
            "--now",
            context.target.unit_name.as_str(),
        ],
    )
    .is_err()
    {
        return Err(fail_after_descriptor_publication(
            diagnostics,
            SetupStage::ServiceEnable,
            DiagnosticCause::ServiceStart,
            RollbackPrimary::SetupServiceStartRetry,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::ServiceEnable, &context.target)?;

    if verify_setup_service(
        &preflight.systemctl,
        &context.target,
        &preflight.expected_running_version,
    )
    .await
    .is_err()
    {
        return Err(fail_after_descriptor_publication(
            diagnostics,
            SetupStage::ServiceVerify,
            DiagnosticCause::ServiceState,
            RollbackPrimary::SetupServiceStateRetryUpdate,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::ServiceVerify, &context.target)?;

    let manifest = InstalledRelease {
        schema_version: 3,
        product_version: context.candidate.product_version.clone(),
        target: context.candidate.target.clone(),
        binary_sha256: preflight.binary_sha256,
        service_unit_sha256: preflight.unit_sha256,
        projection_sha256: preflight.projection.sha256.clone(),
        plugin_version: context.candidate.product_version.clone(),
        codex_executable: preflight.codex,
        codex_home: paths.codex_home.clone(),
        socket_path: paths.socket.clone(),
        desktop_attachment: preflight.desktop.attachment.clone(),
    };
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|_| {
        setup_failed(
            diagnostics,
            SetupStage::Manifest,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::SetupInstallationFilesRetry),
        )
    })?;
    manifest_bytes.push(b'\n');
    if reconcile_file(&paths.manifest, &manifest_bytes, 0o600, paths.euid).is_err() {
        return Err(setup_failed(
            diagnostics,
            SetupStage::Manifest,
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::SetupInstallationFilesRetry),
        ));
    }
    complete_setup_stage(diagnostics, SetupStage::Manifest, &context.target)?;

    let mut notices = Vec::new();
    if let Some(codex) = preflight.compatibility_codex_version {
        notices.push(UserNotice::Compatibility {
            codex,
            product: semver::Version::parse(&context.candidate.product_version)
                .expect("validated candidate version is semantic"),
        });
    }
    if preflight.desktop.warning.is_some() {
        notices.push(UserNotice::DesktopLauncherUnavailable);
    }
    if !install_bin_on_path(&context) {
        notices.push(UserNotice::LocalBinMissingFromPath {
            local_bin: paths.home.join(".local/bin"),
        });
    }
    let desktop = match preflight.desktop.status {
        DesktopAttachmentStatus::Available => OutputDesktopAvailability::Available,
        DesktopAttachmentStatus::Unavailable => OutputDesktopAvailability::Unavailable,
        DesktopAttachmentStatus::Unverified => OutputDesktopAvailability::CouldNotVerify,
    };
    let success = SetupSuccess::new(
        semver::Version::parse(&context.candidate.product_version)
            .expect("validated candidate version is semantic"),
        running_clients,
        desktop,
        desktop_intent_changed,
        notices,
    )
    .expect("setup Desktop evidence forms a valid output state");
    Ok(UserSuccess::Setup(success))
}

pub(super) async fn setup_preflight(
    context: &mut SetupContext,
) -> Result<SetupPreflight, UserFailure> {
    let evidence = classify_selected_home_evidence(&context.target.paths);
    SelectedHomeOperation::Setup
        .require_permitted_case(evidence.case)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupInstalledStateCheckStatus))?;
    let codex = evidence
        .configuration
        .as_ref()
        .map(|configuration| configuration.codex_executable.clone())
        .or_else(|| {
            evidence
                .manifest
                .as_ref()
                .map(|manifest| manifest.codex_executable.clone())
        })
        .map(Ok)
        .unwrap_or_else(|| resolve_codex_executable(&context.path_environment, &context.cwd))
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationRetry))?;
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupServiceConfigurationRetry))?;
    let native = resolve_setup_selected_home(&mut context.target.paths, &codex)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationCheckStatus))?;
    let paths = &context.target.paths;
    require_selected_home_evidence(paths, SelectedHomeOperation::Setup)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupInstalledStateCheckStatus))?;
    if !context.candidate.executable.is_absolute()
        || !context.cwd.is_absolute()
        || context.candidate.product_version.is_empty()
        || context.candidate.target != product_target()
    {
        return Err(UserFailure::Ordinary(
            OrdinaryFailure::SetupInstallationFilesRetry,
        ));
    }
    let (codex_version, expected_running_version) =
        read_codex_version(&codex, &paths.codex_home)
            .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationRetry))?;
    let binary = fs::read(&context.candidate.executable)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupInstallationFilesRetry))?;
    let binary_sha256 = sha256_bytes(&binary);
    let config = ProductConfig {
        schema_version: 2,
        codex_executable: codex.clone(),
        codex_home: paths.codex_home.clone(),
        socket_path: paths.socket.clone(),
    };
    let config = toml::to_string(&config)
        .map(String::into_bytes)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupInstallationFilesRetry))?;
    let projection = render_projection(&paths.binary, &context.candidate.product_version)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationRetry))?;
    let unit = render_unit(paths, &codex)
        .map_err(|_| UserFailure::Ordinary(OrdinaryFailure::SetupServiceConfigurationRetry))?;
    let unit_sha256 = sha256_bytes(&unit);

    validate_manifestless_setup_artifacts(context, &binary, &config, &projection, &unit, &native)?;
    let compatibility_codex_version =
        (expected_running_version != TESTED_CODEX_VERSION).then(|| {
            semver::Version::parse(&codex_version).unwrap_or_else(|_| {
                semver::Version::parse(UNKNOWN_CODEX_VERSION)
                    .expect("unknown Codex version sentinel is semantic")
            })
        });
    let desktop = resolve_setup_desktop(context, &evidence)
        .await
        .map_err(UserFailure::Ordinary)?;
    Ok(SetupPreflight {
        codex,
        systemctl,
        expected_running_version,
        compatibility_codex_version,
        binary,
        binary_sha256,
        config,
        projection,
        unit,
        unit_sha256,
        desktop,
    })
}

async fn resolve_setup_desktop(
    context: &SetupContext,
    evidence: &SelectedHomeEvidence,
) -> Result<SetupDesktopPlan, OrdinaryFailure> {
    let previous = evidence
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.desktop_attachment.clone());
    let availability = if let Some(launcher) = context.desktop_launcher.as_deref() {
        probe_desktop_capability(Some(launcher), &context.desktop_environment)
            .await
            .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationRetry)?
    } else if let Some(identity) = previous.as_ref() {
        probe_persisted_desktop_capability(identity, &context.desktop_environment)
            .await
            .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationRetry)?
    } else {
        probe_desktop_capability(None, &context.desktop_environment)
            .await
            .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationRetry)?
    };
    match availability {
        DesktopAvailability::Verified(target) => {
            let descriptor = render_descriptor(&context.target.paths.socket)
                .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationRetry)?;
            preflight_descriptor_switch(previous.as_ref(), &target.identity, &descriptor)
                .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationCheckStatus)?;
            Ok(SetupDesktopPlan {
                attachment: Some(target.identity.clone()),
                previous_attachment: previous,
                target: Some(target),
                descriptor: Some(descriptor),
                status: DesktopAttachmentStatus::Available,
                warning: None,
            })
        }
        DesktopAvailability::Unavailable { warning } => {
            if let Some(identity) = previous.as_ref() {
                let expected = render_descriptor(&context.target.paths.socket)
                    .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationRetry)?;
                if !matches!(
                    inspect_descriptor(identity, &expected)
                        .map_err(|_| OrdinaryFailure::SetupDesktopIntegrationCheckStatus)?,
                    DescriptorState::Absent | DescriptorState::Expected
                ) {
                    return Err(OrdinaryFailure::SetupDesktopIntegrationCheckStatus);
                }
            }
            Ok(SetupDesktopPlan {
                attachment: previous.clone(),
                previous_attachment: previous.clone(),
                target: None,
                descriptor: None,
                status: if previous.is_some() {
                    DesktopAttachmentStatus::Unverified
                } else {
                    DesktopAttachmentStatus::Unavailable
                },
                warning: Some(warning),
            })
        }
    }
}

fn fail_after_descriptor_publication(
    diagnostics: &mut Diagnostics,
    stage: SetupStage,
    cause: DiagnosticCause,
    primary: RollbackPrimary,
    descriptor_intent_changed: bool,
    preflight: &SetupPreflight,
    target: &LifecycleTarget,
) -> UserFailure {
    let cleanup = descriptor_intent_changed
        .then(|| {
            cleanup_changed_descriptor_after_start_failure(
                &preflight.systemctl,
                target,
                preflight.desktop.target.as_ref(),
                preflight.desktop.descriptor.as_deref(),
            )
        })
        .transpose();
    let failure = match cleanup {
        Err(cleanup) => {
            let residue = cleanup
                .residue
                .expect("changed descriptor cleanup failure has exact final residue");
            UserFailure::RollbackIncomplete(RollbackIncomplete::new(
                primary,
                ManagedPaths::new(descriptor_residue_path(residue), Vec::new()),
            ))
        }
        Ok(_) => UserFailure::Ordinary(match primary {
            RollbackPrimary::SetupServiceConfigurationRetry => {
                OrdinaryFailure::SetupServiceConfigurationRetry
            }
            RollbackPrimary::SetupServiceStartRetry => OrdinaryFailure::SetupServiceStartRetry,
            RollbackPrimary::SetupServiceStateRetryUpdate => {
                OrdinaryFailure::SetupServiceStateRetryUpdate
            }
            _ => unreachable!("post-publication setup uses a setup service primary"),
        }),
    };
    setup_failed(diagnostics, stage, cause, failure)
}

fn validate_manifestless_setup_artifacts(
    context: &SetupContext,
    binary: &[u8],
    config: &[u8],
    projection: &RenderedProjection,
    unit: &[u8],
    native: &NativeProductState,
) -> Result<(), UserFailure> {
    let paths = &context.target.paths;
    let evidence = classify_selected_home_evidence(paths);
    if let Some(expected_manifest) = evidence.manifest.as_ref() {
        let installed_state_failure =
            || UserFailure::Ordinary(OrdinaryFailure::SetupInstalledStateCheckStatus);
        let manifest: InstalledRelease = serde_json::from_slice(
            &read_product_evidence_file(&paths.home, paths.euid, &paths.manifest, 0o600)
                .map_err(|_| installed_state_failure())?,
        )
        .map_err(|_| installed_state_failure())?;
        manifest
            .validate(&paths.codex_home, &paths.socket)
            .map_err(|_| installed_state_failure())?;
        if manifest != *expected_manifest {
            return Err(installed_state_failure());
        }
        if manifest.product_version != context.candidate.product_version
            || manifest.target != context.candidate.target
        {
            return Err(UserFailure::Ordinary(
                OrdinaryFailure::SetupInstallationFilesRetryUpdate,
            ));
        }
        return Ok(());
    }

    let marketplace_file = paths.marketplace.join(".agents/plugins/marketplace.json");
    let plugin_file = paths
        .marketplace
        .join("plugins/codex-session-control/.codex-plugin/plugin.json");
    let mcp_file = paths
        .marketplace
        .join("plugins/codex-session-control/.mcp.json");
    let mut ambiguous: BTreeSet<String> = BTreeSet::new();
    let mut identified_versions: BTreeSet<String> = BTreeSet::new();

    observe_manifestless_binary_artifact(context, binary, &mut ambiguous, &mut identified_versions);
    observe_manifestless_filesystem_artifact(paths, &paths.config, config, &mut ambiguous);
    observe_manifestless_filesystem_artifact(paths, &paths.unit, unit, &mut ambiguous);
    observe_manifestless_filesystem_artifact(
        paths,
        &marketplace_file,
        &projection.marketplace,
        &mut ambiguous,
    );
    observe_manifestless_plugin_artifact(
        paths,
        &plugin_file,
        &projection.plugin,
        &context.candidate.product_version,
        &mut ambiguous,
        &mut identified_versions,
    );
    observe_manifestless_filesystem_artifact(paths, &mcp_file, &projection.mcp, &mut ambiguous);
    observe_manifestless_native_artifacts(
        context,
        native,
        &evidence,
        &mut ambiguous,
        &mut identified_versions,
    );

    if identified_versions.len() > 1 {
        ambiguous.insert("conflicting release identities".to_owned());
    }
    if ambiguous.is_empty()
        && identified_versions.len() == 1
        && !identified_versions.contains(&context.candidate.product_version)
    {
        let version = identified_versions.into_iter().next().unwrap();
        let failure = if read_installed_product_version(&paths.binary, paths.euid).as_deref()
            == Some(version.as_str())
        {
            UserFailure::Ordinary(OrdinaryFailure::SetupInstalledStateRepair {
                binary: paths.binary.clone(),
            })
        } else {
            let version = semver::Version::parse(&version).map_err(|_| {
                UserFailure::Ordinary(OrdinaryFailure::SetupInstalledStateCheckStatus)
            })?;
            UserFailure::VerifiedRelease(crate::cli_output::VerifiedReleaseRecovery::new(
                format!(
                    "https://github.com/{RELEASE_REPOSITORY}/releases/download/v{version}/codex-session-control-{}",
                    context.candidate.target,
                ),
                format!(
                    "https://github.com/{RELEASE_REPOSITORY}/releases/download/v{version}/SHA256SUMS"
                ),
            ))
        };
        return Err(failure);
    }
    if identified_versions.contains(&context.candidate.product_version) && !ambiguous.is_empty() {
        ambiguous.insert("conflicting release identities".to_owned());
    }
    if ambiguous.is_empty() {
        Ok(())
    } else {
        Err(UserFailure::Ordinary(
            OrdinaryFailure::SetupInstalledStateCheckStatus,
        ))
    }
}

fn observe_manifestless_filesystem_artifact(
    paths: &ResolvedUserPaths,
    path: &Path,
    expected: &[u8],
    ambiguous: &mut BTreeSet<String>,
) {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let safe = validate_existing(path, FileKind::RegularFile, paths.euid).is_ok();
            let matches = safe && fs::read(path).is_ok_and(|found| found.as_slice() == expected);
            if !matches {
                ambiguous.insert(path.display().to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            ambiguous.insert(path.display().to_string());
        }
    }
}

fn observe_manifestless_binary_artifact(
    context: &SetupContext,
    expected: &[u8],
    ambiguous: &mut BTreeSet<String>,
    identified_versions: &mut BTreeSet<String>,
) {
    let paths = &context.target.paths;
    match fs::symlink_metadata(&paths.binary) {
        Ok(_) => {
            let safe = validate_existing(&paths.binary, FileKind::RegularFile, paths.euid).is_ok();
            let matches =
                safe && fs::read(&paths.binary).is_ok_and(|found| found.as_slice() == expected);
            if matches {
                identified_versions.insert(context.candidate.product_version.clone());
            } else if let Some(version) = read_installed_product_version(&paths.binary, paths.euid)
            {
                identified_versions.insert(version);
            } else {
                ambiguous.insert(paths.binary.display().to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            ambiguous.insert(paths.binary.display().to_string());
        }
    }
}

fn observe_manifestless_plugin_artifact(
    paths: &ResolvedUserPaths,
    path: &Path,
    expected: &[u8],
    candidate_version: &str,
    ambiguous: &mut BTreeSet<String>,
    identified_versions: &mut BTreeSet<String>,
) {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let safe = validate_existing(path, FileKind::RegularFile, paths.euid).is_ok();
            let matches = safe && fs::read(path).is_ok_and(|found| found.as_slice() == expected);
            if matches {
                identified_versions.insert(candidate_version.to_owned());
            } else if let Some(version) = fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|plugin| {
                    plugin
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            {
                identified_versions.insert(version);
            } else {
                ambiguous.insert(path.display().to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            ambiguous.insert(path.display().to_string());
        }
    }
}

fn observe_manifestless_native_artifacts(
    context: &SetupContext,
    native: &NativeProductState,
    evidence: &SelectedHomeEvidence,
    ambiguous: &mut BTreeSet<String>,
    identified_versions: &mut BTreeSet<String>,
) {
    let paths = &context.target.paths;
    let roots = &native.marketplace_roots;
    if !roots.is_empty() && (roots.len() != 1 || roots[0] != paths.marketplace.to_string_lossy()) {
        ambiguous.insert("native marketplace codex-session-control-local".to_owned());
    }
    if evidence.native_product_residue.is_present()
        && evidence.case == InstalledEvidenceCase::PartialArtifactsWithoutIdentity
    {
        identified_versions.insert(context.candidate.product_version.clone());
    }
    let expected_source = paths
        .marketplace
        .join("plugins/codex-session-control")
        .to_string_lossy()
        .into_owned();
    let product = &native.plugins;
    if !product.is_empty()
        && (product.len() != 1
            || !plugin_matches(
                &product[0],
                &context.candidate.product_version,
                &expected_source,
            ))
    {
        for plugin in product {
            if let Some(version) = plugin.get("version").and_then(Value::as_str) {
                identified_versions.insert(version.to_owned());
            }
        }
        if identified_versions.is_empty() {
            ambiguous.insert(
                "native plugin codex-session-control@codex-session-control-local".to_owned(),
            );
        }
    }
}

fn install_bin_on_path(context: &SetupContext) -> bool {
    let install_bin = context.target.paths.home.join(".local/bin");
    std::env::split_paths(&context.path_environment).any(|entry| entry == install_bin)
}
