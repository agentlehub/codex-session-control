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
    desktop::{
        DescriptorState, DesktopAvailability, DesktopTarget, discover_and_verify_desktop,
        inspect_descriptor, preflight_descriptor_switch, publish_descriptor,
        remove_expected_descriptor, render_descriptor, verify_persisted_desktop,
    },
    error::ControllerError,
    model::{DesktopAttachmentIdentity, InstalledRelease, ProductConfig},
};

use super::{
    CandidateRelease, DesktopAttachmentStatus, cleanup_changed_descriptor_after_start_failure,
    evidence::{
        InstalledEvidenceCase, NativeProductState, ResolvedUserPaths, SelectedHomeEvidence,
        SelectedHomeOperation, classify_selected_home_evidence, require_selected_home_evidence,
        resolve_setup_selected_home,
    },
    native::{
        plugin_matches, read_codex_version, read_installed_product_version, reconcile_marketplace,
        reconcile_plugin, resolve_named_executable, valid_owned_executable,
    },
    paths::{
        FileKind, create_missing_selected_codex_home, create_product_dir, create_shared_dir,
        lifecycle_file_error, read_product_evidence_file, reconcile_file, resolve_codex_executable,
        validate_existing,
    },
    product_target,
    release::RELEASE_REPOSITORY,
    render::{RenderedProjection, reconcile_projection, render_projection, render_unit},
    service::{
        LifecycleTarget, append_unattached_client_guidance, detect_running_unattached_clients,
        run_systemctl, verify_setup_service,
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

#[derive(Debug)]
pub(crate) struct SetupReport {
    pub stdout: String,
    pub stderr: String,
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
}

#[derive(Default)]
struct SetupProgress {
    completed: Vec<SetupStage>,
}

impl SetupProgress {
    fn complete(&mut self, stage: SetupStage) {
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
        stage: SetupStage,
        cause: impl std::fmt::Display,
        recovery: &str,
    ) -> ControllerError {
        let mut message = self.stderr();
        message.push_str(&format!("failed at {}: {cause}\n", stage.name()));
        message.push_str(recovery);
        ControllerError::Operational(message)
    }
}

pub(super) struct SetupPreflight {
    codex: PathBuf,
    systemctl: PathBuf,
    expected_running_version: String,
    compatibility_warning: Option<String>,
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

pub(super) struct PreflightFailure {
    pub(super) cause: String,
    recovery: String,
}

pub(crate) async fn setup(desktop_launcher: Option<&Path>) -> Result<SetupReport, ControllerError> {
    let paths = ResolvedUserPaths::from_effective_user()?;
    let candidate = CandidateRelease {
        executable: std::env::current_exe().map_err(|_| ControllerError::InvalidData {
            field: "executable",
            reason: "cannot resolve current executable",
        })?,
        product_version: env!("CARGO_PKG_VERSION").to_owned(),
        target: product_target().to_owned(),
    };
    let path_environment = std::env::var_os("PATH").ok_or(ControllerError::InvalidData {
        field: "PATH",
        reason: "is unavailable",
    })?;
    let cwd = std::env::current_dir().map_err(|_| ControllerError::InvalidData {
        field: "cwd",
        reason: "is unavailable",
    })?;
    setup_with_context(SetupContext {
        target: LifecycleTarget::production(paths),
        candidate,
        path_environment,
        desktop_environment: std::env::vars_os().collect(),
        desktop_launcher: desktop_launcher.map(Path::to_path_buf),
        cwd,
    })
    .await
}

pub(super) async fn setup_with_context(
    mut context: SetupContext,
) -> Result<SetupReport, ControllerError> {
    let mut progress = SetupProgress::default();
    let retry_setup = match context.desktop_launcher.as_ref() {
        Some(launcher) => format!(
            "retry: {} setup --desktop-launcher {}\n",
            display_command(&context),
            launcher.display()
        ),
        None => format!("retry: {} setup\n", display_command(&context)),
    };
    let preflight = match setup_preflight(&mut context).await {
        Ok(preflight) => preflight,
        Err(failure) => {
            return Err(progress.fail(SetupStage::Preflight, failure.cause, &failure.recovery));
        }
    };
    complete_lifecycle_stage!(
        progress,
        SetupStage::Preflight,
        context.target,
        &retry_setup
    );

    let paths = &context.target.paths;
    if let Err(error) = (|| {
        create_shared_dir(
            paths.binary.parent().ok_or(ControllerError::InvalidData {
                field: "binary",
                reason: "has no parent",
            })?,
            paths.euid,
        )?;
        reconcile_file(&paths.binary, &preflight.binary, 0o755, paths.euid)
    })() {
        return Err(progress.fail(SetupStage::Binary, error, &retry_setup));
    }
    complete_lifecycle_stage!(progress, SetupStage::Binary, context.target, &retry_setup);

    if let Err(error) = (|| {
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
    })() {
        return Err(progress.fail(SetupStage::Configuration, error, &retry_setup));
    }
    complete_lifecycle_stage!(
        progress,
        SetupStage::Configuration,
        context.target,
        &retry_setup
    );

    let projection_changed = match reconcile_projection(paths, &preflight.projection) {
        Ok(changed) => changed,
        Err(error) => {
            return Err(progress.fail(SetupStage::Projection, error, &retry_setup));
        }
    };
    complete_lifecycle_stage!(
        progress,
        SetupStage::Projection,
        context.target,
        &retry_setup
    );

    let marketplace_changed =
        match reconcile_marketplace(&preflight.codex, &paths.codex_home, &paths.marketplace) {
            Ok(changed) => changed,
            Err(error) => {
                return Err(progress.fail(SetupStage::PluginMarketplace, error, &retry_setup));
            }
        };
    complete_lifecycle_stage!(
        progress,
        SetupStage::PluginMarketplace,
        context.target,
        &retry_setup
    );

    let plugin_changed = match reconcile_plugin(
        &preflight.codex,
        &paths.codex_home,
        &paths.marketplace,
        &context.candidate.product_version,
    ) {
        Ok(changed) => changed,
        Err(error) => {
            return Err(progress.fail(SetupStage::PluginInstall, error, &retry_setup));
        }
    };
    complete_lifecycle_stage!(
        progress,
        SetupStage::PluginInstall,
        context.target,
        &retry_setup
    );

    complete_lifecycle_stage!(
        progress,
        SetupStage::DesktopDiscovery,
        context.target,
        &retry_setup
    );
    let (_desktop_published, desktop_intent_changed) = if let (Some(target), Some(descriptor)) =
        (&preflight.desktop.target, &preflight.desktop.descriptor)
    {
        let published = match publish_descriptor(&target.identity, descriptor) {
            Ok(published) => published,
            Err(error) => return Err(progress.fail(SetupStage::Descriptor, error, &retry_setup)),
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
                && let Err(error) = fs::write(
                    &previous.descriptor_path,
                    b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/descriptor-removal-race\"}",
                )
                .and_then(|()| {
                    fs::set_permissions(
                        &previous.descriptor_path,
                        fs::Permissions::from_mode(0o600),
                    )
                })
            {
                return Err(progress.fail(SetupStage::Descriptor, error, &retry_setup));
            }
            if let Err(error) = remove_expected_descriptor(previous, descriptor) {
                let cleanup = if published {
                    remove_expected_descriptor(&target.identity, descriptor).err()
                } else {
                    None
                };
                let cause = match cleanup {
                    Some(cleanup) => format!(
                        "Desktop descriptor switch failed at {}: {error}; newly published descriptor at {} could not be removed safely: {cleanup}; new routing intent may remain unmanifested",
                        previous.descriptor_path.display(),
                        target.identity.descriptor_path.display(),
                    ),
                    None => format!(
                        "Desktop descriptor switch failed at {}: {error}",
                        previous.descriptor_path.display(),
                    ),
                };
                return Err(progress.fail(SetupStage::Descriptor, cause, &retry_setup));
            }
        }
        (published, published || descriptor_path_changed)
    } else {
        (false, false)
    };
    complete_lifecycle_stage!(
        progress,
        SetupStage::Descriptor,
        context.target,
        &retry_setup
    );

    if let Err(error) = (|| {
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
    })() {
        return Err(fail_after_descriptor_publication(
            &mut progress,
            SetupStage::ServiceUnit,
            error,
            &retry_setup,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_lifecycle_stage!(
        progress,
        SetupStage::ServiceUnit,
        context.target,
        &retry_setup
    );
    let unattached_clients = if preflight.desktop.status == DesktopAttachmentStatus::Available {
        detect_running_unattached_clients(&context.target.client_process_source, paths.euid)
    } else {
        BTreeSet::new()
    };

    if let Err(error) = run_systemctl(&preflight.systemctl, ["--user", "daemon-reload"]) {
        return Err(fail_after_descriptor_publication(
            &mut progress,
            SetupStage::DaemonReload,
            error,
            &retry_setup,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_lifecycle_stage!(
        progress,
        SetupStage::DaemonReload,
        context.target,
        &retry_setup
    );

    if let Err(error) = run_systemctl(
        &preflight.systemctl,
        [
            "--user",
            "enable",
            "--now",
            context.target.unit_name.as_str(),
        ],
    ) {
        return Err(fail_after_descriptor_publication(
            &mut progress,
            SetupStage::ServiceEnable,
            error,
            &retry_setup,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_lifecycle_stage!(
        progress,
        SetupStage::ServiceEnable,
        context.target,
        &retry_setup
    );

    if let Err(error) = verify_setup_service(
        &preflight.systemctl,
        &context.target,
        &preflight.expected_running_version,
    )
    .await
    {
        let update = format!("retry: {} update\n", display_command(&context));
        return Err(fail_after_descriptor_publication(
            &mut progress,
            SetupStage::ServiceVerify,
            error,
            &update,
            desktop_intent_changed,
            &preflight,
            &context.target,
        ));
    }
    complete_lifecycle_stage!(
        progress,
        SetupStage::ServiceVerify,
        context.target,
        &retry_setup
    );

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
        progress.fail(
            SetupStage::Manifest,
            "cannot serialize installed release",
            &retry_setup,
        )
    })?;
    manifest_bytes.push(b'\n');
    if let Err(error) = reconcile_file(&paths.manifest, &manifest_bytes, 0o600, paths.euid) {
        return Err(progress.fail(SetupStage::Manifest, error, &retry_setup));
    }
    complete_lifecycle_stage!(progress, SetupStage::Manifest, context.target, &retry_setup);

    let projection_changed = projection_changed || marketplace_changed || plugin_changed;
    let mut stderr = progress.stderr();
    if let Some(warning) = preflight.compatibility_warning {
        stderr.push_str(&warning);
        stderr.push('\n');
    }
    if let Some(warning) = &preflight.desktop.warning {
        stderr.push_str(warning);
        stderr.push('\n');
    }
    let mut stdout = format!(
        "Installed release: {version}\n\
Codex app-server service: enabled, active\n\
Codex home: {home}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: {desktop}\n\
Desktop restart required: {desktop_restart}\n\
Plugin: codex-session-control {version} at {plugin}\n\
Durable plugin state: current\n\
Loaded task state: {loaded}\n\
New task required for guaranteed plugin convergence: yes\n",
        version = context.candidate.product_version,
        plugin = paths
            .marketplace
            .join("plugins/codex-session-control")
            .display(),
        home = paths.codex_home.display(),
        desktop = preflight.desktop.status.receipt(),
        desktop_restart = if desktop_intent_changed { "yes" } else { "no" },
        loaded = if projection_changed {
            "may_be_stale"
        } else {
            "not_verified"
        },
    );
    if !install_bin_on_path(&context) {
        stdout.push_str(&format!(
            "\nNote: {} is not on PATH. Add it to PATH to use the short codex-session-control command.\n",
            paths.home.join(".local/bin").display()
        ));
    }
    append_unattached_client_guidance(&mut stdout, &unattached_clients);
    Ok(SetupReport { stdout, stderr })
}

pub(super) async fn setup_preflight(
    context: &mut SetupContext,
) -> Result<SetupPreflight, PreflightFailure> {
    let fail = |cause: String, recovery: String| PreflightFailure { cause, recovery };
    let candidate_retry = match context.desktop_launcher.as_ref() {
        Some(launcher) => format!(
            "retry: {} setup --desktop-launcher {}\n",
            context.candidate.executable.display(),
            launcher.display()
        ),
        None => format!("retry: {} setup\n", context.candidate.executable.display()),
    };
    let evidence = classify_selected_home_evidence(&context.target.paths);
    SelectedHomeOperation::Setup
        .require_permitted_case(evidence.case)
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
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
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    let systemctl = resolve_named_executable(&context.path_environment, &context.cwd, "systemctl")
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    let native = resolve_setup_selected_home(&mut context.target.paths, &codex)
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    let paths = &context.target.paths;
    require_selected_home_evidence(paths, SelectedHomeOperation::Setup)
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    if !context.candidate.executable.is_absolute()
        || !context.cwd.is_absolute()
        || context.candidate.product_version.is_empty()
        || context.candidate.target != product_target()
    {
        return Err(fail(
            "candidate identity is invalid".to_owned(),
            candidate_retry,
        ));
    }
    let (codex_version, expected_running_version) =
        read_codex_version(&codex, &paths.codex_home)
            .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    let binary = fs::read(&context.candidate.executable).map_err(|_| {
        fail(
            "candidate executable is unreadable".to_owned(),
            candidate_retry.clone(),
        )
    })?;
    let binary_sha256 = sha256_bytes(&binary);
    let config = ProductConfig {
        schema_version: 2,
        codex_executable: codex.clone(),
        codex_home: paths.codex_home.clone(),
        socket_path: paths.socket.clone(),
    };
    let config = toml::to_string(&config)
        .map(String::into_bytes)
        .map_err(|_| {
            fail(
                "configuration cannot be rendered".to_owned(),
                candidate_retry.clone(),
            )
        })?;
    let projection = render_projection(&paths.binary, &context.candidate.product_version)
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    let unit = render_unit(paths, &codex)
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    let unit_sha256 = sha256_bytes(&unit);

    validate_manifestless_setup_artifacts(context, &binary, &config, &projection, &unit, &native)?;
    let compatibility_warning = (expected_running_version != TESTED_CODEX_VERSION).then(|| {
        format!(
            "Compatibility warning: Codex app-server {codex_version} has not been tested with codex-session-control {}; native results remain authoritative.",
            context.candidate.product_version
        )
    });
    let desktop = resolve_setup_desktop(context, &evidence)
        .await
        .map_err(|error| fail(error.to_string(), candidate_retry.clone()))?;
    Ok(SetupPreflight {
        codex,
        systemctl,
        expected_running_version,
        compatibility_warning,
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
) -> Result<SetupDesktopPlan, ControllerError> {
    let previous = evidence
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.desktop_attachment.clone());
    let availability = if let Some(launcher) = context.desktop_launcher.as_deref() {
        discover_and_verify_desktop(Some(launcher), &context.desktop_environment).await?
    } else if let Some(identity) = previous.as_ref() {
        verify_persisted_desktop(identity, &context.desktop_environment).await?
    } else {
        discover_and_verify_desktop(None, &context.desktop_environment).await?
    };
    match availability {
        DesktopAvailability::Verified(target) => {
            let descriptor = render_descriptor(&context.target.paths.socket)?;
            preflight_descriptor_switch(previous.as_ref(), &target.identity, &descriptor)?;
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
                let expected = render_descriptor(&context.target.paths.socket)?;
                if !matches!(
                    inspect_descriptor(identity, &expected)?,
                    DescriptorState::Absent | DescriptorState::Expected
                ) {
                    return Err(ControllerError::Operational(
                        "Desktop descriptor is foreign".to_owned(),
                    ));
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
    progress: &mut SetupProgress,
    stage: SetupStage,
    cause: impl std::fmt::Display,
    recovery: &str,
    descriptor_intent_changed: bool,
    preflight: &SetupPreflight,
    target: &LifecycleTarget,
) -> ControllerError {
    let cause = cause.to_string();
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
    match cleanup {
        Ok(_) => progress.fail(stage, cause, recovery),
        Err(cleanup) => progress.fail(
            stage,
            format!(
                "{cause}; cleanup after changed Desktop descriptor could not complete: {cleanup}; Desktop routing state is unverified"
            ),
            recovery,
        ),
    }
}

fn validate_manifestless_setup_artifacts(
    context: &SetupContext,
    binary: &[u8],
    config: &[u8],
    projection: &RenderedProjection,
    unit: &[u8],
    native: &NativeProductState,
) -> Result<(), PreflightFailure> {
    let paths = &context.target.paths;
    let evidence = classify_selected_home_evidence(paths);
    if let Some(expected_manifest) = evidence.manifest.as_ref() {
        let manifest: InstalledRelease = serde_json::from_slice(
            &read_product_evidence_file(&paths.home, paths.euid, &paths.manifest, 0o600).map_err(
                |error| PreflightFailure {
                    cause: lifecycle_file_error(&paths.manifest, error),
                    recovery: String::new(),
                },
            )?,
        )
        .map_err(|_| PreflightFailure {
            cause: "installed manifest is invalid".to_owned(),
            recovery: String::new(),
        })?;
        manifest
            .validate(&paths.codex_home, &paths.socket)
            .map_err(|error| PreflightFailure {
                cause: error.to_string(),
                recovery: String::new(),
            })?;
        if manifest != *expected_manifest {
            return Err(PreflightFailure {
                cause: "installed manifest changed during validation".to_owned(),
                recovery: String::new(),
            });
        }
        if manifest.product_version != context.candidate.product_version
            || manifest.target != context.candidate.target
        {
            let executable = if valid_owned_executable(&paths.binary, paths.euid) {
                paths.binary.as_path()
            } else {
                context.candidate.executable.as_path()
            };
            return Err(PreflightFailure {
                cause: format!(
                    "installed release {} differs from candidate {}",
                    manifest.product_version, context.candidate.product_version
                ),
                recovery: format!("retry: {} update\n", executable.display()),
            });
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
        let recovery = if read_installed_product_version(&paths.binary, paths.euid).as_deref()
            == Some(version.as_str())
        {
            format!("retry: {} setup\n", paths.binary.display())
        } else {
            format!(
                "recovery source: https://github.com/{RELEASE_REPOSITORY}/releases/download/v{version}/codex-session-control-{}\nchecksum source: https://github.com/{RELEASE_REPOSITORY}/releases/download/v{version}/SHA256SUMS\n",
                context.candidate.target,
            )
        };
        return Err(PreflightFailure {
            cause: format!("release {version} is partially installed without a manifest"),
            recovery,
        });
    }
    if identified_versions.contains(&context.candidate.product_version) && !ambiguous.is_empty() {
        ambiguous.insert("conflicting release identities".to_owned());
    }
    if ambiguous.is_empty() {
        Ok(())
    } else {
        Err(PreflightFailure {
            cause: format!(
                "ambiguous manifestless release artifacts: {}",
                ambiguous.into_iter().collect::<Vec<_>>().join(", ")
            ),
            recovery: String::new(),
        })
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

fn display_command(context: &SetupContext) -> String {
    if install_bin_on_path(context) {
        "codex-session-control".to_owned()
    } else {
        context.target.paths.binary.display().to_string()
    }
}
