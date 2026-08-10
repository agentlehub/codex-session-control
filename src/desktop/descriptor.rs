use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::AsFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rustix::fs::{CWD, Mode, OFlags, RenameFlags};
use serde::Deserialize;

use crate::{error::ControllerError, model::DesktopAttachmentIdentity};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DescriptorState {
    Absent,
    Expected,
    Foreign,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DescriptorPublicationResidue {
    Stage(PathBuf),
    Final(PathBuf),
}

impl DescriptorPublicationResidue {
    pub(crate) fn into_path(self) -> PathBuf {
        match self {
            Self::Stage(path) | Self::Final(path) => path,
        }
    }
}

#[derive(Debug)]
pub(crate) struct DescriptorPublicationFailure {
    pub(crate) source: ControllerError,
    pub(crate) residue: Option<DescriptorPublicationResidue>,
}

impl DescriptorPublicationFailure {
    fn clean(source: ControllerError) -> Self {
        Self {
            source,
            residue: None,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DescriptorPublicationTestPoint {
    BeforeStage,
    AfterStage { cleanup_unverified: bool },
    AfterRename,
}

pub(crate) fn render_descriptor(socket_path: &Path) -> Result<Vec<u8>, ControllerError> {
    let socket_path = normalized_descriptor_socket_path(socket_path)?;
    serde_json::to_vec(&DescriptorDocument {
        schema_version: 1,
        transport: "unix".to_owned(),
        socket_path,
    })
    .map_err(|_| ControllerError::Operational("Desktop descriptor cannot be rendered".to_owned()))
}

pub(crate) fn inspect_descriptor(
    identity: &DesktopAttachmentIdentity,
    expected: &[u8],
) -> Result<DescriptorState, ControllerError> {
    identity.validate()?;
    let expected = parse_descriptor(expected)?;
    let parent = match open_descriptor_parent(identity)? {
        Some(parent) => parent,
        None => return Ok(DescriptorState::Absent),
    };
    let file_name = identity
        .descriptor_path
        .file_name()
        .ok_or_else(|| desktop_error("descriptor path has no file name"))?;
    inspect_open_descriptor(&parent, file_name, &expected)
}

pub(super) fn inspect_open_descriptor(
    parent: &File,
    file_name: &OsStr,
    expected: &DescriptorDocument,
) -> Result<DescriptorState, ControllerError> {
    let file = match rustix::fs::openat(
        parent,
        Path::new(file_name),
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(file) => File::from(file),
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(DescriptorState::Absent),
        Err(_) => return Err(desktop_error("descriptor cannot be opened safely")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| desktop_error("descriptor cannot be inspected"))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(desktop_error(
            "descriptor is not an owner-only regular file",
        ));
    }
    let mut bytes = Vec::new();
    let mut file = file;
    file.read_to_end(&mut bytes)
        .map_err(|_| desktop_error("descriptor cannot be read"))?;
    let actual = parse_descriptor(&bytes)?;
    Ok(if &actual == expected {
        DescriptorState::Expected
    } else {
        DescriptorState::Foreign
    })
}

pub(crate) fn preflight_descriptor_switch(
    old: Option<&DesktopAttachmentIdentity>,
    new: &DesktopAttachmentIdentity,
    expected: &[u8],
) -> Result<(), ControllerError> {
    new.validate()?;
    if let Some(old) = old {
        let old_state = inspect_descriptor(old, expected)?;
        if !matches!(
            old_state,
            DescriptorState::Absent | DescriptorState::Expected
        ) {
            return Err(desktop_error("existing Desktop descriptor is foreign"));
        }
        if old.descriptor_path == new.descriptor_path {
            return Ok(());
        }
    }
    let new_state = inspect_descriptor(new, expected)?;
    if matches!(
        new_state,
        DescriptorState::Absent | DescriptorState::Expected
    ) {
        Ok(())
    } else {
        Err(desktop_error("proposed Desktop descriptor is foreign"))
    }
}

static DESCRIPTOR_TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn publish_descriptor(
    identity: &DesktopAttachmentIdentity,
    expected: &[u8],
) -> Result<bool, DescriptorPublicationFailure> {
    #[cfg(test)]
    {
        publish_descriptor_internal(identity, expected, None)
    }
    #[cfg(not(test))]
    {
        publish_descriptor_internal(identity, expected)
    }
}

#[cfg(test)]
pub(super) fn publish_descriptor_with_test_point(
    identity: &DesktopAttachmentIdentity,
    expected: &[u8],
    test_point: DescriptorPublicationTestPoint,
) -> Result<bool, DescriptorPublicationFailure> {
    publish_descriptor_internal(identity, expected, Some(test_point))
}

fn publish_descriptor_internal(
    identity: &DesktopAttachmentIdentity,
    expected: &[u8],
    #[cfg(test)] test_point: Option<DescriptorPublicationTestPoint>,
) -> Result<bool, DescriptorPublicationFailure> {
    identity
        .validate()
        .map_err(DescriptorPublicationFailure::clean)?;
    let expected_document =
        parse_descriptor(expected).map_err(DescriptorPublicationFailure::clean)?;
    prepare_descriptor_parent(identity).map_err(DescriptorPublicationFailure::clean)?;
    let parent = open_descriptor_parent(identity)
        .map_err(DescriptorPublicationFailure::clean)?
        .ok_or_else(|| {
            DescriptorPublicationFailure::clean(desktop_error(
                "descriptor parent disappeared after preparation",
            ))
        })?;
    let file_name = identity.descriptor_path.file_name().ok_or_else(|| {
        DescriptorPublicationFailure::clean(desktop_error("descriptor path has no file name"))
    })?;
    match inspect_open_descriptor(&parent, file_name, &expected_document)
        .map_err(DescriptorPublicationFailure::clean)?
    {
        DescriptorState::Expected => return Ok(false),
        DescriptorState::Foreign => {
            return Err(DescriptorPublicationFailure::clean(desktop_error(
                "Desktop descriptor is foreign",
            )));
        }
        DescriptorState::Absent => {}
    }

    #[cfg(test)]
    if test_point == Some(DescriptorPublicationTestPoint::BeforeStage) {
        return Err(DescriptorPublicationFailure::clean(desktop_error(
            "injected failure before descriptor stage",
        )));
    }

    let temporary = OsString::from(format!(
        ".{}-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        DESCRIPTOR_TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    let temporary_path = Path::new(&temporary);
    let temporary_full_path = identity
        .descriptor_path
        .parent()
        .expect("validated descriptor has a parent")
        .join(temporary_path);
    let file = rustix::fs::openat(
        &parent,
        temporary_path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(|_| {
        DescriptorPublicationFailure::clean(desktop_error(
            "descriptor stage file cannot be created safely",
        ))
    })?;

    #[cfg(test)]
    if let Some(DescriptorPublicationTestPoint::AfterStage { cleanup_unverified }) = test_point {
        drop(file);
        let residue = if cleanup_unverified {
            Some(DescriptorPublicationResidue::Stage(temporary_full_path))
        } else {
            cleanup_descriptor_stage(&parent, temporary_path, &temporary_full_path)
        };
        return Err(DescriptorPublicationFailure {
            source: desktop_error("injected failure after descriptor stage"),
            residue,
        });
    }

    let mut renamed = false;
    let result = (|| {
        let mut file = file;
        file.write_all(expected)
            .and_then(|()| file.set_permissions(fs::Permissions::from_mode(0o600)))
            .and_then(|()| file.sync_all())
            .map_err(|_| desktop_error("descriptor stage file cannot be written or synced"))?;
        drop(file);
        rustix::fs::renameat_with(
            &parent,
            temporary_path,
            &parent,
            Path::new(file_name),
            RenameFlags::NOREPLACE,
        )
        .map_err(|_| desktop_error("descriptor changed during publication"))?;
        renamed = true;
        #[cfg(test)]
        if test_point == Some(DescriptorPublicationTestPoint::AfterRename) {
            return Err(desktop_error("injected failure after descriptor rename"));
        }
        parent
            .sync_all()
            .map_err(|_| desktop_error("descriptor parent cannot be synced"))?;
        match inspect_open_descriptor(&parent, file_name, &expected_document)? {
            DescriptorState::Expected => Ok(true),
            DescriptorState::Absent | DescriptorState::Foreign => Err(desktop_error(
                "descriptor cannot be revalidated after publication",
            )),
        }
    })();
    match result {
        Ok(changed) => Ok(changed),
        Err(source) => {
            let residue = if renamed {
                Some(DescriptorPublicationResidue::Final(
                    identity.descriptor_path.clone(),
                ))
            } else {
                cleanup_descriptor_stage(&parent, temporary_path, &temporary_full_path)
            };
            Err(DescriptorPublicationFailure { source, residue })
        }
    }
}

fn cleanup_descriptor_stage(
    parent: &File,
    temporary_path: &Path,
    temporary_full_path: &Path,
) -> Option<DescriptorPublicationResidue> {
    let residue = || {
        Some(DescriptorPublicationResidue::Stage(
            temporary_full_path.to_owned(),
        ))
    };
    if rustix::fs::unlinkat(parent, temporary_path, rustix::fs::AtFlags::empty()).is_err()
        || parent.sync_all().is_err()
    {
        return residue();
    }
    match rustix::fs::openat(
        parent,
        temporary_path,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Err(error) if error == rustix::io::Errno::NOENT => None,
        Ok(file) => {
            drop(file);
            residue()
        }
        Err(_) => residue(),
    }
}

pub(crate) fn remove_expected_descriptor(
    identity: &DesktopAttachmentIdentity,
    expected: &[u8],
) -> Result<bool, ControllerError> {
    identity.validate()?;
    let expected_document = parse_descriptor(expected)?;
    let Some(parent) = open_descriptor_parent(identity)? else {
        return Ok(false);
    };
    let file_name = identity
        .descriptor_path
        .file_name()
        .ok_or_else(|| desktop_error("descriptor path has no file name"))?;
    match inspect_open_descriptor(&parent, file_name, &expected_document)? {
        DescriptorState::Absent => Ok(false),
        DescriptorState::Foreign => Err(desktop_error("Desktop descriptor is foreign")),
        DescriptorState::Expected => {
            rustix::fs::unlinkat(&parent, Path::new(file_name), rustix::fs::AtFlags::empty())
                .map_err(|_| desktop_error("descriptor cannot be removed safely"))?;
            parent
                .sync_all()
                .map_err(|_| desktop_error("descriptor parent cannot be synced"))?;
            Ok(true)
        }
    }
}

fn normalized_descriptor_socket_path(socket_path: &Path) -> Result<String, ControllerError> {
    let value = socket_path
        .to_str()
        .ok_or_else(|| desktop_error("descriptor socket path must be UTF-8"))?;
    let mut lexical = String::from("/");
    let mut components = socket_path
        .components()
        .filter_map(|component| match component {
            Component::Normal(component) => component.to_str(),
            _ => None,
        });
    if let Some(first) = components.next() {
        lexical.push_str(first);
        for component in components {
            lexical.push('/');
            lexical.push_str(component);
        }
    }
    if value.is_empty()
        || !socket_path.is_absolute()
        || value.contains("//")
        || value.ends_with('/') && value != "/"
        || value != lexical
        || value.chars().any(|character| character.is_control())
        || socket_path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(desktop_error(
            "descriptor socket path is not a normalized absolute path",
        ));
    }
    Ok(value.to_owned())
}

#[derive(Debug, Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DescriptorDocument {
    schema_version: u32,
    transport: String,
    socket_path: String,
}

pub(super) fn parse_descriptor(bytes: &[u8]) -> Result<DescriptorDocument, ControllerError> {
    let descriptor: DescriptorDocument =
        serde_json::from_slice(bytes).map_err(|_| desktop_error("descriptor JSON is invalid"))?;
    if descriptor.schema_version != 1 || descriptor.transport != "unix" {
        return Err(desktop_error("descriptor schema is unsupported"));
    }
    normalized_descriptor_socket_path(Path::new(&descriptor.socket_path))?;
    Ok(descriptor)
}

pub(crate) fn prepare_descriptor_parent(
    identity: &DesktopAttachmentIdentity,
) -> Result<(), ControllerError> {
    identity.validate()?;
    let app_directory = identity
        .descriptor_path
        .parent()
        .ok_or_else(|| desktop_error("descriptor path has no parent"))?;
    let config_root = app_directory
        .parent()
        .ok_or_else(|| desktop_error("descriptor config root is missing"))?;
    ensure_safe_config_directory(config_root)?;
    ensure_safe_app_directory(app_directory)?;
    Ok(())
}

fn ensure_safe_config_directory(path: &Path) -> Result<(), ControllerError> {
    let parent = path
        .parent()
        .ok_or_else(|| desktop_error("descriptor config root has no parent"))?;
    validate_existing_ancestor_chain(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => validate_existing_owned_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_existing_owned_directory(parent)?;
            create_private_directory(path)
        }
        Err(_) => Err(desktop_error("descriptor config root cannot be inspected")),
    }
}

fn ensure_safe_app_directory(path: &Path) -> Result<(), ControllerError> {
    let parent = path
        .parent()
        .ok_or_else(|| desktop_error("descriptor application directory has no parent"))?;
    validate_existing_ancestor_chain(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => validate_existing_owned_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_existing_owned_directory(parent)?;
            create_private_directory(path)
        }
        Err(_) => Err(desktop_error(
            "descriptor application directory cannot be inspected",
        )),
    }
}

fn create_private_directory(path: &Path) -> Result<(), ControllerError> {
    let parent_path = path
        .parent()
        .ok_or_else(|| desktop_error("descriptor directory has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| desktop_error("descriptor directory has no name"))?;
    let parent = open_existing_owned_directory(parent_path)?;
    match rustix::fs::mkdirat(&parent, Path::new(name), Mode::RWXU) {
        Ok(()) => {
            let mut entered_user_tree = true;
            let directory = open_directory(&parent, name, &mut entered_user_tree)?
                .ok_or_else(|| desktop_error("new descriptor directory is missing"))?;
            let metadata = directory
                .metadata()
                .map_err(|_| desktop_error("new descriptor directory cannot be revalidated"))?;
            if !metadata.file_type().is_dir()
                || metadata.uid() != effective_uid()
                || metadata.mode() & 0o777 != 0o700
            {
                return Err(desktop_error("new descriptor directory is unsafe"));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_existing_owned_directory(path)
        }
        Err(_) => Err(desktop_error("descriptor directory cannot be created")),
    }
}

fn open_existing_owned_directory(path: &Path) -> Result<File, ControllerError> {
    let mut directory = open_root_directory()?;
    let mut entered_user_tree = directory
        .metadata()
        .map_err(|_| desktop_error("descriptor ancestor cannot be inspected"))?
        .uid()
        == effective_uid();
    for component in path.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        directory = open_directory(&directory, component, &mut entered_user_tree)?
            .ok_or_else(|| desktop_error("descriptor ancestor is missing"))?;
    }
    let metadata = directory
        .metadata()
        .map_err(|_| desktop_error("descriptor directory cannot be inspected"))?;
    if !entered_user_tree
        || metadata.uid() != effective_uid()
        || !metadata.file_type().is_dir()
        || metadata.mode() & 0o022 != 0
    {
        return Err(desktop_error(
            "descriptor directory is not a safe owner-owned directory",
        ));
    }
    Ok(directory)
}

fn validate_existing_ancestor_chain(path: &Path) -> Result<(), ControllerError> {
    let mut directory = open_root_directory()?;
    let mut entered_user_tree = directory
        .metadata()
        .map_err(|_| desktop_error("descriptor ancestor cannot be inspected"))?
        .uid()
        == effective_uid();
    for component in path.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        directory = open_directory(&directory, component, &mut entered_user_tree)?
            .ok_or_else(|| desktop_error("descriptor ancestor is missing"))?;
    }
    Ok(())
}

fn validate_existing_owned_directory(path: &Path) -> Result<(), ControllerError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| desktop_error("descriptor directory cannot be inspected"))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o022 != 0
    {
        return Err(desktop_error(
            "descriptor directory is not a safe owner-owned directory",
        ));
    }
    Ok(())
}

pub(super) fn open_descriptor_parent(
    identity: &DesktopAttachmentIdentity,
) -> Result<Option<File>, ControllerError> {
    let parent = identity
        .descriptor_path
        .parent()
        .ok_or_else(|| desktop_error("descriptor path has no parent"))?;
    let mut directory = open_root_directory()?;
    let mut entered_user_tree = directory
        .metadata()
        .map_err(|_| desktop_error("descriptor ancestor cannot be inspected"))?
        .uid()
        == effective_uid();
    for component in parent.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        match open_directory(&directory, component, &mut entered_user_tree)? {
            Some(next) => directory = next,
            None => return Ok(None),
        }
    }
    if !entered_user_tree {
        return Err(desktop_error(
            "descriptor parent is not owned by the effective user",
        ));
    }
    Ok(Some(directory))
}

fn open_root_directory() -> Result<File, ControllerError> {
    rustix::fs::openat(
        CWD,
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| desktop_error("descriptor root cannot be opened safely"))
}

fn open_directory(
    parent: &File,
    component: &OsStr,
    entered_user_tree: &mut bool,
) -> Result<Option<File>, ControllerError> {
    let directory = rustix::fs::openat(
        parent.as_fd(),
        Path::new(component),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    );
    let directory = match directory {
        Ok(directory) => File::from(directory),
        Err(error) if error == rustix::io::Errno::NOENT => {
            return Ok(None);
        }
        Err(_) => return Err(desktop_error("descriptor ancestor cannot be opened safely")),
    };
    let metadata = directory
        .metadata()
        .map_err(|_| desktop_error("descriptor ancestor cannot be inspected"))?;
    if !metadata.file_type().is_dir() || metadata.mode() & 0o022 != 0 {
        return Err(desktop_error("descriptor ancestor is unsafe"));
    }
    if metadata.uid() == effective_uid() {
        *entered_user_tree = true;
    } else if *entered_user_tree {
        return Err(desktop_error(
            "descriptor ancestor leaves the effective-user tree",
        ));
    }
    Ok(Some(directory))
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
