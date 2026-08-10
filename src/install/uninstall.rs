use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::{
    cli_output::{
        IndependentTerminal, ManagedPaths, ManualCleanup, NativeCleanupCommand, OrdinaryFailure,
        RollbackIncomplete, RollbackPrimary, StopThenRetry, TerminalPartialUninstall,
        UninstallSuccess, UserFailure, UserSuccess,
    },
    diagnostics::{DiagnosticCause, DiagnosticEvent, DiagnosticTarget, Diagnostics},
};

use super::{
    LifecycleContext, LifecycleTarget,
    evidence::{
        ResolvedUserPaths, SelectedHomeOperation, classify_selected_home_evidence,
        require_selected_home_evidence,
    },
    lifecycle_context,
    native::{
        self, remove_native_marketplace_if_present, remove_native_plugin_if_present,
        resolve_named_executable,
    },
    paths::{remove_owned_empty_dir, remove_owned_file, remove_owned_tree},
    remove_persisted_desktop_descriptor,
    service::{
        CallerUnitEvidence, CallerUnitInspection, ServiceActivity, inspect_caller_unit,
        query_service_activity, run_systemctl, verify_absent_managed_unit_stop,
        verify_disabled_service,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UninstallStage {
    ServiceStop,
    ServiceStopVerify,
    DescriptorRemove,
    ServiceUnitRemove,
    PluginRemove,
    MarketplaceRemove,
    ProjectionRemove,
    ConfigurationRemove,
    ManifestRemove,
    BinaryRemove,
}

impl UninstallStage {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::ServiceStop => "service-stop",
            Self::ServiceStopVerify => "service-stop-verify",
            Self::DescriptorRemove => "descriptor-remove",
            Self::ServiceUnitRemove => "service-unit-remove",
            Self::PluginRemove => "plugin-remove",
            Self::MarketplaceRemove => "marketplace-remove",
            Self::ProjectionRemove => "projection-remove",
            Self::ConfigurationRemove => "configuration-remove",
            Self::ManifestRemove => "manifest-remove",
            Self::BinaryRemove => "binary-remove",
        }
    }
}

fn rollback_failure(primary: RollbackPrimary, first: PathBuf, rest: Vec<PathBuf>) -> UserFailure {
    UserFailure::RollbackIncomplete(RollbackIncomplete::new(
        primary,
        ManagedPaths::new(first, rest),
    ))
}

fn installed_state_failure(paths: &ResolvedUserPaths) -> UserFailure {
    rollback_failure(
        RollbackPrimary::UninstallInstalledStateCheckStatus,
        paths.config.clone(),
        vec![paths.manifest.clone()],
    )
}

fn cleanup_failure(path: PathBuf) -> UserFailure {
    rollback_failure(RollbackPrimary::UninstallCleanupRetry, path, Vec::new())
}

fn manual_cleanup_failure(
    command: NativeCleanupCommand,
    paths: &ResolvedUserPaths,
    codex: Option<&Path>,
) -> UserFailure {
    let codex_executable = codex
        .filter(|path| native::valid_owned_executable(path, paths.euid))
        .and_then(|path| {
            super::shell_quote_path(path)
                .ok()
                .map(|_| path.to_path_buf())
        });
    UserFailure::ManualCleanup(ManualCleanup::new(
        command,
        paths.codex_home.clone(),
        codex_executable,
    ))
}

fn complete_uninstall_stage(
    diagnostics: &mut Diagnostics,
    stage: UninstallStage,
    _target: &LifecycleTarget,
) -> Result<(), UserFailure> {
    diagnostics.completed(stage.diagnostic_name());
    #[cfg(test)]
    if _target.test_hooks.fail_after_completed_stage == Some(stage.diagnostic_name()) {
        let failure = if stage == UninstallStage::ServiceStop {
            UserFailure::StopThenRetry(StopThenRetry::UninstallServiceStateStopThenUninstall)
        } else {
            UserFailure::Ordinary(OrdinaryFailure::UninstallUnexpectedRetry)
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

pub(crate) async fn uninstall(
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
            UninstallStage::ServiceStop.diagnostic_name(),
            DiagnosticCause::Validation,
            UserFailure::Ordinary(OrdinaryFailure::UninstallUnexpectedRetry),
        )
    })?;
    uninstall_with_context_and_diagnostics(context, diagnostics).await
}

#[cfg(test)]
pub(super) async fn uninstall_with_context(
    context: LifecycleContext,
) -> Result<UserSuccess, UserFailure> {
    let mut diagnostics = Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Uninstall);
    uninstall_with_context_and_diagnostics(context, &mut diagnostics).await
}

pub(super) async fn uninstall_with_context_and_diagnostics(
    context: LifecycleContext,
    diagnostics: &mut Diagnostics,
) -> Result<UserSuccess, UserFailure> {
    let paths = &context.target.paths;
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ServiceStop.diagnostic_name(),
                DiagnosticCause::ServiceStop,
                UserFailure::Ordinary(OrdinaryFailure::UninstallServiceStopRetry),
            )
        })?;

    match query_service_activity(&systemctl, &context.target.unit_name) {
        ServiceActivity::Inactive => {}
        ServiceActivity::Active => match inspect_caller_unit(&systemctl, &context.target) {
            CallerUnitInspection::Independent => {}
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::WhoAmI) => {
                return Err(super::fail_with_diagnostic(
                    diagnostics,
                    UninstallStage::ServiceStop.diagnostic_name(),
                    DiagnosticCause::Validation,
                    UserFailure::IndependentTerminal(IndependentTerminal::Uninstall),
                ));
            }
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::ControlGroup)
            | CallerUnitInspection::Unknown { .. } => {
                return Err(super::fail_with_diagnostic(
                    diagnostics,
                    UninstallStage::ServiceStop.diagnostic_name(),
                    DiagnosticCause::Validation,
                    UserFailure::StopThenRetry(StopThenRetry::UninstallUnsafeStopThenUninstall),
                ));
            }
        },
        ServiceActivity::Unproven => {
            return Err(super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ServiceStop.diagnostic_name(),
                DiagnosticCause::ServiceState,
                UserFailure::StopThenRetry(StopThenRetry::UninstallUnsafeStopThenUninstall),
            ));
        }
    }

    let stopped_as_absent_unit = match run_systemctl(
        &systemctl,
        [
            "--user",
            "disable",
            "--now",
            context.target.unit_name.as_str(),
        ],
    ) {
        Ok(()) => false,
        Err(_) => {
            verify_absent_managed_unit_stop(&systemctl, &context.target).map_err(|_| {
                super::fail_with_diagnostic(
                    diagnostics,
                    UninstallStage::ServiceStop.diagnostic_name(),
                    DiagnosticCause::ServiceState,
                    UserFailure::StopThenRetry(
                        StopThenRetry::UninstallServiceStateStopThenUninstall,
                    ),
                )
            })?;
            true
        }
    };
    complete_uninstall_stage(diagnostics, UninstallStage::ServiceStop, &context.target)?;

    if !stopped_as_absent_unit {
        verify_disabled_service(&systemctl, &context.target).map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ServiceStopVerify.diagnostic_name(),
                DiagnosticCause::ServiceState,
                UserFailure::StopThenRetry(StopThenRetry::UninstallServiceStateStopThenUninstall),
            )
        })?;
    }
    complete_uninstall_stage(
        diagnostics,
        UninstallStage::ServiceStopVerify,
        &context.target,
    )?;

    let evidence = require_selected_home_evidence(paths, SelectedHomeOperation::Uninstall)
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::DescriptorRemove.diagnostic_name(),
                DiagnosticCause::Validation,
                installed_state_failure(paths),
            )
        })?;
    let descriptor_path = evidence
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.desktop_attachment.as_ref())
        .map(|identity| identity.descriptor_path.clone());
    let desktop_intent_removed =
        remove_persisted_desktop_descriptor(paths, &evidence).map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::DescriptorRemove.diagnostic_name(),
                DiagnosticCause::DesktopIntegration,
                rollback_failure(
                    RollbackPrimary::UninstallDesktopCheckStatus,
                    descriptor_path
                        .clone()
                        .unwrap_or_else(|| paths.manifest.clone()),
                    Vec::new(),
                ),
            )
        })?;
    complete_uninstall_stage(
        diagnostics,
        UninstallStage::DescriptorRemove,
        &context.target,
    )?;

    remove_owned_file(&paths.unit, paths.euid, 0o644)
        .and_then(|()| run_systemctl(&systemctl, ["--user", "daemon-reload"]))
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ServiceUnitRemove.diagnostic_name(),
                DiagnosticCause::Cleanup,
                cleanup_failure(paths.unit.clone()),
            )
        })?;
    complete_uninstall_stage(
        diagnostics,
        UninstallStage::ServiceUnitRemove,
        &context.target,
    )?;

    let codex = cleanup_codex_executable(paths, &context.path_environment, &context.cwd);
    let codex = codex.ok_or_else(|| {
        super::fail_with_diagnostic(
            diagnostics,
            UninstallStage::PluginRemove.diagnostic_name(),
            DiagnosticCause::CliIntegration,
            manual_cleanup_failure(NativeCleanupCommand::RemovePlugin, paths, None),
        )
    })?;
    remove_native_plugin_if_present(&codex, &paths.codex_home).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            UninstallStage::PluginRemove.diagnostic_name(),
            DiagnosticCause::CliIntegration,
            manual_cleanup_failure(NativeCleanupCommand::RemovePlugin, paths, Some(&codex)),
        )
    })?;
    complete_uninstall_stage(diagnostics, UninstallStage::PluginRemove, &context.target)?;

    remove_native_marketplace_if_present(&codex, &paths.codex_home).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            UninstallStage::MarketplaceRemove.diagnostic_name(),
            DiagnosticCause::CliIntegration,
            manual_cleanup_failure(NativeCleanupCommand::RemoveMarketplace, paths, Some(&codex)),
        )
    })?;
    complete_uninstall_stage(
        diagnostics,
        UninstallStage::MarketplaceRemove,
        &context.target,
    )?;

    remove_owned_tree(&paths.marketplace, paths.euid).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            UninstallStage::ProjectionRemove.diagnostic_name(),
            DiagnosticCause::Cleanup,
            cleanup_failure(paths.marketplace.clone()),
        )
    })?;
    complete_uninstall_stage(
        diagnostics,
        UninstallStage::ProjectionRemove,
        &context.target,
    )?;

    let config_root = paths
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.config.clone());
    remove_owned_file(&paths.config, paths.euid, 0o600)
        .and_then(|()| remove_owned_empty_dir(&config_root, paths.euid))
        .map_err(|_| {
            super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ConfigurationRemove.diagnostic_name(),
                DiagnosticCause::Cleanup,
                cleanup_failure(config_root.clone()),
            )
        })?;
    complete_uninstall_stage(
        diagnostics,
        UninstallStage::ConfigurationRemove,
        &context.target,
    )?;

    #[cfg(test)]
    if matches!(
        context.target.test_hooks.fail_after_completed_stage,
        Some("manifest-remove" | "binary-remove")
    ) {
        let stage = if context.target.test_hooks.fail_after_completed_stage
            == Some(UninstallStage::ManifestRemove.diagnostic_name())
        {
            UninstallStage::ManifestRemove
        } else {
            UninstallStage::BinaryRemove
        };
        let failure = if stage == UninstallStage::ManifestRemove {
            cleanup_failure(paths.manifest.clone())
        } else {
            UserFailure::Ordinary(OrdinaryFailure::UninstallUnexpectedRetry)
        };
        return Err(super::fail_with_diagnostic(
            diagnostics,
            stage.diagnostic_name(),
            DiagnosticCause::Unexpected,
            failure,
        ));
    }

    remove_owned_file(&paths.manifest, paths.euid, 0o600).map_err(|_| {
        super::fail_with_diagnostic(
            diagnostics,
            UninstallStage::ManifestRemove.diagnostic_name(),
            DiagnosticCause::Cleanup,
            cleanup_failure(paths.manifest.clone()),
        )
    })?;
    let data_root_error = remove_owned_empty_dir(&paths.data_root, paths.euid).err();
    if data_root_error.is_none() {
        complete_uninstall_stage(diagnostics, UninstallStage::ManifestRemove, &context.target)?;
    }

    let binary_error = remove_owned_file(&paths.binary, paths.euid, 0o755).err();
    match (data_root_error, binary_error) {
        (None, None) => {
            complete_uninstall_stage(diagnostics, UninstallStage::BinaryRemove, &context.target)?
        }
        (None, Some(_)) => {
            return Err(super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::BinaryRemove.diagnostic_name(),
                DiagnosticCause::Cleanup,
                UserFailure::TerminalPartialUninstall(TerminalPartialUninstall::new(
                    ManagedPaths::new(paths.binary.clone(), Vec::new()),
                )),
            ));
        }
        (Some(_), None) => {
            return Err(super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ManifestRemove.diagnostic_name(),
                DiagnosticCause::Cleanup,
                UserFailure::TerminalPartialUninstall(TerminalPartialUninstall::new(
                    ManagedPaths::new(paths.data_root.clone(), Vec::new()),
                )),
            ));
        }
        (Some(_), Some(_)) => {
            return Err(super::fail_with_diagnostic(
                diagnostics,
                UninstallStage::ManifestRemove.diagnostic_name(),
                DiagnosticCause::Cleanup,
                UserFailure::TerminalPartialUninstall(TerminalPartialUninstall::new(
                    ManagedPaths::new(paths.data_root.clone(), vec![paths.binary.clone()]),
                )),
            ));
        }
    }

    Ok(UserSuccess::Uninstall(UninstallSuccess::new(
        desktop_intent_removed,
    )))
}

fn cleanup_codex_executable(
    paths: &ResolvedUserPaths,
    path_environment: &OsStr,
    cwd: &Path,
) -> Option<PathBuf> {
    let evidence = classify_selected_home_evidence(paths);
    native::cleanup_codex_executable(
        evidence
            .configuration
            .map(|configuration| configuration.codex_executable),
        evidence.manifest.map(|manifest| manifest.codex_executable),
        path_environment,
        cwd,
    )
}
