use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

pub(crate) const BRIDGE_SOCKET_ENV: &str = "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET";
pub(crate) const RUNTIME_DIR_ENV: &str = "XDG_RUNTIME_DIR";
pub(crate) const APP_ID_ENV: &str = "CODEX_LINUX_APP_ID";
pub(crate) const DEFAULT_APP_ID: &str = "codex-desktop";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointResolutionError {
    TargetUnavailable,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedEndpoint {
    socket_path: PathBuf,
    derived_directories: Option<[PathBuf; 3]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EndpointMetadata {
    pub(crate) owner: u32,
    pub(crate) mode: u32,
    pub(crate) expected_type: bool,
}

impl ResolvedEndpoint {
    pub(crate) fn explicit(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            derived_directories: None,
        }
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub(crate) fn derived_directories(&self) -> Option<[&Path; 3]> {
        self.derived_directories
            .as_ref()
            .map(|paths| [&*paths[0], &*paths[1], &*paths[2]])
    }
}

pub(crate) fn resolve_endpoint_with(
    mut lookup: impl FnMut(&'static str) -> Option<OsString>,
) -> Result<ResolvedEndpoint, EndpointResolutionError> {
    if let Some(explicit) = lookup(BRIDGE_SOCKET_ENV)
        && !explicit.is_empty()
    {
        let socket_path = PathBuf::from(explicit);
        if !is_normalized_absolute_path(&socket_path) {
            return Err(EndpointResolutionError::Rejected);
        }
        return Ok(ResolvedEndpoint::explicit(socket_path));
    }

    let runtime_dir = lookup(RUNTIME_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(EndpointResolutionError::TargetUnavailable)?;
    if !is_normalized_absolute_path(&runtime_dir) {
        return Err(EndpointResolutionError::Rejected);
    }
    let app_id = match lookup(APP_ID_ENV) {
        Some(value) if !value.is_empty() => {
            let text = value.to_str().ok_or(EndpointResolutionError::Rejected)?;
            if matches!(text, "." | "..")
                || !text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(EndpointResolutionError::Rejected);
            }
            value
        }
        _ => OsString::from(DEFAULT_APP_ID),
    };
    let app_dir = runtime_dir.join(app_id);
    let bridge_dir = app_dir.join("app-server-bridge");
    let socket_path = bridge_dir.join("app-server.sock");
    Ok(ResolvedEndpoint {
        socket_path,
        derived_directories: Some([runtime_dir, app_dir, bridge_dir]),
    })
}

pub(crate) fn endpoint_metadata_is_safe(
    endpoint: &ResolvedEndpoint,
    euid: u32,
    derived: Option<[EndpointMetadata; 3]>,
    parent: EndpointMetadata,
    parent_is_canonical: bool,
    socket: EndpointMetadata,
) -> bool {
    let derived_is_safe = match (endpoint.derived_directories(), derived) {
        (None, None) => true,
        (Some(_), Some(metadata)) => metadata.into_iter().all(|metadata| {
            owned_private_directory(metadata.owner, metadata.mode, euid, metadata.expected_type)
        }),
        _ => false,
    };
    derived_is_safe
        && owned_private_directory(parent.owner, parent.mode, euid, parent.expected_type)
        && parent_is_canonical
        && owned_private_socket(socket.owner, socket.mode, euid, socket.expected_type)
}

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

    #[cfg_attr(
        test,
        allow(
            unfulfilled_lint_expectations,
            reason = "integration tests exercise this retained shared policy directly"
        )
    )]
    #[expect(
        dead_code,
        reason = "recovery identity validation is shared with retained app-server integration tests"
    )]
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

#[cfg_attr(
    test,
    allow(
        unfulfilled_lint_expectations,
        reason = "integration tests exercise this retained shared policy directly"
    )
)]
#[expect(
    dead_code,
    reason = "storage classification is shared with retained app-server integration tests"
)]
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
