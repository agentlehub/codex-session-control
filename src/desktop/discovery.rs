use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    process::Stdio,
};

use serde_json::Value;

use crate::{
    app_server::NATIVE_STAGE_TIMEOUT, error::ControllerError, model::DesktopAttachmentIdentity,
};

use super::{
    DESCRIPTOR_FILE_NAME, DESKTOP_CAPABILITY, DESKTOP_ENTRY_NAME, DiscoveryFailure,
    descriptor::{prepare_descriptor_parent, validate_identity_shape},
    entry::{ParsedDesktopExec, parse_desktop_entry},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DesktopAvailability {
    Verified(DesktopTarget),
    Unavailable { warning: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DesktopTarget {
    pub identity: DesktopAttachmentIdentity,
    pub command: DesktopLaunchCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DesktopLaunchCommand {
    pub launcher_path: PathBuf,
    pub fixed_args: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
    pub effective_config_root: PathBuf,
}

pub(crate) async fn discover_and_verify_desktop(
    override_path: Option<&Path>,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<DesktopAvailability, ControllerError> {
    discover_and_verify_desktop_inner(override_path, environment, true).await
}

pub(crate) async fn inspect_desktop_availability(
    override_path: Option<&Path>,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<DesktopAvailability, ControllerError> {
    discover_and_verify_desktop_inner(override_path, environment, false).await
}

async fn discover_and_verify_desktop_inner(
    override_path: Option<&Path>,
    environment: &BTreeMap<OsString, OsString>,
    prepare_parent: bool,
) -> Result<DesktopAvailability, ControllerError> {
    let parsed = match override_path {
        Some(path) => {
            if !path.is_absolute() || is_desktop_entry_path(path) {
                return Ok(unavailable(
                    "the Desktop launcher override is not one absolute executable path",
                ));
            }
            ParsedDesktopExec {
                executable: path.to_path_buf(),
                fixed_args: Vec::new(),
                environment: BTreeMap::new(),
            }
        }
        None => match desktop_entry_path(environment) {
            Ok(Some(path)) => match fs::read(&path) {
                Ok(bytes) => match parse_desktop_entry(&bytes) {
                    Ok(parsed) => parsed,
                    Err(error) => return Ok(unavailable_failure(error)),
                },
                Err(_) => return Ok(unavailable("the selected Desktop entry cannot be read")),
            },
            Ok(None) => return Ok(unavailable("codex-desktop.desktop was not found")),
            Err(error) => return Ok(unavailable_failure(error)),
        },
    };

    let mut child_environment = environment.clone();
    child_environment.extend(parsed.environment.clone());
    let launcher_path = match resolve_launcher(&parsed.executable, &child_environment) {
        Ok(path) => path,
        Err(error) => return Ok(unavailable_failure(error)),
    };
    let app_id =
        match inspect_build_info(&launcher_path, &parsed.fixed_args, &child_environment).await {
            Ok(app_id) => app_id,
            Err(error) => return Ok(unavailable_failure(error)),
        };
    let effective_config_root = match effective_config_root(&child_environment) {
        Ok(root) => root,
        Err(error) => return Ok(unavailable_failure(error)),
    };
    if let Err(error) = validate_app_id(&app_id) {
        return Ok(unavailable_failure(error));
    }
    let identity = DesktopAttachmentIdentity {
        launcher_path: launcher_path.clone(),
        app_id: app_id.clone(),
        descriptor_path: effective_config_root
            .join(&app_id)
            .join(DESCRIPTOR_FILE_NAME),
    };
    if prepare_parent {
        prepare_descriptor_parent(&identity)?;
    } else {
        validate_identity_shape(&identity)?;
    }
    Ok(DesktopAvailability::Verified(DesktopTarget {
        identity,
        command: DesktopLaunchCommand {
            launcher_path,
            fixed_args: parsed.fixed_args,
            environment: parsed.environment,
            effective_config_root,
        },
    }))
}

pub(crate) async fn verify_persisted_desktop(
    identity: &DesktopAttachmentIdentity,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<DesktopAvailability, ControllerError> {
    if let Err(error) = validate_identity_shape(identity) {
        return Ok(unavailable(error.to_string()));
    }
    let launcher_path = match resolve_launcher(&identity.launcher_path, environment) {
        Ok(path) => path,
        Err(error) => return Ok(unavailable_failure(error)),
    };
    let app_id = match inspect_build_info(&launcher_path, &[], environment).await {
        Ok(app_id) => app_id,
        Err(error) => return Ok(unavailable_failure(error)),
    };
    let config_root = identity
        .descriptor_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| desktop_error("persisted descriptor config root is missing"))?;
    if app_id != identity.app_id
        || identity.descriptor_path != config_root.join(&app_id).join(DESCRIPTOR_FILE_NAME)
    {
        return Ok(unavailable(
            "the persisted Desktop identity no longer matches build information",
        ));
    }
    Ok(DesktopAvailability::Verified(DesktopTarget {
        identity: identity.clone(),
        command: DesktopLaunchCommand {
            launcher_path,
            fixed_args: Vec::new(),
            environment: BTreeMap::new(),
            effective_config_root: config_root,
        },
    }))
}

fn unavailable(reason: impl Into<String>) -> DesktopAvailability {
    DesktopAvailability::Unavailable {
        warning: format!("Desktop attachment unavailable: {}", reason.into()),
    }
}

fn unavailable_failure(error: DiscoveryFailure) -> DesktopAvailability {
    DesktopAvailability::Unavailable {
        warning: error.warning(),
    }
}

pub(super) fn desktop_entry_path(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Option<PathBuf>, DiscoveryFailure> {
    for root in application_search_roots(environment)? {
        let candidate = root.join("applications").join(DESKTOP_ENTRY_NAME);
        match fs::symlink_metadata(&candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(DiscoveryFailure::unavailable(
                    "a Desktop entry cannot be inspected",
                ));
            }
        }
    }
    Ok(None)
}

pub(super) fn application_search_roots(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Vec<PathBuf>, DiscoveryFailure> {
    let home = environment_path(environment, "HOME")?;
    let data_home = environment
        .get(OsStr::new("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let mut roots = vec![data_home];
    match environment
        .get(OsStr::new("XDG_DATA_DIRS"))
        .filter(|value| !value.is_empty())
    {
        Some(value) => roots.extend(std::env::split_paths(value).filter(|path| path.is_absolute())),
        None => roots.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]),
    }
    Ok(roots)
}

fn resolve_launcher(
    executable: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, DiscoveryFailure> {
    if executable.is_absolute() {
        return validate_launcher(executable);
    }
    let path = environment.get(OsStr::new("PATH")).ok_or_else(|| {
        DiscoveryFailure::unavailable("PATH is unavailable for Desktop launcher resolution")
    })?;
    for root in std::env::split_paths(path).filter(|root| root.is_absolute()) {
        let candidate = root.join(executable);
        if let Ok(launcher) = validate_launcher(&candidate) {
            return Ok(launcher);
        }
    }
    Err(DiscoveryFailure::unavailable(
        "Desktop launcher is not an owner-owned executable",
    ))
}

pub(super) fn validate_launcher(path: &Path) -> Result<PathBuf, DiscoveryFailure> {
    let resolved = fs::canonicalize(path)
        .map_err(|_| DiscoveryFailure::unavailable("Desktop launcher cannot be resolved"))?;
    if !resolved.is_absolute() {
        return Err(DiscoveryFailure::unavailable(
            "Desktop launcher did not resolve to an absolute path",
        ));
    }
    validate_launcher_ancestor_chain(&resolved)?;
    let metadata = fs::symlink_metadata(&resolved)
        .map_err(|_| DiscoveryFailure::unavailable("Desktop launcher cannot be inspected"))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o100 == 0
    {
        return Err(DiscoveryFailure::unavailable(
            "Desktop launcher is not an owner-owned executable",
        ));
    }
    validate_launcher_ancestor_chain(&resolved)?;
    let post_validation = fs::symlink_metadata(&resolved)
        .map_err(|_| DiscoveryFailure::unavailable("Desktop launcher cannot be revalidated"))?;
    if !post_validation.file_type().is_file()
        || post_validation.uid() != effective_uid()
        || post_validation.mode() & 0o100 == 0
        || post_validation.dev() != metadata.dev()
        || post_validation.ino() != metadata.ino()
    {
        return Err(DiscoveryFailure::unavailable(
            "Desktop launcher changed during validation",
        ));
    }
    Ok(resolved)
}

fn validate_launcher_ancestor_chain(launcher_path: &Path) -> Result<(), DiscoveryFailure> {
    let parent = launcher_path
        .parent()
        .ok_or_else(|| DiscoveryFailure::unavailable("Desktop launcher has no parent directory"))?;
    let mut current = PathBuf::from("/");
    validate_launcher_ancestor(&current)?;
    for component in parent.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        current.push(component);
        validate_launcher_ancestor(&current)?;
    }
    Ok(())
}

fn validate_launcher_ancestor(path: &Path) -> Result<(), DiscoveryFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        DiscoveryFailure::unavailable("Desktop launcher ancestor cannot be inspected")
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(DiscoveryFailure::unavailable(
            "Desktop launcher ancestor is not a directory",
        ));
    }
    if metadata.uid() != effective_uid() && metadata.uid() != 0 {
        return Err(DiscoveryFailure::unavailable(
            "Desktop launcher ancestor is not owned by the effective user or root",
        ));
    }
    let mode = metadata.mode();
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        return Err(DiscoveryFailure::unavailable(
            "Desktop launcher ancestor permits cross-user replacement",
        ));
    }
    Ok(())
}

fn is_desktop_entry_path(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("desktop"))
}

pub(super) async fn inspect_build_info(
    launcher_path: &Path,
    fixed_args: &[OsString],
    environment: &BTreeMap<OsString, OsString>,
) -> Result<String, DiscoveryFailure> {
    let mut command = tokio::process::Command::new(launcher_path);
    command
        .arg("--print-build-info")
        .args(fixed_args)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(NATIVE_STAGE_TIMEOUT, command.output())
        .await
        .map_err(|_| DiscoveryFailure::unavailable("Desktop build-info inspection timed out"))?
        .map_err(|_| DiscoveryFailure::unavailable("Desktop build-info cannot be executed"))?;
    if !output.status.success() {
        return Err(DiscoveryFailure::unavailable(
            "Desktop build-info command failed",
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        DiscoveryFailure::unavailable("Desktop build-info is not one complete JSON object")
    })?;
    let object = value.as_object().ok_or_else(|| {
        DiscoveryFailure::unavailable("Desktop build-info is not one complete JSON object")
    })?;
    let app_id = object
        .get("appIdentity")
        .and_then(Value::as_object)
        .and_then(|identity| identity.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| DiscoveryFailure::unavailable("Desktop build-info has no appIdentity.id"))?;
    let capability_present = object
        .get("linuxCapabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some(DESKTOP_CAPABILITY))
        });
    if !capability_present {
        return Err(DiscoveryFailure::unavailable(
            "Desktop build-info lacks the attachment capability",
        ));
    }
    validate_app_id(app_id)?;
    Ok(app_id.to_owned())
}

pub(super) fn effective_config_root(
    environment: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, DiscoveryFailure> {
    match environment
        .get(OsStr::new("XDG_CONFIG_HOME"))
        .filter(|value| !value.is_empty())
    {
        Some(value) => absolute_environment_path("XDG_CONFIG_HOME", value),
        None => Ok(environment_path(environment, "HOME")?.join(".config")),
    }
}

fn environment_path(
    environment: &BTreeMap<OsString, OsString>,
    key: &'static str,
) -> Result<PathBuf, DiscoveryFailure> {
    let value = environment
        .get(OsStr::new(key))
        .ok_or_else(|| DiscoveryFailure::unavailable(format!("{key} is unavailable")))?;
    absolute_environment_path(key, value)
}

fn absolute_environment_path(
    key: &'static str,
    value: &OsStr,
) -> Result<PathBuf, DiscoveryFailure> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(DiscoveryFailure::unavailable(format!(
            "{key} is not absolute"
        )))
    }
}

pub(super) fn validate_app_id(app_id: &str) -> Result<(), DiscoveryFailure> {
    if app_id.is_empty()
        || app_id == "."
        || app_id == ".."
        || app_id.contains(['/', '\\'])
        || app_id.chars().any(char::is_control)
    {
        Err(DiscoveryFailure::unavailable(
            "Desktop appIdentity.id is not one safe path component",
        ))
    } else {
        Ok(())
    }
}

fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn desktop_error(reason: impl Into<String>) -> ControllerError {
    ControllerError::Operational(format!(
        "Desktop descriptor safety error: {}",
        reason.into()
    ))
}
