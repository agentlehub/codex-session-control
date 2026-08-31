use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub(crate) struct InitializedIdentity {
    codex_home: Option<PathBuf>,
    reported_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ThreadStorage {
    Active,
    Archived,
}

impl InitializedIdentity {
    pub(crate) fn from_initialize(codex_home: Option<&str>, user_agent: Option<&str>) -> Self {
        Self {
            codex_home: codex_home
                .filter(|home| !home.is_empty() && Path::new(home).is_absolute())
                .map(PathBuf::from),
            reported_version: user_agent.and_then(extract_codex_version),
        }
    }

    pub(crate) fn ordinary_codex_home(&self) -> Option<&Path> {
        self.codex_home.as_deref()
    }

    pub(crate) fn reported_version(&self) -> Option<&str> {
        self.reported_version.as_deref()
    }

    pub(crate) fn recovery_home(&self, expected_version: &str) -> Result<&Path, &'static str> {
        let Some(codex_home) = self.codex_home.as_deref() else {
            return Err("identity_unverified");
        };
        if !is_normalized_absolute_path(codex_home) || self.reported_version.is_none() {
            return Err("identity_unverified");
        }
        if self.reported_version() != Some(expected_version) {
            return Err("version_unsupported");
        }
        Ok(codex_home)
    }
}

pub(crate) fn is_normalized_absolute_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }

    let mut rebuilt = PathBuf::from("/");
    let mut normal_components = 0;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                rebuilt.push(component);
                normal_components += 1;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => return false,
        }
    }
    normal_components > 0 && rebuilt.as_os_str() == path.as_os_str()
}

pub(crate) fn owned_private_directory(
    owner: u32,
    mode: u32,
    euid: u32,
    is_directory: bool,
) -> bool {
    owner == euid && is_directory && mode & 0o7777 == 0o700
}

pub(crate) fn owned_private_socket(owner: u32, mode: u32, euid: u32, is_socket: bool) -> bool {
    owner == euid && is_socket && matches!(mode & 0o7777, 0o600 | 0o700)
}

pub(crate) fn classify_thread_storage(
    codex_home: &Path,
    expected_id: &str,
    reported_id: Option<&str>,
    path: &Path,
) -> Result<ThreadStorage, &'static str> {
    if reported_id != Some(expected_id) {
        return Err("mismatched_id");
    }
    if !path.is_absolute() {
        return Err("unclassifiable_storage");
    }
    let relative = path
        .strip_prefix(codex_home)
        .map_err(|_| "unclassifiable_storage")?;
    let mut components = relative.components();
    let Some(Component::Normal(storage)) = components.next() else {
        return Err("unclassifiable_storage");
    };
    let storage = if storage == "sessions" {
        ThreadStorage::Active
    } else if storage == "archived_sessions" {
        ThreadStorage::Archived
    } else {
        return Err("unclassifiable_storage");
    };
    let mut has_rollout_component = false;
    for component in components {
        let Component::Normal(component) = component else {
            return Err("unclassifiable_storage");
        };
        if matches!(component.to_str(), Some("sessions" | "archived_sessions")) {
            return Err("unclassifiable_storage");
        }
        has_rollout_component = true;
    }
    has_rollout_component
        .then_some(storage)
        .ok_or("unclassifiable_storage")
}

pub(super) fn extract_codex_version(user_agent: &str) -> Option<String> {
    user_agent
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
        })
        .find_map(|candidate| {
            semver::Version::parse(candidate)
                .ok()
                .map(|version| version.to_string())
        })
}
