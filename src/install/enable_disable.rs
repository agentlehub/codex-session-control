use std::{collections::BTreeMap, ffi::OsString, path::Path};

use crate::{
    app_server::TESTED_CODEX_VERSION,
    cli_output::{
        DesktopAvailability as OutputDesktopAvailability, DisableSuccess, EnableSuccess,
        IndependentTerminal, ManagedPaths, OrdinaryFailure, PartialDisable, RollbackIncomplete,
        RollbackPrimary, StopThenRetry, UserFailure, UserNotice, UserSuccess,
    },
    desktop::{
        DescriptorPublicationFailure, DescriptorState, DesktopAvailability, DesktopTarget,
        inspect_descriptor, preflight_descriptor_switch, probe_persisted_desktop_capability,
        publish_descriptor, render_descriptor,
    },
    diagnostics::{DiagnosticCause, DiagnosticEvent, DiagnosticTarget, Diagnostics},
    error::ControllerError,
    model::{InstalledRelease, ProductConfig},
};

use super::{
    DesktopAttachmentStatus, LifecycleContext, LifecycleDesktopPlan, LifecycleTarget,
    UNKNOWN_CODEX_VERSION, cleanup_changed_descriptor_after_start_failure,
    evidence::{ResolvedUserPaths, SelectedHomeOperation, require_selected_home_evidence},
    lifecycle_context,
    native::{read_codex_version, resolve_named_executable},
    paths::{read_product_evidence_file, read_status_file},
    remove_persisted_desktop_descriptor,
    render::render_unit,
    service::{
        CallerUnitEvidence, CallerUnitInspection, ServiceActivity,
        detect_running_unattached_clients, inspect_caller_unit, query_service_activity,
        run_systemctl, verify_disabled_service, verify_enabled_service,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecycleStage {
    Configuration,
    ServiceUnit,
    Descriptor,
    DescriptorRemove,
    ServiceEnable,
    ServiceDisable,
    ServiceVerify,
}

impl LifecycleStage {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::ServiceUnit => "service-unit",
            Self::Descriptor => "descriptor",
            Self::DescriptorRemove => "descriptor-remove",
            Self::ServiceEnable => "service-enable",
            Self::ServiceDisable => "service-disable",
            Self::ServiceVerify => "service-verify",
        }
    }
}

pub(super) fn enable_context_failure() -> UserFailure {
    UserFailure::Ordinary(OrdinaryFailure::EnableUnexpectedRetry)
}

pub(super) fn disable_context_failure() -> UserFailure {
    UserFailure::Ordinary(OrdinaryFailure::DisableUnexpectedRetry)
}

pub(super) fn enable_publication_failure(failure: DescriptorPublicationFailure) -> UserFailure {
    match failure.residue {
        Some(residue) => UserFailure::RollbackIncomplete(RollbackIncomplete::new(
            RollbackPrimary::EnableDesktopRetry,
            ManagedPaths::new(residue.into_path(), Vec::new()),
        )),
        None => UserFailure::Ordinary(OrdinaryFailure::EnableDesktopIntegrationRetry),
    }
}

pub(super) fn enable_service_failure(
    primary: StopThenRetry,
    descriptor_changed: bool,
    cleanup: Result<(), DescriptorPublicationFailure>,
) -> UserFailure {
    if !descriptor_changed {
        return UserFailure::StopThenRetry(primary);
    }
    match cleanup {
        Ok(()) => UserFailure::Ordinary(match primary {
            StopThenRetry::EnableServiceStartStopThenEnable => {
                OrdinaryFailure::EnableServiceStartRetry
            }
            StopThenRetry::EnableServiceStateStopThenEnable => {
                OrdinaryFailure::EnableServiceStateRetry
            }
            _ => unreachable!("enable cleanup receives an enable primary"),
        }),
        Err(failure) => {
            let residue = failure
                .residue
                .expect("changed descriptor cleanup failure has exact residue");
            UserFailure::RollbackIncomplete(RollbackIncomplete::new(
                RollbackPrimary::EnableServiceStateCheckStatus,
                ManagedPaths::new(residue.into_path(), Vec::new()),
            ))
        }
    }
}

fn complete_enable_stage(
    diagnostics: &mut Diagnostics,
    stage: LifecycleStage,
    _target: &LifecycleTarget,
) -> Result<(), UserFailure> {
    diagnostics.completed(stage.diagnostic_name());
    #[cfg(test)]
    if _target.test_hooks.fail_after_completed_stage == Some(stage.diagnostic_name()) {
        let failure = match stage {
            LifecycleStage::Descriptor => {
                UserFailure::Ordinary(OrdinaryFailure::EnableUnexpectedRetry)
            }
            LifecycleStage::ServiceEnable => {
                UserFailure::StopThenRetry(StopThenRetry::EnableServiceStateStopThenEnable)
            }
            LifecycleStage::ServiceVerify => {
                UserFailure::Ordinary(OrdinaryFailure::EnableUnexpectedCheckStatus)
            }
            _ => unreachable!("enable does not complete this injected stage"),
        };
        return Err(super::fail_with_diagnostic(
            diagnostics,
            stage.diagnostic_name(),
            DiagnosticCause::Unexpected,
            failure,
        ));
    }
    Ok(())
}

fn complete_disable_stage(
    diagnostics: &mut Diagnostics,
    stage: LifecycleStage,
    _target: &LifecycleTarget,
) -> Result<(), UserFailure> {
    diagnostics.completed(stage.diagnostic_name());
    #[cfg(test)]
    if _target.test_hooks.fail_after_completed_stage == Some(stage.diagnostic_name()) {
        let failure = match stage {
            LifecycleStage::ServiceDisable => {
                UserFailure::StopThenRetry(StopThenRetry::DisableServiceStopThenDisable)
            }
            LifecycleStage::ServiceVerify => UserFailure::PartialDisable(PartialDisable::new(None)),
            LifecycleStage::DescriptorRemove => {
                UserFailure::Ordinary(OrdinaryFailure::DisableUnexpectedCheckStatus)
            }
            _ => unreachable!("disable does not complete this injected stage"),
        };
        return Err(super::fail_with_diagnostic(
            diagnostics,
            stage.diagnostic_name(),
            DiagnosticCause::Unexpected,
            failure,
        ));
    }
    Ok(())
}

pub(crate) async fn enable(
    target: LifecycleTarget,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    diagnostics.emit(DiagnosticEvent::ControllerStarted {
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("package version is semantic"),
        target: DiagnosticTarget::current(),
    });
    let context = lifecycle_context(target).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::Configuration.diagnostic_name(),
            DiagnosticCause::Validation,
            enable_context_failure(),
        )
    })?;
    enable_with_context_and_diagnostics(context, diagnostics).await
}

pub(crate) async fn disable(
    target: LifecycleTarget,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    diagnostics.emit(DiagnosticEvent::ControllerStarted {
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("package version is semantic"),
        target: DiagnosticTarget::current(),
    });
    let context = lifecycle_context(target).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceDisable.diagnostic_name(),
            DiagnosticCause::Validation,
            disable_context_failure(),
        )
    })?;
    disable_with_context_and_diagnostics(context, diagnostics).await
}

#[cfg(test)]
pub(super) async fn enable_with_context(
    context: LifecycleContext,
) -> Result<UserSuccess, UserFailure> {
    let mut diagnostics = Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Enable);
    enable_with_context_and_diagnostics(context, &mut diagnostics).await
}

pub(super) async fn enable_with_context_and_diagnostics(
    context: LifecycleContext,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    let paths = &context.target.paths;
    let evidence =
        require_selected_home_evidence(paths, SelectedHomeOperation::Enable).map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::Configuration.diagnostic_name(),
                DiagnosticCause::Validation,
                UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
            )
        })?;
    let expected_config = evidence.configuration.ok_or_else(|| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::Configuration.diagnostic_name(),
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
        )
    })?;
    let manifest = evidence.manifest.as_ref().ok_or_else(|| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::Configuration.diagnostic_name(),
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
        )
    })?;
    let config_bytes = read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600)
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::Configuration.diagnostic_name(),
                DiagnosticCause::Validation,
                UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
            )
        })?;
    let config = std::str::from_utf8(&config_bytes)
        .ok()
        .and_then(|text| toml::from_str::<ProductConfig>(text).ok())
        .filter(|config| config.validate(&paths.codex_home, &paths.socket).is_ok())
        .ok_or_else(|| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::Configuration.diagnostic_name(),
                DiagnosticCause::Validation,
                UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
            )
        })?;
    if config != expected_config {
        return Err(super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::Configuration.diagnostic_name(),
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
        ));
    }

    let unit = read_status_file(&paths.unit, paths.euid, 0o644).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceUnit.diagnostic_name(),
            DiagnosticCause::ServiceConfiguration,
            UserFailure::Ordinary(OrdinaryFailure::EnableServiceConfigurationRepairSetup),
        )
    })?;
    let expected_unit = render_unit(paths, &config.codex_executable).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceUnit.diagnostic_name(),
            DiagnosticCause::ServiceConfiguration,
            UserFailure::Ordinary(OrdinaryFailure::EnableServiceConfigurationRepairSetup),
        )
    })?;
    if unit != expected_unit {
        return Err(super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceUnit.diagnostic_name(),
            DiagnosticCause::ServiceConfiguration,
            UserFailure::Ordinary(OrdinaryFailure::EnableServiceConfigurationRepairSetup),
        ));
    }

    let desktop = resolve_enable_desktop(manifest, paths, &context.desktop_environment)
        .await
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::Descriptor.diagnostic_name(),
                DiagnosticCause::DesktopIntegration,
                UserFailure::Ordinary(OrdinaryFailure::EnableDesktopIntegrationCheckStatus),
            )
        })?;
    let (codex_version, expected_running_version) =
        read_codex_version(&config.codex_executable, &paths.codex_home).map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::Configuration.diagnostic_name(),
                DiagnosticCause::CliIntegration,
                UserFailure::Ordinary(OrdinaryFailure::EnableInstalledStateRepairSetup),
            )
        })?;
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::ServiceEnable.diagnostic_name(),
                DiagnosticCause::ServiceStart,
                UserFailure::Ordinary(OrdinaryFailure::EnableServiceStartRetry),
            )
        })?;
    let desktop_published = if let (Some(target), Some(descriptor)) =
        (&desktop.target, &desktop.descriptor)
    {
        let published = publish_descriptor(&target.identity, descriptor).map_err(|failure| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::Descriptor.diagnostic_name(),
                DiagnosticCause::DesktopIntegration,
                enable_publication_failure(failure),
            )
        })?;
        if published {
            complete_enable_stage(diagnostics, LifecycleStage::Descriptor, &context.target)?;
        }
        published
    } else {
        false
    };
    let running_clients =
        detect_running_unattached_clients(&context.target.client_process_source, paths.euid);
    run_systemctl(
        &systemctl,
        [
            "--user",
            "enable",
            "--now",
            context.target.unit_name.as_str(),
        ],
    )
    .map_err(|_| {
        let cleanup = cleanup_enable_descriptor(
            &systemctl,
            &context.target,
            desktop_published,
            desktop.target.as_ref(),
            desktop.descriptor.as_deref(),
        );
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceEnable.diagnostic_name(),
            DiagnosticCause::ServiceStart,
            enable_service_failure(
                StopThenRetry::EnableServiceStartStopThenEnable,
                desktop_published,
                cleanup,
            ),
        )
    })?;
    complete_enable_stage(diagnostics, LifecycleStage::ServiceEnable, &context.target)?;

    verify_enabled_service(&systemctl, &context.target, &expected_running_version)
        .await
        .map_err(|_| {
            let cleanup = cleanup_enable_descriptor(
                &systemctl,
                &context.target,
                desktop_published,
                desktop.target.as_ref(),
                desktop.descriptor.as_deref(),
            );
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::ServiceVerify.diagnostic_name(),
                DiagnosticCause::ServiceState,
                enable_service_failure(
                    StopThenRetry::EnableServiceStateStopThenEnable,
                    desktop_published,
                    cleanup,
                ),
            )
        })?;
    complete_enable_stage(diagnostics, LifecycleStage::ServiceVerify, &context.target)?;

    let mut notices = Vec::new();
    if expected_running_version != TESTED_CODEX_VERSION {
        notices.push(UserNotice::Compatibility {
            codex: semver::Version::parse(&codex_version).unwrap_or_else(|_| {
                semver::Version::parse(UNKNOWN_CODEX_VERSION)
                    .expect("unknown Codex version sentinel is semantic")
            }),
            product: semver::Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("package version is semantic"),
        });
    }
    if desktop.warning.is_some() {
        notices.push(UserNotice::DesktopLauncherUnavailable);
    }
    let desktop_status = match desktop.status {
        DesktopAttachmentStatus::Available => OutputDesktopAvailability::Available,
        DesktopAttachmentStatus::Unavailable if desktop.setup_required => {
            OutputDesktopAvailability::SetupRequired
        }
        DesktopAttachmentStatus::Unavailable => OutputDesktopAvailability::Unavailable,
        DesktopAttachmentStatus::Unverified => OutputDesktopAvailability::CouldNotVerify,
    };
    let success = EnableSuccess::new(running_clients, desktop_status, desktop_published, notices)
        .expect("enable Desktop evidence forms a valid output state");
    Ok(UserSuccess::Enable(success))
}

fn cleanup_enable_descriptor(
    systemctl: &Path,
    target: &LifecycleTarget,
    descriptor_intent_changed: bool,
    desktop: Option<&DesktopTarget>,
    descriptor: Option<&[u8]>,
) -> Result<(), crate::desktop::DescriptorPublicationFailure> {
    if descriptor_intent_changed {
        cleanup_changed_descriptor_after_start_failure(systemctl, target, desktop, descriptor)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn disable_with_context(
    context: LifecycleContext,
) -> Result<UserSuccess, UserFailure> {
    let mut diagnostics = Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Disable);
    disable_with_context_and_diagnostics(context, &mut diagnostics).await
}

pub(super) async fn disable_with_context_and_diagnostics(
    context: LifecycleContext,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::ServiceDisable.diagnostic_name(),
                DiagnosticCause::ServiceStop,
                UserFailure::Ordinary(OrdinaryFailure::DisableServiceStopRetry),
            )
        })?;
    match query_service_activity(&systemctl, &context.target.unit_name) {
        ServiceActivity::Inactive => {}
        ServiceActivity::Active => match inspect_caller_unit(&systemctl, &context.target) {
            CallerUnitInspection::Independent => {}
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::WhoAmI) => {
                return Err(super::fail_with_diagnostic(
                    diagnostics,
                    LifecycleStage::ServiceDisable.diagnostic_name(),
                    DiagnosticCause::Validation,
                    UserFailure::IndependentTerminal(IndependentTerminal::Disable),
                ));
            }
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::ControlGroup)
            | CallerUnitInspection::Unknown { .. } => {
                return Err(super::fail_with_diagnostic(
                    diagnostics,
                    LifecycleStage::ServiceDisable.diagnostic_name(),
                    DiagnosticCause::Validation,
                    UserFailure::StopThenRetry(StopThenRetry::DisableUnsafeStopThenDisable),
                ));
            }
        },
        ServiceActivity::Unproven => {
            return Err(super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::ServiceDisable.diagnostic_name(),
                DiagnosticCause::ServiceState,
                UserFailure::StopThenRetry(StopThenRetry::DisableUnsafeStopThenDisable),
            ));
        }
    }
    run_systemctl(
        &systemctl,
        [
            "--user",
            "disable",
            "--now",
            context.target.unit_name.as_str(),
        ],
    )
    .map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceDisable.diagnostic_name(),
            DiagnosticCause::ServiceStop,
            UserFailure::StopThenRetry(StopThenRetry::DisableServiceStopThenDisable),
        )
    })?;
    complete_disable_stage(diagnostics, LifecycleStage::ServiceDisable, &context.target)?;

    verify_disabled_service(&systemctl, &context.target).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            LifecycleStage::ServiceVerify.diagnostic_name(),
            DiagnosticCause::ServiceState,
            UserFailure::StopThenRetry(StopThenRetry::DisableServiceStateStopThenDisable),
        )
    })?;
    complete_disable_stage(diagnostics, LifecycleStage::ServiceVerify, &context.target)?;

    let evidence =
        require_selected_home_evidence(&context.target.paths, SelectedHomeOperation::Disable)
            .map_err(|_| {
                super::fail_with_diagnostic(
                    diagnostics,
                    LifecycleStage::DescriptorRemove.diagnostic_name(),
                    DiagnosticCause::Cleanup,
                    UserFailure::PartialDisable(PartialDisable::new(None)),
                )
            })?;
    let managed_path = evidence
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.desktop_attachment.as_ref())
        .map(|identity| identity.descriptor_path.clone());
    let desktop_intent_removed =
        remove_persisted_desktop_descriptor(&context.target.paths, &evidence).map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                LifecycleStage::DescriptorRemove.diagnostic_name(),
                DiagnosticCause::Cleanup,
                UserFailure::PartialDisable(PartialDisable::new(managed_path)),
            )
        })?;
    complete_disable_stage(
        diagnostics,
        LifecycleStage::DescriptorRemove,
        &context.target,
    )?;

    Ok(UserSuccess::Disable(DisableSuccess::new(
        desktop_intent_removed,
    )))
}

async fn resolve_enable_desktop(
    manifest: &InstalledRelease,
    paths: &ResolvedUserPaths,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<LifecycleDesktopPlan, ControllerError> {
    let Some(identity) = manifest.desktop_attachment.as_ref() else {
        return Ok(LifecycleDesktopPlan {
            target: None,
            descriptor: None,
            status: DesktopAttachmentStatus::Unavailable,
            warning: None,
            setup_required: true,
        });
    };
    let descriptor = render_descriptor(&paths.socket)?;
    match inspect_descriptor(identity, &descriptor)? {
        DescriptorState::Absent | DescriptorState::Expected => {}
        DescriptorState::Foreign => {
            return Err(ControllerError::Operational(
                "Desktop descriptor is foreign".to_owned(),
            ));
        }
    }
    match probe_persisted_desktop_capability(identity, environment).await? {
        DesktopAvailability::Verified(target) => {
            preflight_descriptor_switch(Some(identity), &target.identity, &descriptor)?;
            Ok(LifecycleDesktopPlan {
                target: Some(target),
                descriptor: Some(descriptor),
                status: DesktopAttachmentStatus::Available,
                warning: None,
                setup_required: false,
            })
        }
        DesktopAvailability::Unavailable { warning } => Ok(LifecycleDesktopPlan {
            target: None,
            descriptor: None,
            status: DesktopAttachmentStatus::Unverified,
            warning: Some(warning),
            setup_required: false,
        }),
    }
}
