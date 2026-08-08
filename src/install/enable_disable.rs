use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::Path,
};

use crate::{
    app_server::TESTED_CODEX_VERSION,
    desktop::{
        DescriptorState, DesktopAvailability, DesktopTarget, inspect_descriptor,
        preflight_descriptor_switch, publish_descriptor, render_descriptor,
        verify_persisted_desktop,
    },
    error::ControllerError,
    model::{InstalledRelease, ProductConfig},
};

use super::{
    DESKTOP_DETACH_GUIDANCE, DesktopAttachmentStatus, LifecycleContext, LifecycleDesktopPlan,
    LifecycleReceipt, LifecycleTarget, cleanup_changed_descriptor_after_start_failure,
    display_command_for_paths,
    evidence::{ResolvedUserPaths, SelectedHomeOperation, require_selected_home_evidence},
    incomplete_descriptor_cleanup, lifecycle_context,
    native::{read_codex_version, resolve_named_executable},
    paths::{lifecycle_file_error, read_product_evidence_file, read_status_file},
    remove_persisted_desktop_descriptor,
    render::render_unit,
    service::{
        CallerUnitEvidence, CallerUnitInspection, ServiceActivity,
        append_unattached_client_guidance, detect_running_unattached_clients, inspect_caller_unit,
        query_service_activity, run_systemctl, verify_disabled_service, verify_enabled_service,
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
    const fn name(self) -> &'static str {
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

#[derive(Default)]
struct LifecycleProgress {
    completed: Vec<LifecycleStage>,
}

impl LifecycleProgress {
    fn complete(&mut self, stage: LifecycleStage) {
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
        stage: LifecycleStage,
        cause: impl std::fmt::Display,
        retry: &str,
    ) -> ControllerError {
        ControllerError::Operational(format!(
            "{}failed at {}: {cause}\nretry: {retry}\n",
            self.stderr(),
            stage.name()
        ))
    }
}

pub(crate) async fn enable(target: LifecycleTarget) -> Result<LifecycleReceipt, ControllerError> {
    enable_with_context(lifecycle_context(target)?).await
}

pub(crate) async fn disable(target: LifecycleTarget) -> Result<LifecycleReceipt, ControllerError> {
    disable_with_context(lifecycle_context(target)?).await
}

pub(super) async fn enable_with_context(
    context: LifecycleContext,
) -> Result<LifecycleReceipt, ControllerError> {
    let mut progress = LifecycleProgress::default();
    let display_command =
        display_command_for_paths(&context.target.paths, &context.path_environment);
    let retry = format!("{display_command} enable");
    let paths = &context.target.paths;
    let evidence = require_selected_home_evidence(paths, SelectedHomeOperation::Enable)
        .map_err(|error| progress.fail(LifecycleStage::Configuration, error, &retry))?;
    let expected_config = evidence.configuration.ok_or_else(|| {
        progress.fail(
            LifecycleStage::Configuration,
            "invalid installed configuration",
            &retry,
        )
    })?;
    let manifest = evidence.manifest.as_ref().ok_or_else(|| {
        progress.fail(
            LifecycleStage::Configuration,
            "invalid installed manifest",
            &retry,
        )
    })?;
    let config_bytes = read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600)
        .map_err(|error| {
            progress.fail(
                LifecycleStage::Configuration,
                lifecycle_file_error(&paths.config, error),
                &retry,
            )
        })?;
    let config = std::str::from_utf8(&config_bytes)
        .ok()
        .and_then(|text| toml::from_str::<ProductConfig>(text).ok())
        .filter(|config| config.validate(&paths.codex_home, &paths.socket).is_ok())
        .ok_or_else(|| {
            progress.fail(
                LifecycleStage::Configuration,
                "invalid installed configuration",
                &retry,
            )
        })?;
    if config != expected_config {
        return Err(progress.fail(
            LifecycleStage::Configuration,
            "installed configuration changed during validation",
            &retry,
        ));
    }

    let unit = read_status_file(&paths.unit, paths.euid, 0o644).map_err(|error| {
        progress.fail(
            LifecycleStage::ServiceUnit,
            lifecycle_file_error(&paths.unit, error),
            &retry,
        )
    })?;
    let expected_unit = render_unit(paths, &config.codex_executable)
        .map_err(|error| progress.fail(LifecycleStage::ServiceUnit, error, &retry))?;
    if unit != expected_unit {
        return Err(progress.fail(
            LifecycleStage::ServiceUnit,
            "installed unit does not match configuration",
            &retry,
        ));
    }

    let desktop = resolve_enable_desktop(manifest, paths, &context.desktop_environment)
        .await
        .map_err(|error| progress.fail(LifecycleStage::Descriptor, error, &retry))?;
    let (codex_version, expected_running_version) =
        read_codex_version(&config.codex_executable, &paths.codex_home)
            .map_err(|error| progress.fail(LifecycleStage::Configuration, error, &retry))?;
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|error| progress.fail(LifecycleStage::ServiceEnable, error, &retry))?;
    let desktop_published = if let (Some(target), Some(descriptor)) =
        (&desktop.target, &desktop.descriptor)
    {
        let published = publish_descriptor(&target.identity, descriptor)
            .map_err(|error| progress.fail(LifecycleStage::Descriptor, error, &retry))?;
        if published {
            complete_lifecycle_stage!(progress, LifecycleStage::Descriptor, context.target, &retry);
        }
        published
    } else {
        false
    };
    let unattached_clients = if desktop.status == DesktopAttachmentStatus::Available {
        detect_running_unattached_clients(&context.target.client_process_source, paths.euid)
    } else {
        BTreeSet::new()
    };
    run_systemctl(
        &systemctl,
        [
            "--user",
            "enable",
            "--now",
            context.target.unit_name.as_str(),
        ],
    )
    .map_err(|error| {
        let error = cleanup_enable_descriptor(
            &systemctl,
            &context.target,
            desktop_published,
            desktop.target.as_ref(),
            desktop.descriptor.as_deref(),
        )
        .err()
        .map(|cleanup| {
            format!(
                "{error}; cleanup after published Desktop descriptor failed: {cleanup}; Desktop routing state is unverified"
            )
        })
        .unwrap_or_else(|| error.to_string());
        progress.fail(LifecycleStage::ServiceEnable, error, &retry)
    })?;
    complete_lifecycle_stage!(
        progress,
        LifecycleStage::ServiceEnable,
        context.target,
        &retry
    );

    verify_enabled_service(&systemctl, &context.target, &expected_running_version)
        .await
        .map_err(|error| {
            let error = cleanup_enable_descriptor(
                &systemctl,
                &context.target,
                desktop_published,
                desktop.target.as_ref(),
                desktop.descriptor.as_deref(),
            )
            .err()
            .map(|cleanup| {
                format!(
                    "{error}; cleanup after published Desktop descriptor failed: {cleanup}; Desktop routing state is unverified"
                )
            })
            .unwrap_or_else(|| error.to_string());
            progress.fail(LifecycleStage::ServiceVerify, error, &retry)
        })?;
    complete_lifecycle_stage!(
        progress,
        LifecycleStage::ServiceVerify,
        context.target,
        &retry
    );

    let mut stderr = progress.stderr();
    if expected_running_version != TESTED_CODEX_VERSION {
        stderr.push_str(&format!(
            "Compatibility warning: Codex app-server {codex_version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
            env!("CARGO_PKG_VERSION")
        ));
    }
    if let Some(warning) = desktop.warning {
        stderr.push_str(&warning);
        stderr.push('\n');
    }
    let mut stdout = format!(
        "Codex app-server service: enabled, active\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: {}\n\
Desktop restart required: {}\n",
        desktop.status.receipt(),
        if desktop_published { "yes" } else { "no" },
    );
    if desktop.setup_required {
        stdout.push_str("Run codex-session-control setup to attach Desktop.\n");
    }
    append_unattached_client_guidance(&mut stdout, &unattached_clients);
    Ok(LifecycleReceipt { stdout, stderr })
}

fn cleanup_enable_descriptor(
    systemctl: &Path,
    target: &LifecycleTarget,
    descriptor_intent_changed: bool,
    desktop: Option<&DesktopTarget>,
    descriptor: Option<&[u8]>,
) -> Result<(), ControllerError> {
    if descriptor_intent_changed {
        cleanup_changed_descriptor_after_start_failure(systemctl, target, desktop, descriptor)?;
    }
    Ok(())
}

pub(super) async fn disable_with_context(
    context: LifecycleContext,
) -> Result<LifecycleReceipt, ControllerError> {
    let mut progress = LifecycleProgress::default();
    let display_command =
        display_command_for_paths(&context.target.paths, &context.path_environment);
    let retry = format!("{display_command} disable");
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|error| progress.fail(LifecycleStage::ServiceDisable, error, &retry))?;
    match query_service_activity(&systemctl, &context.target.unit_name) {
        ServiceActivity::Inactive => {}
        ServiceActivity::Active => match inspect_caller_unit(&systemctl, &context.target) {
            CallerUnitInspection::Independent => {}
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::WhoAmI) => {
                let recovery =
                    format!("run from an independent terminal:\n{display_command} disable\n");
                return Err(progress.fail(
                    LifecycleStage::ServiceDisable,
                    "refusing disable: this command is running inside the managed app-server",
                    &recovery,
                ));
            }
            CallerUnitInspection::SelfHosted(CallerUnitEvidence::ControlGroup)
            | CallerUnitInspection::Unknown { .. } => {
                let recovery = format!(
                    "caller independence could not be proven; from an independent terminal:\n\
                     systemctl --user stop {}\n\
                     {display_command} disable\n",
                    context.target.unit_name,
                );
                return Err(progress.fail(
                    LifecycleStage::ServiceDisable,
                    "refusing disable: caller independence cannot be proven",
                    &recovery,
                ));
            }
        },
        ServiceActivity::Unproven => {
            let recovery = format!(
                "service activity could not be proven; from an independent terminal:\n\
                 systemctl --user stop {}\n\
                 {display_command} disable\n",
                context.target.unit_name,
            );
            return Err(progress.fail(
                LifecycleStage::ServiceDisable,
                "refusing disable: service activity cannot be proven",
                &recovery,
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
    .map_err(|error| progress.fail(LifecycleStage::ServiceDisable, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        LifecycleStage::ServiceDisable,
        context.target,
        &retry
    );

    verify_disabled_service(&systemctl, &context.target)
        .map_err(|error| progress.fail(LifecycleStage::ServiceVerify, error, &retry))?;
    complete_lifecycle_stage!(
        progress,
        LifecycleStage::ServiceVerify,
        context.target,
        &retry
    );

    let evidence =
        require_selected_home_evidence(&context.target.paths, SelectedHomeOperation::Disable)
            .map_err(|error| {
                progress.fail(
                    LifecycleStage::DescriptorRemove,
                    incomplete_descriptor_cleanup(error),
                    &retry,
                )
            })?;
    let desktop_intent_removed =
        remove_persisted_desktop_descriptor(&context.target.paths, &evidence).map_err(|error| {
            progress.fail(
                LifecycleStage::DescriptorRemove,
                incomplete_descriptor_cleanup(error),
                &retry,
            )
        })?;
    #[cfg(test)]
    let descriptor_recovery = if desktop_intent_removed {
        format!("{DESKTOP_DETACH_GUIDANCE}{retry}")
    } else {
        retry.clone()
    };
    complete_lifecycle_stage!(
        progress,
        LifecycleStage::DescriptorRemove,
        context.target,
        &descriptor_recovery
    );

    let mut stdout = format!(
        "Codex app-server service: disabled, inactive\n\
Native Codex sessions: preserved\n\
Desktop restart required: {}\n",
        if desktop_intent_removed { "yes" } else { "no" }
    );
    if desktop_intent_removed {
        stdout.push_str(DESKTOP_DETACH_GUIDANCE);
    }
    Ok(LifecycleReceipt {
        stdout,
        stderr: progress.stderr(),
    })
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
    match verify_persisted_desktop(identity, environment).await? {
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
