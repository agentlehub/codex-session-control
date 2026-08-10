use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::AsFd,
        unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use crate::{app_server::socket_mode_is_owner_only, error::ControllerError};

pub(super) const SOCKET_SECURITY_REQUIREMENT: &str = "must be an owner-owned Unix socket with owner read/write permissions and no group/other permissions";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileKind {
    Directory,
    RegularFile,
    #[cfg(test)]
    UnixSocket,
}

pub(super) struct SelectedHomePath {
    pub(super) normalized: PathBuf,
    pub(super) canonical: PathBuf,
}

pub(super) fn validate_selected_codex_home(
    selected_home: &Path,
    config: &Path,
    effective_home: &Path,
    data_root: &Path,
    runtime_dir: &Path,
    euid: u32,
) -> Result<(), ControllerError> {
    let _ = inspect_selected_home_path(
        selected_home,
        config,
        effective_home,
        data_root,
        runtime_dir,
        euid,
    )?;
    Ok(())
}

pub(super) fn inspect_selected_home_path(
    selected_home: &Path,
    config: &Path,
    effective_home: &Path,
    data_root: &Path,
    runtime_dir: &Path,
    euid: u32,
) -> Result<SelectedHomePath, ControllerError> {
    let normalized = lexically_normalize_absolute(selected_home)?;
    if normalized != selected_home {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "path must be lexically normalized",
        });
    }
    let config_directory = config.parent().ok_or(ControllerError::InvalidData {
        field: "codex_home",
        reason: "product configuration directory is unavailable",
    })?;
    let normalized_config_directory = lexically_normalize_absolute(config_directory)?;
    if normalized == normalized_config_directory {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "must not overlap product-managed paths",
        });
    }

    let canonical = validated_selected_home_canonical_path(&normalized, effective_home, euid)?;
    let canonical_config_directory =
        canonicalize_from_existing_ancestor(&normalized_config_directory)?;
    let mut overlaps_product_path = canonical == canonical_config_directory;
    for root in [data_root, runtime_dir] {
        overlaps_product_path |= normalized.starts_with(root)
            || canonical.starts_with(canonicalize_from_existing_ancestor(root)?);
    }
    if overlaps_product_path {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "must not overlap product-managed paths",
        });
    }
    Ok(SelectedHomePath {
        normalized,
        canonical,
    })
}

pub(super) fn lexically_normalize_absolute(path: &Path) -> Result<PathBuf, ControllerError> {
    if !path.is_absolute() {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "path must be absolute",
        });
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(component) => normalized.push(component),
            Component::Prefix(_) => {
                return Err(ControllerError::InvalidData {
                    field: "codex_home",
                    reason: "path is not supported",
                });
            }
        }
    }
    Ok(normalized)
}

pub(super) fn canonicalize_from_existing_ancestor(path: &Path) -> Result<PathBuf, ControllerError> {
    let normalized = lexically_normalize_absolute(path)?;
    let mut current = normalized.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(current) {
            Ok(_) => {
                let mut canonical =
                    fs::canonicalize(current).map_err(|_| ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "existing ancestor cannot be canonicalized",
                    })?;
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = current.file_name().ok_or(ControllerError::InvalidData {
                    field: "codex_home",
                    reason: "existing ancestor is unavailable",
                })?;
                missing.push(component.to_os_string());
                current = current.parent().ok_or(ControllerError::InvalidData {
                    field: "codex_home",
                    reason: "existing ancestor is unavailable",
                })?;
            }
            Err(_) => {
                return Err(ControllerError::InvalidData {
                    field: "codex_home",
                    reason: "existing ancestor is inaccessible",
                });
            }
        }
    }
}

pub(super) fn validated_selected_home_canonical_path(
    selected_home: &Path,
    effective_home: &Path,
    euid: u32,
) -> Result<PathBuf, ControllerError> {
    if selected_home == Path::new("/") {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "root must not be the selected home",
        });
    }
    let ancestors = selected_home.ancestors().collect::<Vec<_>>();
    let checked_ancestors = ancestors
        .iter()
        .position(|ancestor| *ancestor == effective_home)
        .map(|home_index| &ancestors[..=home_index])
        .unwrap_or(&ancestors[..]);
    let mut entered_effective_user_tree = false;
    let mut existing_ancestor = None;
    let mut missing_leaf: Option<std::ffi::OsString> = None;

    for ancestor in checked_ancestors.iter().rev() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if missing_leaf.is_some()
                    || metadata.file_type().is_symlink()
                    || !metadata.file_type().is_dir()
                {
                    return Err(ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "existing ancestor has unsafe type",
                    });
                }
                if metadata.mode() & 0o022 != 0 {
                    return Err(ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "existing ancestor has unsafe mode",
                    });
                }
                if metadata.uid() == euid {
                    entered_effective_user_tree |= *ancestor != Path::new("/");
                    validate_existing(ancestor, FileKind::Directory, euid)?;
                } else if entered_effective_user_tree {
                    return Err(ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "ancestor ownership leaves effective-user tree",
                    });
                }
                existing_ancestor = Some(ancestor);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !entered_effective_user_tree {
                    return Err(ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "effective-user ancestor is unavailable",
                    });
                }
                if *ancestor != selected_home || missing_leaf.is_some() {
                    return Err(ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "only the selected-home leaf may be missing",
                    });
                }
                missing_leaf = ancestor.file_name().map(Into::into);
            }
            Err(_) => {
                return Err(ControllerError::InvalidData {
                    field: "codex_home",
                    reason: "existing ancestor is inaccessible",
                });
            }
        }
    }

    if !entered_effective_user_tree {
        return Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "effective-user ancestor is unavailable",
        });
    }
    let existing_ancestor = existing_ancestor.ok_or(ControllerError::InvalidData {
        field: "codex_home",
        reason: "effective-user ancestor is unavailable",
    })?;
    let mut canonical =
        fs::canonicalize(existing_ancestor).map_err(|_| ControllerError::InvalidData {
            field: "codex_home",
            reason: "existing ancestor cannot be canonicalized",
        })?;
    if let Some(missing_leaf) = missing_leaf {
        canonical.push(missing_leaf);
    }
    Ok(canonical)
}

pub(super) fn create_missing_selected_codex_home(
    selected_home: &Path,
    config: &Path,
    effective_home: &Path,
    data_root: &Path,
    runtime_dir: &Path,
    euid: u32,
) -> Result<(), ControllerError> {
    let inspected = inspect_selected_home_path(
        selected_home,
        config,
        effective_home,
        data_root,
        runtime_dir,
        euid,
    )?;
    match fs::symlink_metadata(selected_home) {
        Ok(_) => {
            debug_assert_eq!(inspected.normalized, selected_home);
            debug_assert!(inspected.canonical.is_absolute());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(selected_home) {
                Ok(()) => fs::set_permissions(selected_home, fs::Permissions::from_mode(0o700))
                    .map_err(|_| ControllerError::InvalidData {
                        field: "codex_home",
                        reason: "cannot set selected-home mode",
                    })
                    .and_then(|()| validate_existing(selected_home, FileKind::Directory, euid)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    inspect_selected_home_path(
                        selected_home,
                        config,
                        effective_home,
                        data_root,
                        runtime_dir,
                        euid,
                    )
                    .map(|_| ())
                }
                Err(_) => Err(ControllerError::InvalidData {
                    field: "codex_home",
                    reason: "cannot create selected-home leaf",
                }),
            }
        }
        Err(_) => Err(ControllerError::InvalidData {
            field: "codex_home",
            reason: "cannot inspect selected-home leaf",
        }),
    }
}

pub(super) fn validate_existing(
    path: &Path,
    expected: FileKind,
    euid: u32,
) -> Result<(), ControllerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ControllerError::InvalidData {
        field: "path",
        reason: "missing or inaccessible",
    })?;
    let file_type = metadata.file_type();
    let type_matches = match expected {
        FileKind::Directory => file_type.is_dir(),
        FileKind::RegularFile => file_type.is_file(),
        #[cfg(test)]
        FileKind::UnixSocket => file_type.is_socket(),
    };
    if file_type.is_symlink()
        || !type_matches
        || metadata.uid() != euid
        || metadata.mode() & 0o022 != 0
    {
        return Err(ControllerError::InvalidData {
            field: "path",
            reason: "unsafe owner, type, or mode",
        });
    }
    Ok(())
}

pub(super) fn resolve_codex_executable(
    path_environment: &OsStr,
    cwd: &Path,
) -> Result<PathBuf, ControllerError> {
    if !cwd.is_absolute() {
        return Err(ControllerError::InvalidData {
            field: "cwd",
            reason: "must be absolute",
        });
    }
    for directory in std::env::split_paths(path_environment) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            cwd.join(directory)
        };
        let candidate = directory.join("codex");
        let Ok(invocation_metadata) = fs::symlink_metadata(&candidate) else {
            continue;
        };
        if !(invocation_metadata.file_type().is_file()
            || invocation_metadata.file_type().is_symlink())
        {
            continue;
        }
        let Ok(target_metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if target_metadata.file_type().is_file() && target_metadata.mode() & 0o111 != 0 {
            return Ok(candidate);
        }
    }
    Err(ControllerError::InvalidData {
        field: "PATH",
        reason: "does not contain an executable Codex binary",
    })
}

pub(super) fn create_product_dir(path: &Path, euid: u32) -> Result<(), ControllerError> {
    create_owned_dir(path, euid, true)
}

pub(super) fn create_shared_dir(path: &Path, euid: u32) -> Result<(), ControllerError> {
    create_owned_dir(path, euid, false)
}

pub(super) fn create_owned_dir(
    path: &Path,
    euid: u32,
    enforce_private_final: bool,
) -> Result<(), ControllerError> {
    let mut ancestors = path.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    let mut entered_user_tree = false;
    let mut missing = Vec::new();
    for ancestor in ancestors {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.uid() == euid => {
                entered_user_tree = true;
                validate_existing(ancestor, FileKind::Directory, euid)?;
            }
            Ok(_) if entered_user_tree => {
                return Err(ControllerError::InvalidData {
                    field: "directory",
                    reason: "ancestor ownership leaves effective-user tree",
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && entered_user_tree => {
                missing.push(ancestor.to_path_buf());
            }
            Err(_) => {
                return Err(ControllerError::InvalidData {
                    field: "directory",
                    reason: "ancestor is inaccessible",
                });
            }
        }
    }
    if !entered_user_tree {
        return Err(ControllerError::InvalidData {
            field: "directory",
            reason: "effective-user ancestor is unavailable",
        });
    }
    for directory in missing {
        match fs::create_dir(&directory) {
            Ok(()) => {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(
                    |_| ControllerError::InvalidData {
                        field: "directory",
                        reason: "cannot set private mode",
                    },
                )?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                validate_existing(&directory, FileKind::Directory, euid)?;
            }
            Err(_) => {
                return Err(ControllerError::InvalidData {
                    field: "directory",
                    reason: "cannot create",
                });
            }
        }
    }
    validate_existing(path, FileKind::Directory, euid)?;
    if enforce_private_final {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
            ControllerError::InvalidData {
                field: "directory",
                reason: "cannot set private mode",
            }
        })?;
    }
    Ok(())
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), ControllerError> {
    let parent = path.parent().ok_or(ControllerError::InvalidData {
        field: "destination",
        reason: "has no parent directory",
    })?;
    let basename = path.file_name().ok_or(ControllerError::InvalidData {
        field: "destination",
        reason: "has no file name",
    })?;
    let prefix = format!(".{}-{}-", basename.to_string_lossy(), std::process::id());
    let mut temporary = tempfile::Builder::new()
        .prefix(&prefix)
        .tempfile_in(parent)
        .map_err(|_| ControllerError::InvalidData {
            field: "destination",
            reason: "cannot create sibling stage file",
        })?;
    temporary
        .write_all(bytes)
        .and_then(|()| {
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(mode))
        })
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| ControllerError::InvalidData {
            field: "destination",
            reason: "cannot write or sync stage file",
        })?;
    temporary.persist(path).map_err(|error| {
        let _ = error.file.close();
        ControllerError::InvalidData {
            field: "destination",
            reason: "cannot rename stage file",
        }
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ControllerError::InvalidData {
            field: "destination",
            reason: "cannot sync parent directory",
        })
}

#[derive(Clone, Copy, Debug)]
pub(super) enum StatusFileError {
    Missing,
    Unsafe,
    Unreadable,
}

pub(super) fn read_status_file(
    path: &Path,
    euid: u32,
    expected_mode: u32,
) -> Result<Vec<u8>, StatusFileError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StatusFileError::Missing);
        }
        Err(_) => return Err(StatusFileError::Unreadable),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != euid
        || metadata.mode() & 0o777 != expected_mode
    {
        return Err(StatusFileError::Unsafe);
    }
    fs::read(path).map_err(|_| StatusFileError::Unreadable)
}

pub(super) fn read_product_evidence_file(
    home: &Path,
    euid: u32,
    path: &Path,
    expected_mode: u32,
) -> Result<Vec<u8>, StatusFileError> {
    let parent = path.parent().ok_or(StatusFileError::Unsafe)?;
    let relative_parent = parent
        .strip_prefix(home)
        .map_err(|_| StatusFileError::Unsafe)?;
    if !home.is_absolute() {
        return Err(StatusFileError::Unsafe);
    }
    let mut home_components = home.components();
    if !matches!(home_components.next(), Some(Component::RootDir)) {
        return Err(StatusFileError::Unsafe);
    }
    let mut entered_effective_user_tree = false;
    let mut directory = open_evidence_directory(
        rustix::fs::CWD,
        Path::new("/"),
        euid,
        &mut entered_effective_user_tree,
    )?;
    for component in home_components {
        let Component::Normal(component) = component else {
            return Err(StatusFileError::Unsafe);
        };
        directory = open_evidence_directory(
            &directory,
            Path::new(component),
            euid,
            &mut entered_effective_user_tree,
        )?;
    }
    if !entered_effective_user_tree {
        return Err(StatusFileError::Unsafe);
    }
    for component in relative_parent.components() {
        let Component::Normal(component) = component else {
            return Err(StatusFileError::Unsafe);
        };
        directory = open_evidence_directory(
            &directory,
            Path::new(component),
            euid,
            &mut entered_effective_user_tree,
        )?;
    }

    let file_name = path.file_name().ok_or(StatusFileError::Unsafe)?;
    let file = rustix::fs::openat(
        &directory,
        Path::new(file_name),
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(status_file_open_error)?;
    let mut file = File::from(file);
    let metadata = file.metadata().map_err(|_| StatusFileError::Unreadable)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != euid
        || metadata.mode() & 0o777 != expected_mode
    {
        return Err(StatusFileError::Unsafe);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| StatusFileError::Unreadable)?;
    Ok(bytes)
}

pub(super) fn open_evidence_directory<Fd: AsFd>(
    parent: Fd,
    path: &Path,
    euid: u32,
    entered_effective_user_tree: &mut bool,
) -> Result<File, StatusFileError> {
    let directory = rustix::fs::openat(
        parent,
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(status_file_open_error)?;
    let directory = File::from(directory);
    let metadata = directory
        .metadata()
        .map_err(|_| StatusFileError::Unreadable)?;
    if !metadata.file_type().is_dir() || metadata.mode() & 0o022 != 0 {
        return Err(StatusFileError::Unsafe);
    }
    if metadata.uid() == euid {
        *entered_effective_user_tree = true;
    } else if *entered_effective_user_tree {
        return Err(StatusFileError::Unsafe);
    }
    Ok(directory)
}

fn status_file_open_error(error: rustix::io::Errno) -> StatusFileError {
    if error == rustix::io::Errno::NOENT {
        StatusFileError::Missing
    } else {
        StatusFileError::Unsafe
    }
}

pub(super) fn lifecycle_file_error(path: &Path, error: StatusFileError) -> String {
    match error {
        StatusFileError::Missing => format!("{} is missing", path.display()),
        StatusFileError::Unsafe => {
            format!("{} has unsafe owner, type, or mode", path.display())
        }
        StatusFileError::Unreadable => format!("{} is unreadable", path.display()),
    }
}

pub(super) fn validate_control_socket(path: &Path, euid: u32) -> Result<(), ControllerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| super::missing_control_socket_error())?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != euid
        || !socket_mode_is_owner_only(metadata.mode())
    {
        return Err(ControllerError::InvalidData {
            field: "socket",
            reason: SOCKET_SECURITY_REQUIREMENT,
        });
    }
    Ok(())
}

pub(crate) fn shell_quote_path(path: &Path) -> Result<String, ControllerError> {
    let value = path.to_str().ok_or(ControllerError::InvalidData {
        field: "path",
        reason: "shell recovery path must be UTF-8",
    })?;
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

pub(super) fn remove_owned_file(
    path: &Path,
    euid: u32,
    expected_mode: u32,
) -> Result<(), ControllerError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(ControllerError::InvalidData {
                field: "removal",
                reason: "cannot inspect path",
            });
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != euid
        || metadata.mode() & 0o777 != expected_mode
    {
        return Err(ControllerError::InvalidData {
            field: "removal",
            reason: "unsafe owner, type, or mode",
        });
    }
    fs::remove_file(path).map_err(|_| ControllerError::InvalidData {
        field: "removal",
        reason: "cannot remove file",
    })
}

pub(super) fn remove_owned_empty_dir(path: &Path, euid: u32) -> Result<(), ControllerError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(ControllerError::InvalidData {
                field: "removal",
                reason: "cannot inspect path",
            });
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != euid
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(ControllerError::InvalidData {
            field: "removal",
            reason: "unsafe owner, type, or mode",
        });
    }
    fs::remove_dir(path).map_err(|_| ControllerError::InvalidData {
        field: "removal",
        reason: "cannot remove directory",
    })
}

pub(super) fn remove_owned_tree(path: &Path, euid: u32) -> Result<(), ControllerError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(ControllerError::InvalidData {
                field: "removal",
                reason: "cannot inspect path",
            });
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != euid
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(ControllerError::InvalidData {
            field: "removal",
            reason: "unsafe owner, type, or mode",
        });
    }
    fs::remove_dir_all(path).map_err(|_| ControllerError::InvalidData {
        field: "removal",
        reason: "cannot remove directory",
    })
}

pub(super) fn reconcile_file(
    path: &Path,
    bytes: &[u8],
    mode: u32,
    euid: u32,
) -> Result<bool, ControllerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_existing(path, FileKind::RegularFile, euid)?;
            if metadata.mode() & 0o777 == mode
                && fs::read(path).is_ok_and(|found| found.as_slice() == bytes)
            {
                return Ok(false);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ControllerError::InvalidData {
                field: "path",
                reason: "cannot inspect destination",
            });
        }
    }
    atomic_write(path, bytes, mode)?;
    Ok(true)
}

#[cfg(test)]
pub(super) fn validate_config_file(path: &Path, euid: u32) -> Result<(), ControllerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ControllerError::InvalidData {
        field: "config",
        reason: "missing or inaccessible",
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != euid
        || metadata.mode() & 0o022 != 0
    {
        return Err(ControllerError::InvalidData {
            field: "config",
            reason: "unsafe owner, type, or mode",
        });
    }
    Ok(())
}
