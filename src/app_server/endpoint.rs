use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
};

use rustix::fs::{FileType, Stat};

use crate::error::{ToolErrorCategory, ToolErrorData};

const CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET: &str = "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET";
const XDG_RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";
const CODEX_LINUX_APP_ID: &str = "CODEX_LINUX_APP_ID";
const DEFAULT_CODEX_LINUX_APP_ID: &str = "codex-desktop";
const SOCKET_VALIDATION_STAGE: &str = "socket_validation";

#[derive(Debug)]
pub(crate) struct DesktopEndpoint {
    socket_path: PathBuf,
    kind: EndpointKind,
}

#[derive(Debug)]
enum EndpointKind {
    Explicit,
    Derived {
        runtime_dir: PathBuf,
        app_dir: PathBuf,
        bridge_dir: PathBuf,
    },
}

impl DesktopEndpoint {
    pub(crate) fn resolve() -> Result<Self, ToolErrorData> {
        Self::resolve_with(env::var_os::<&'static str>)
    }

    fn resolve_with(
        mut lookup: impl FnMut(&'static str) -> Option<OsString>,
    ) -> Result<Self, ToolErrorData> {
        if let Some(explicit) = lookup(CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET)
            && !explicit.is_empty()
        {
            let socket_path = PathBuf::from(explicit);
            require_normalized_absolute_path(&socket_path)?;
            return Ok(Self::explicit(socket_path));
        }

        let runtime_dir = lookup(XDG_RUNTIME_DIR)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(target_unavailable)?;
        require_normalized_absolute_path(&runtime_dir)?;

        let app_id = match lookup(CODEX_LINUX_APP_ID) {
            Some(value) if !value.is_empty() => {
                require_valid_app_id(&value)?;
                value
            }
            _ => OsString::from(DEFAULT_CODEX_LINUX_APP_ID),
        };
        let app_dir = runtime_dir.join(app_id);
        let bridge_dir = app_dir.join("app-server-bridge");
        let socket_path = bridge_dir.join("app-server.sock");

        Ok(Self {
            socket_path,
            kind: EndpointKind::Derived {
                runtime_dir,
                app_dir,
                bridge_dir,
            },
        })
    }

    pub(super) fn explicit(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            kind: EndpointKind::Explicit,
        }
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub(crate) fn validate(&self) -> Result<(), ToolErrorData> {
        require_normalized_absolute_path(&self.socket_path)?;

        if let EndpointKind::Derived {
            runtime_dir,
            app_dir,
            bridge_dir,
        } = &self.kind
        {
            validate_directory(runtime_dir)?;
            validate_directory(app_dir)?;
            validate_directory(bridge_dir)?;
        }

        let parent = self
            .socket_path
            .parent()
            .ok_or_else(socket_validation_failure)?;
        validate_directory(parent)?;
        let canonical_parent = fs::canonicalize(parent).map_err(|_| socket_validation_failure())?;
        if canonical_parent.as_os_str() != parent.as_os_str() {
            return Err(socket_validation_failure());
        }

        let stat = selected_lstat(&self.socket_path)?;
        let euid = rustix::process::geteuid().as_raw();
        if !owned_socket_is_private(stat.st_uid, stat.st_mode, euid) {
            return Err(socket_validation_failure());
        }

        Ok(())
    }
}

fn require_valid_app_id(value: &OsStr) -> Result<(), ToolErrorData> {
    let value = value.to_str().ok_or_else(socket_validation_failure)?;
    if matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(socket_validation_failure());
    }
    Ok(())
}

fn require_normalized_absolute_path(path: &Path) -> Result<(), ToolErrorData> {
    if !path.is_absolute() {
        return Err(socket_validation_failure());
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
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(socket_validation_failure());
            }
        }
    }

    if normal_components == 0 || rebuilt.as_os_str() != path.as_os_str() {
        return Err(socket_validation_failure());
    }
    Ok(())
}

fn validate_directory(path: &Path) -> Result<(), ToolErrorData> {
    let stat = selected_lstat(path)?;
    let euid = rustix::process::geteuid().as_raw();
    if !owned_directory_is_private(stat.st_uid, stat.st_mode, euid) {
        return Err(socket_validation_failure());
    }
    Ok(())
}

fn selected_lstat(path: &Path) -> Result<Stat, ToolErrorData> {
    match rustix::fs::lstat(path) {
        Ok(stat) => Ok(stat),
        Err(rustix::io::Errno::NOENT) => Err(target_unavailable()),
        Err(_) => Err(socket_validation_failure()),
    }
}

fn owned_directory_is_private(st_uid: u32, st_mode: u32, euid: u32) -> bool {
    st_uid == euid
        && FileType::from_raw_mode(st_mode) == FileType::Directory
        && st_mode & 0o7777 == 0o700
}

fn owned_socket_is_private(st_uid: u32, st_mode: u32, euid: u32) -> bool {
    st_uid == euid
        && FileType::from_raw_mode(st_mode) == FileType::Socket
        && matches!(st_mode & 0o7777, 0o600 | 0o700)
}

fn target_unavailable() -> ToolErrorData {
    ToolErrorData::fixed(
        ToolErrorCategory::TargetUnavailable,
        "native_transport",
        SOCKET_VALIDATION_STAGE,
    )
}

fn socket_validation_failure() -> ToolErrorData {
    ToolErrorData::fixed(
        ToolErrorCategory::AuthorityTransportFailure,
        "native_transport",
        SOCKET_VALIDATION_STAGE,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        ffi::OsString,
        fs,
        os::unix::{
            ffi::OsStringExt,
            fs::{PermissionsExt, symlink},
            net::UnixListener,
        },
        path::{Path, PathBuf},
    };

    use rustix::fs::FileType;

    use super::*;
    use crate::error::{ToolErrorCategory, ToolErrorData};

    const DEFAULT_APP_ID: &str = "codex-desktop";

    struct PrivateEndpointFixture {
        _root: tempfile::TempDir,
        runtime_dir: PathBuf,
        app_dir: PathBuf,
        bridge_dir: PathBuf,
        socket_path: PathBuf,
        listener: Option<UnixListener>,
    }

    #[derive(Clone, Copy, Debug)]
    enum SelectedEntry {
        RuntimeDirectory,
        AppDirectory,
        BridgeDirectory,
        Socket,
    }

    impl PrivateEndpointFixture {
        fn new() -> Self {
            let root = crate::test_support::private_tempdir();
            let runtime_dir = root.path().join("runtime");
            let app_dir = runtime_dir.join(DEFAULT_APP_ID);
            let bridge_dir = app_dir.join("app-server-bridge");
            create_private_directory(&runtime_dir);
            create_private_directory(&app_dir);
            create_private_directory(&bridge_dir);
            let socket_path = bridge_dir.join("app-server.sock");
            let listener = UnixListener::bind(&socket_path).unwrap();
            set_mode(&socket_path, 0o600);

            Self {
                _root: root,
                runtime_dir,
                app_dir,
                bridge_dir,
                socket_path,
                listener: Some(listener),
            }
        }

        fn derived_endpoint(&self) -> DesktopEndpoint {
            resolve_values(None, Some(self.runtime_dir.clone().into_os_string()), None).unwrap()
        }

        fn explicit_endpoint(&self) -> DesktopEndpoint {
            DesktopEndpoint::explicit(self.socket_path.clone())
        }

        fn selected_path(&self, selected: SelectedEntry) -> &Path {
            match selected {
                SelectedEntry::RuntimeDirectory => &self.runtime_dir,
                SelectedEntry::AppDirectory => &self.app_dir,
                SelectedEntry::BridgeDirectory => &self.bridge_dir,
                SelectedEntry::Socket => &self.socket_path,
            }
        }

        fn close_listener(&mut self) {
            drop(self.listener.take());
        }
    }

    fn create_private_directory(path: &Path) {
        fs::create_dir(path).unwrap();
        set_mode(path, 0o700);
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }

    fn resolve_values(
        explicit: Option<OsString>,
        runtime_dir: Option<OsString>,
        app_id: Option<OsString>,
    ) -> Result<DesktopEndpoint, ToolErrorData> {
        DesktopEndpoint::resolve_with(|name| match name {
            CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET => explicit.clone(),
            XDG_RUNTIME_DIR => runtime_dir.clone(),
            CODEX_LINUX_APP_ID => app_id.clone(),
            _ => panic!("unexpected lookup: {name}"),
        })
    }

    fn assert_validation_failure(error: &ToolErrorData) {
        assert_eq!(error.category, ToolErrorCategory::AuthorityTransportFailure);
        assert_eq!(error.stage, "socket_validation");
    }

    fn assert_target_unavailable(error: &ToolErrorData) {
        assert_eq!(error.category, ToolErrorCategory::TargetUnavailable);
    }

    fn rendered_error(error: &ToolErrorData) -> String {
        format!("{error:?} {}", serde_json::to_string(error).unwrap())
    }

    fn replace_with_symlink(path: &Path) {
        let target = path.with_file_name(format!(
            "{}.real",
            path.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(path, &target).unwrap();
        symlink(&target, path).unwrap();
    }

    fn replace_with_regular_file(path: &Path) {
        if path.is_dir() {
            fs::remove_dir_all(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
        fs::File::create(path).unwrap();
    }

    #[test]
    fn explicit_socket_precedes_other_inputs_and_preserves_non_utf8_bytes() {
        let selected = OsString::from_vec(b"/tmp/codex-desktop-\xff.sock".to_vec());
        let endpoint = DesktopEndpoint::resolve_with(|name| {
            if name == CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET {
                Some(selected.clone())
            } else {
                panic!("explicit selection read {name}")
            }
        })
        .unwrap();

        assert_eq!(endpoint.socket_path().as_os_str(), selected.as_os_str());
    }

    #[test]
    fn malformed_explicit_socket_fails_without_consulting_fallback_inputs() {
        let error = DesktopEndpoint::resolve_with(|name| {
            if name == CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET {
                Some(OsString::from("relative.sock"))
            } else {
                panic!("malformed explicit selection read {name}")
            }
        })
        .unwrap_err();

        assert_validation_failure(&error);
    }

    #[test]
    fn empty_explicit_socket_falls_through_to_the_derived_endpoint() {
        let root = crate::test_support::private_tempdir();
        let endpoint = resolve_values(
            Some(OsString::new()),
            Some(root.path().as_os_str().to_owned()),
            Some(OsString::from("Agentle_1.2-x")),
        )
        .unwrap();

        assert_eq!(
            endpoint.socket_path(),
            root.path()
                .join("Agentle_1.2-x")
                .join("app-server-bridge")
                .join("app-server.sock")
        );
    }

    #[test]
    fn resolution_reads_fresh_values_instead_of_caching_a_selection() {
        let lookup_count = Cell::new(0);
        let mut lookup = |name| {
            assert_eq!(name, CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET);
            let next = lookup_count.get() + 1;
            lookup_count.set(next);
            Some(OsString::from(format!("/tmp/selection-{next}.sock")))
        };

        let first = DesktopEndpoint::resolve_with(&mut lookup).unwrap();
        let second = DesktopEndpoint::resolve_with(&mut lookup).unwrap();

        assert_eq!(first.socket_path(), Path::new("/tmp/selection-1.sock"));
        assert_eq!(second.socket_path(), Path::new("/tmp/selection-2.sock"));
        assert_eq!(lookup_count.get(), 2);
    }

    #[test]
    fn missing_or_empty_runtime_directory_is_target_unavailable() {
        for (label, runtime_dir) in [("missing", None), ("empty", Some(OsString::new()))] {
            let error = resolve_values(None, runtime_dir, None).unwrap_err();
            assert_target_unavailable(&error);
            assert_eq!(error.stage, "socket_validation", "{label}");
        }
    }

    #[test]
    fn app_id_defaults_only_when_missing_or_empty() {
        let root = crate::test_support::private_tempdir();
        for (label, app_id) in [("missing", None), ("empty", Some(OsString::new()))] {
            let endpoint =
                resolve_values(None, Some(root.path().as_os_str().to_owned()), app_id).unwrap();
            assert_eq!(
                endpoint.socket_path(),
                root.path()
                    .join(DEFAULT_APP_ID)
                    .join("app-server-bridge")
                    .join("app-server.sock"),
                "{label}"
            );
        }
    }

    #[test]
    fn valid_nondefault_app_id_is_used_verbatim() {
        let root = crate::test_support::private_tempdir();
        let endpoint = resolve_values(
            None,
            Some(root.path().as_os_str().to_owned()),
            Some(OsString::from("Agentle_1.2-x")),
        )
        .unwrap();

        assert_eq!(
            endpoint.socket_path(),
            root.path()
                .join("Agentle_1.2-x")
                .join("app-server-bridge")
                .join("app-server.sock")
        );
    }

    #[test]
    fn invalid_app_id_classes_fail_closed() {
        let root = crate::test_support::private_tempdir();
        for (label, app_id) in [
            (
                "invalid UTF-8",
                OsString::from_vec(vec![b'c', b'o', b'd', b'e', b'x', 0xff]),
            ),
            ("reserved dot", OsString::from(".")),
            ("reserved dot-dot", OsString::from("..")),
            ("disallowed ASCII", OsString::from("codex/desktop")),
            ("non-ASCII", OsString::from("codex-desktóp")),
        ] {
            let error =
                resolve_values(None, Some(root.path().as_os_str().to_owned()), Some(app_id))
                    .unwrap_err();
            assert_validation_failure(&error);
            assert_eq!(error.stage, "socket_validation", "{label}");
        }
    }

    #[test]
    fn explicit_and_runtime_paths_require_exact_absolute_lexical_normalization() {
        let root = crate::test_support::private_tempdir();
        let base = root.path().to_string_lossy();
        let malformed = [
            ("relative", OsString::from("relative/socket")),
            ("non-root prefix", OsString::from("C:/socket")),
            ("dot component", OsString::from(format!("{base}/./socket"))),
            (
                "parent component",
                OsString::from(format!("{base}/nested/../socket")),
            ),
            (
                "repeated separator",
                OsString::from(format!("{base}//socket")),
            ),
            (
                "trailing separator",
                OsString::from(format!("{base}/socket/")),
            ),
            ("root without basename", OsString::from("/")),
            ("repeated root separator", OsString::from("//tmp/socket")),
        ];

        for (label, path) in malformed {
            let explicit_error = resolve_values(Some(path.clone()), None, None).unwrap_err();
            assert_validation_failure(&explicit_error);

            let runtime_error = resolve_values(None, Some(path), None).unwrap_err();
            assert_validation_failure(&runtime_error);
            assert_eq!(runtime_error.stage, "socket_validation", "{label}");
        }
    }

    #[test]
    fn errors_never_render_selected_paths_or_environment_values() {
        let root = crate::test_support::private_tempdir();
        let selected_path = "RELATIVE_SELECTED_PATH_SENTINEL";
        let app_id = "APP_ID_VALUE_SENTINEL!";
        let explicit_error = resolve_values(
            Some(OsString::from(selected_path)),
            Some(root.path().as_os_str().to_owned()),
            Some(OsString::from(app_id)),
        )
        .unwrap_err();
        let app_id_error = resolve_values(
            None,
            Some(root.path().as_os_str().to_owned()),
            Some(OsString::from(app_id)),
        )
        .unwrap_err();

        for error in [&explicit_error, &app_id_error] {
            let rendered = rendered_error(error);
            assert!(!rendered.contains(selected_path));
            assert!(!rendered.contains(app_id));
            assert!(!rendered.contains(root.path().to_string_lossy().as_ref()));
        }
    }

    #[test]
    fn directory_predicate_requires_type_owner_and_exact_private_mode() {
        let euid = 1000;
        let directory = FileType::Directory.as_raw_mode();
        let regular = FileType::RegularFile.as_raw_mode();
        assert!(owned_directory_is_private(euid, directory | 0o700, euid));

        for (label, uid, mode) in [
            ("foreign owner", euid + 1, directory | 0o700),
            ("wrong type", euid, regular | 0o700),
            ("wrong owner bits", euid, directory | 0o600),
            ("group permissions", euid, directory | 0o710),
            ("other permissions", euid, directory | 0o701),
            ("special bits", euid, directory | 0o4700),
        ] {
            assert!(!owned_directory_is_private(uid, mode, euid), "{label}");
        }
    }

    #[test]
    fn socket_predicate_requires_type_owner_and_one_of_two_private_modes() {
        let euid = 1000;
        let socket = FileType::Socket.as_raw_mode();
        let regular = FileType::RegularFile.as_raw_mode();
        assert!(owned_socket_is_private(euid, socket | 0o600, euid));
        assert!(owned_socket_is_private(euid, socket | 0o700, euid));

        for (label, uid, mode) in [
            ("foreign owner", euid + 1, socket | 0o600),
            ("wrong type", euid, regular | 0o600),
            ("wrong owner bits", euid, socket | 0o400),
            ("group permissions", euid, socket | 0o610),
            ("other permissions", euid, socket | 0o601),
            ("special bits", euid, socket | 0o4600),
        ] {
            assert!(!owned_socket_is_private(uid, mode, euid), "{label}");
        }
    }

    #[test]
    fn explicit_and_derived_endpoints_accept_only_the_two_approved_socket_modes() {
        for mode in [0o600, 0o700] {
            let fixture = PrivateEndpointFixture::new();
            set_mode(&fixture.socket_path, mode);

            fixture.explicit_endpoint().validate().unwrap();
            fixture.derived_endpoint().validate().unwrap();
        }
    }

    #[test]
    fn validation_reads_fresh_metadata_on_every_call() {
        let fixture = PrivateEndpointFixture::new();
        let endpoint = fixture.explicit_endpoint();
        endpoint.validate().unwrap();

        set_mode(&fixture.socket_path, 0o610);
        let error = endpoint.validate().unwrap_err();
        assert_validation_failure(&error);
    }

    #[test]
    fn every_selected_derived_component_rejects_symlinks() {
        for selected in [
            SelectedEntry::RuntimeDirectory,
            SelectedEntry::AppDirectory,
            SelectedEntry::BridgeDirectory,
            SelectedEntry::Socket,
        ] {
            let mut fixture = PrivateEndpointFixture::new();
            let endpoint = fixture.derived_endpoint();
            if matches!(selected, SelectedEntry::Socket) {
                fixture.close_listener();
            }
            replace_with_symlink(fixture.selected_path(selected));

            let error = endpoint.validate().unwrap_err();
            assert_validation_failure(&error);
        }
    }

    #[test]
    fn every_selected_derived_component_rejects_the_wrong_type() {
        for selected in [
            SelectedEntry::RuntimeDirectory,
            SelectedEntry::AppDirectory,
            SelectedEntry::BridgeDirectory,
            SelectedEntry::Socket,
        ] {
            let mut fixture = PrivateEndpointFixture::new();
            let endpoint = fixture.derived_endpoint();
            fixture.close_listener();
            replace_with_regular_file(fixture.selected_path(selected));

            let error = endpoint.validate().unwrap_err();
            assert_validation_failure(&error);
        }
    }

    #[test]
    fn every_derived_directory_requires_private_mode() {
        for selected in [
            SelectedEntry::RuntimeDirectory,
            SelectedEntry::AppDirectory,
            SelectedEntry::BridgeDirectory,
        ] {
            let fixture = PrivateEndpointFixture::new();
            let endpoint = fixture.derived_endpoint();
            set_mode(fixture.selected_path(selected), 0o750);

            let error = endpoint.validate().unwrap_err();
            assert_validation_failure(&error);
        }
    }

    #[test]
    fn real_directories_reject_each_disallowed_mode_class() {
        for (label, mode) in [
            ("wrong owner bits", 0o600),
            ("group permissions", 0o710),
            ("other permissions", 0o701),
            ("special bits", 0o4700),
        ] {
            let fixture = PrivateEndpointFixture::new();
            let endpoint = fixture.derived_endpoint();
            set_mode(&fixture.bridge_dir, mode);

            let error = endpoint.validate().unwrap_err();
            assert_validation_failure(&error);
            assert_eq!(error.stage, "socket_validation", "{label}");
        }
    }

    #[test]
    fn real_sockets_reject_each_disallowed_mode_class() {
        for (label, mode) in [
            ("wrong owner bits", 0o400),
            ("group permissions", 0o610),
            ("other permissions", 0o601),
            ("special bits", 0o4600),
        ] {
            let fixture = PrivateEndpointFixture::new();
            let endpoint = fixture.derived_endpoint();
            set_mode(&fixture.socket_path, mode);

            let error = endpoint.validate().unwrap_err();
            assert_validation_failure(&error);
            assert_eq!(error.stage, "socket_validation", "{label}");
        }
    }

    #[test]
    fn missing_selected_parent_or_socket_is_target_unavailable() {
        let root = crate::test_support::private_tempdir();
        let missing_parent_endpoint =
            DesktopEndpoint::explicit(root.path().join("missing/app-server.sock"));
        let parent_error = missing_parent_endpoint.validate().unwrap_err();
        assert_target_unavailable(&parent_error);

        let mut fixture = PrivateEndpointFixture::new();
        let missing_socket_endpoint = fixture.derived_endpoint();
        fixture.close_listener();
        fs::remove_file(&fixture.socket_path).unwrap();
        let socket_error = missing_socket_endpoint.validate().unwrap_err();
        assert_target_unavailable(&socket_error);
    }

    #[test]
    fn explicit_constructor_fails_closed_on_a_malformed_path() {
        let error = DesktopEndpoint::explicit(PathBuf::from("relative.sock"))
            .validate()
            .unwrap_err();
        assert_validation_failure(&error);
    }

    #[test]
    fn canonical_parent_replacement_is_rejected() {
        let root = crate::test_support::private_tempdir();
        let selected_ancestor = root.path().join("selected");
        let parent = selected_ancestor.join("bridge");
        create_private_directory(&selected_ancestor);
        create_private_directory(&parent);
        let socket_path = parent.join("app-server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        set_mode(&socket_path, 0o600);
        let endpoint = DesktopEndpoint::explicit(socket_path);

        let relocated = root.path().join("relocated");
        fs::rename(&selected_ancestor, &relocated).unwrap();
        symlink(&relocated, &selected_ancestor).unwrap();

        let error = endpoint.validate().unwrap_err();
        assert_validation_failure(&error);
        drop(listener);
    }
}
