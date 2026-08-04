use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::{app_server::TESTED_CODEX_VERSION, error::ControllerError};

use super::{
    DESKTOP_DETACH_GUIDANCE, LifecycleContext, LifecycleTarget, display_command_for_paths,
    evidence::{
        InstalledEvidenceCase, ResolvedUserPaths, classify_selected_home_evidence,
        require_selected_home_evidence,
    },
    incomplete_descriptor_cleanup, lifecycle_context,
    native::{
        self, read_codex_version, remove_native_marketplace_if_present,
        remove_native_plugin_if_present, resolve_named_executable,
    },
    paths::{remove_owned_empty_dir, remove_owned_file, remove_owned_tree},
    remove_persisted_desktop_descriptor,
    service::{run_systemctl, verify_absent_managed_unit_stop, verify_disabled_service},
};

#[derive(Debug)]
pub(crate) struct UninstallReceipt {
    pub stdout: String,
    pub stderr: String,
}

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
    const fn name(self) -> &'static str {
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

#[derive(Default)]
struct UninstallProgress {
    completed: Vec<UninstallStage>,
}

impl UninstallProgress {
    fn complete(&mut self, stage: UninstallStage) {
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
        stage: UninstallStage,
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

pub(crate) async fn uninstall(
    target: LifecycleTarget,
) -> Result<UninstallReceipt, ControllerError> {
    uninstall_with_context(lifecycle_context(target)?).await
}

pub(super) async fn uninstall_with_context(
    context: LifecycleContext,
) -> Result<UninstallReceipt, ControllerError> {
    let mut progress = UninstallProgress::default();
    let paths = &context.target.paths;
    let display_command = display_command_for_paths(paths, &context.path_environment);
    let retry = format!("retry: {display_command} uninstall\n");
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|error| progress.fail(UninstallStage::ServiceStop, error, &retry))?;

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
        Err(stop_error) => {
            verify_absent_managed_unit_stop(&systemctl, &context.target).map_err(
                |proof_error| {
                    progress.fail(
                        UninstallStage::ServiceStop,
                        format!("{stop_error}; {proof_error}"),
                        &retry,
                    )
                },
            )?;
            true
        }
    };
    complete_lifecycle_stage!(
        progress,
        UninstallStage::ServiceStop,
        context.target,
        &retry
    );

    if !stopped_as_absent_unit {
        verify_disabled_service(&systemctl, &context.target)
            .map_err(|error| progress.fail(UninstallStage::ServiceStopVerify, error, &retry))?;
    }
    complete_lifecycle_stage!(
        progress,
        UninstallStage::ServiceStopVerify,
        context.target,
        &retry
    );

    let evidence = require_selected_home_evidence(
        paths,
        &[
            InstalledEvidenceCase::CoherentV2,
            InstalledEvidenceCase::ManifestOnlyV2,
        ],
        "uninstall",
    )
    .map_err(|error| {
        progress.fail(
            UninstallStage::DescriptorRemove,
            incomplete_descriptor_cleanup(error),
            &retry,
        )
    })?;
    let desktop_intent_removed =
        remove_persisted_desktop_descriptor(paths, &evidence).map_err(|error| {
            progress.fail(
                UninstallStage::DescriptorRemove,
                incomplete_descriptor_cleanup(error),
                &retry,
            )
        })?;
    let retry = if desktop_intent_removed {
        format!("{DESKTOP_DETACH_GUIDANCE}{retry}")
    } else {
        retry
    };
    complete_lifecycle_stage!(
        progress,
        UninstallStage::DescriptorRemove,
        context.target,
        &retry
    );

    remove_owned_file(&paths.unit, paths.euid, 0o644)
        .and_then(|()| run_systemctl(&systemctl, ["--user", "daemon-reload"]))
        .map_err(|error| progress.fail(UninstallStage::ServiceUnitRemove, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UninstallStage::ServiceUnitRemove,
        context.target,
        &retry
    );

    let codex = cleanup_codex_executable(paths, &context.path_environment, &context.cwd);
    let compatibility_warning = codex.as_ref().and_then(|codex| {
        read_codex_version(codex, &paths.codex_home)
            .ok()
            .and_then(|(display, expected)| {
                (expected != TESTED_CODEX_VERSION).then(|| {
                    format!(
                        "Compatibility warning: Codex app-server {display} has not been tested with codex-session-control {}; native results remain authoritative.\n",
                        env!("CARGO_PKG_VERSION")
                    )
                })
            })
    });

    let codex = codex.ok_or_else(|| {
        let recovery = manual_native_removal(&paths.codex_home, None, false);
        let recovery = if desktop_intent_removed {
            format!("{DESKTOP_DETACH_GUIDANCE}{recovery}")
        } else {
            recovery
        };
        progress.fail(
            UninstallStage::PluginRemove,
            "native plugin absence cannot be proven",
            &recovery,
        )
    })?;
    remove_native_plugin_if_present(&codex, &paths.codex_home).map_err(|_| {
        let recovery = manual_native_removal(&paths.codex_home, Some(&codex), false);
        let recovery = if desktop_intent_removed {
            format!("{DESKTOP_DETACH_GUIDANCE}{recovery}")
        } else {
            recovery
        };
        progress.fail(
            UninstallStage::PluginRemove,
            "native plugin absence cannot be proven",
            &recovery,
        )
    })?;
    complete_lifecycle_stage!(
        progress,
        UninstallStage::PluginRemove,
        context.target,
        &retry
    );

    remove_native_marketplace_if_present(&codex, &paths.codex_home).map_err(|_| {
        let recovery = manual_native_removal(&paths.codex_home, Some(&codex), true);
        let recovery = if desktop_intent_removed {
            format!("{DESKTOP_DETACH_GUIDANCE}{recovery}")
        } else {
            recovery
        };
        progress.fail(
            UninstallStage::MarketplaceRemove,
            "native marketplace absence cannot be proven",
            &recovery,
        )
    })?;
    complete_lifecycle_stage!(
        progress,
        UninstallStage::MarketplaceRemove,
        context.target,
        &retry
    );

    remove_owned_tree(&paths.marketplace, paths.euid)
        .map_err(|error| progress.fail(UninstallStage::ProjectionRemove, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UninstallStage::ProjectionRemove,
        context.target,
        &retry
    );

    remove_owned_file(&paths.config, paths.euid, 0o600)
        .and_then(|()| {
            remove_owned_empty_dir(
                paths.config.parent().ok_or(ControllerError::InvalidData {
                    field: "configuration",
                    reason: "has no parent",
                })?,
                paths.euid,
            )
        })
        .map_err(|error| progress.fail(UninstallStage::ConfigurationRemove, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        UninstallStage::ConfigurationRemove,
        context.target,
        &retry
    );

    #[cfg(test)]
    if matches!(
        context.target.test_hooks.fail_after_completed_stage,
        Some("manifest-remove" | "binary-remove")
    ) {
        let stage = if context.target.test_hooks.fail_after_completed_stage
            == Some(UninstallStage::ManifestRemove.name())
        {
            UninstallStage::ManifestRemove
        } else {
            UninstallStage::BinaryRemove
        };
        return Err(progress.fail(
            stage,
            "injected failure before product identity removal",
            &retry,
        ));
    }

    remove_owned_file(&paths.manifest, paths.euid, 0o600)
        .map_err(|error| progress.fail(UninstallStage::ManifestRemove, error, &retry))?;
    let data_root_error = remove_owned_empty_dir(&paths.data_root, paths.euid).err();
    if data_root_error.is_none() {
        progress.complete(UninstallStage::ManifestRemove);
    }

    let binary_error = remove_owned_file(&paths.binary, paths.euid, 0o755).err();
    match (data_root_error, binary_error) {
        (None, None) => progress.complete(UninstallStage::BinaryRemove),
        (None, Some(error)) => {
            return Err(ControllerError::Operational(format!(
                "{}failed at binary-remove: terminal partial uninstall: {error}\n\
remaining product executable: {}\n\
installed identity was removed; no fresh-process retry is available\n",
                progress.stderr(),
                paths.binary.display()
            )));
        }
        (Some(error), None) => {
            return Err(ControllerError::Operational(format!(
                "{}failed at manifest-remove: terminal partial uninstall: {error}\n\
remaining product root: {}\n\
product executable and installed identity were removed; no fresh-process retry is available\n",
                progress.stderr(),
                paths.data_root.display()
            )));
        }
        (Some(data_root_error), Some(binary_error)) => {
            return Err(ControllerError::Operational(format!(
                "{}failed at manifest-remove: terminal partial uninstall: {data_root_error}; binary-remove also failed: {binary_error}\n\
remaining product root: {}\n\
remaining product executable: {}\n\
installed identity was removed; no fresh-process retry is available\n",
                progress.stderr(),
                paths.data_root.display(),
                paths.binary.display()
            )));
        }
    }

    let mut stderr = progress.stderr();
    if let Some(warning) = compatibility_warning {
        stderr.push_str(&warning);
    }
    if desktop_intent_removed {
        stderr.push_str(DESKTOP_DETACH_GUIDANCE);
    }
    Ok(UninstallReceipt {
        stdout: format!(
            "Codex app-server service: removed\n\
Product descriptor: removed\n\
Product projection: removed\n\
Product configuration: removed\n\
Product executable: removed\n\
Codex home preserved: {}\n\
Authentication preserved: yes\n\
Tasks preserved: yes\n\
Rollouts preserved: yes\n",
            paths.codex_home.display()
        ),
        stderr,
    })
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

fn manual_native_removal(codex_home: &Path, codex: Option<&Path>, marketplace: bool) -> String {
    native::manual_native_removal(codex_home, codex, marketplace)
}
