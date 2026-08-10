use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use crate::{
    desktop::{
        DescriptorPublicationFailure, DescriptorPublicationResidue, DesktopTarget,
        remove_expected_descriptor, render_descriptor,
    },
    error::ControllerError,
};
use sha2::{Digest, Sha256};

mod evidence;
mod native;
mod paths;
mod release;
mod render;
mod service;
mod status;
mod wrapper;

pub(crate) use evidence::{ResolvedUserPaths, load_installed_config};
pub(crate) use service::LifecycleTarget;
pub(crate) use status::status;
pub(crate) use wrapper::codex_wrapper;

use evidence::SelectedHomeEvidence;
use service::{ServiceActivity, query_service_activity, verify_absent_control_socket};

const DESKTOP_DETACH_GUIDANCE: &str =
    "Desktop: fully exit and restart Desktop to return to ordinary mode.\n";

fn display_command_for_paths(paths: &ResolvedUserPaths, path_environment: &OsStr) -> String {
    let install_bin = paths.home.join(".local/bin");
    if std::env::split_paths(path_environment).any(|entry| entry == install_bin) {
        "codex-session-control".to_owned()
    } else {
        paths.binary.display().to_string()
    }
}

fn missing_control_socket_error() -> ControllerError {
    ControllerError::InvalidData {
        field: "socket",
        reason: "missing or inaccessible",
    }
}

#[derive(Clone, Debug)]
struct CandidateRelease {
    executable: PathBuf,
    product_version: String,
    target: String,
}

#[derive(Clone, Debug)]
struct LifecycleContext {
    target: LifecycleTarget,
    path_environment: std::ffi::OsString,
    desktop_environment: BTreeMap<OsString, OsString>,
    cwd: PathBuf,
}

#[derive(Debug)]
pub(crate) struct LifecycleReceipt {
    pub stdout: String,
    pub stderr: String,
}

macro_rules! complete_lifecycle_stage {
    ($progress:expr, $stage:expr, $target:expr, $recovery:expr) => {{
        $progress.complete($stage);
        #[cfg(test)]
        if $target.test_hooks.fail_after_completed_stage == Some($stage.name()) {
            return Err($progress.fail(
                $stage,
                "injected failure after completed stage",
                $recovery,
            ));
        }
    }};
}

mod update;
pub(crate) use update::update;

mod enable_disable;
mod setup;
mod uninstall;

pub(crate) use enable_disable::{disable, enable};
pub(crate) use setup::setup;
pub(crate) use uninstall::uninstall;

#[cfg(target_arch = "x86_64")]
fn product_target() -> &'static str {
    "x86_64-unknown-linux-gnu"
}

#[cfg(target_arch = "aarch64")]
fn product_target() -> &'static str {
    "aarch64-unknown-linux-gnu"
}

#[cfg(test)]
fn test_target() -> &'static str {
    product_target()
}

fn lifecycle_context(target: LifecycleTarget) -> Result<LifecycleContext, ControllerError> {
    Ok(LifecycleContext {
        target,
        path_environment: std::env::var_os("PATH").ok_or(ControllerError::InvalidData {
            field: "PATH",
            reason: "is unavailable",
        })?,
        desktop_environment: std::env::vars_os().collect(),
        cwd: std::env::current_dir().map_err(|_| ControllerError::InvalidData {
            field: "cwd",
            reason: "is unavailable",
        })?,
    })
}

fn remove_persisted_desktop_descriptor(
    paths: &ResolvedUserPaths,
    evidence: &SelectedHomeEvidence,
) -> Result<bool, ControllerError> {
    let manifest = evidence.manifest.as_ref().ok_or_else(|| {
        ControllerError::Operational(
            "valid noncontradictory manifest identity is required; Desktop descriptor path cannot be guessed"
                .to_owned(),
        )
    })?;
    let Some(identity) = manifest.desktop_attachment.as_ref() else {
        return Ok(false);
    };
    let expected = render_descriptor(&paths.socket)?;
    remove_expected_descriptor(identity, &expected)
}

fn incomplete_descriptor_cleanup(error: impl std::fmt::Display) -> String {
    format!("Desktop descriptor cleanup is incomplete: {error}; later product state was retained")
}

fn cleanup_changed_descriptor_after_start_failure(
    systemctl: &Path,
    target: &LifecycleTarget,
    desktop: Option<&DesktopTarget>,
    descriptor: Option<&[u8]>,
) -> Result<(), DescriptorPublicationFailure> {
    let (desktop, descriptor) = desktop
        .zip(descriptor)
        .expect("changed Desktop descriptor has exact identity and bytes");
    let residue = desktop.identity.descriptor_path.clone();
    let failed = |source| DescriptorPublicationFailure {
        source,
        residue: Some(DescriptorPublicationResidue::Final(residue.clone())),
    };
    match query_service_activity(systemctl, &target.unit_name) {
        ServiceActivity::Active => {
            return Err(failed(ControllerError::Operational(
                "service is active; exact Desktop descriptor was retained".to_owned(),
            )));
        }
        ServiceActivity::Unproven => {
            return Err(failed(ControllerError::Operational(
                "service activity could not be proven; exact Desktop descriptor was retained"
                    .to_owned(),
            )));
        }
        ServiceActivity::Inactive => {}
    }
    verify_absent_control_socket(target).map_err(&failed)?;
    remove_expected_descriptor(&desktop.identity, descriptor).map_err(failed)?;
    Ok(())
}

const UNKNOWN_CODEX_VERSION: &str = "0.0.0+unknown";

fn normalized_codex_version(display: &str) -> String {
    if display == UNKNOWN_CODEX_VERSION {
        return "unknown".to_owned();
    }
    semver::Version::parse(display)
        .map(|version| version.to_string())
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopAttachmentStatus {
    Available,
    Unavailable,
    Unverified,
}

impl DesktopAttachmentStatus {
    const fn receipt(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Unverified => "unverified",
        }
    }
}

#[derive(Clone, Debug)]
struct LifecycleDesktopPlan {
    target: Option<DesktopTarget>,
    descriptor: Option<Vec<u8>>,
    status: DesktopAttachmentStatus,
    warning: Option<String>,
    setup_required: bool,
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
