use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::Path,
};

use tempfile::TempDir;

use super::*;

fn fixture() -> (TempDir, ResolvedUserPaths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let euid = rustix::process::geteuid().as_raw();
    (root, ResolvedUserPaths::for_test(euid, home, runtime))
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn write_executable(path: &Path) {
    fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn lifecycle_target_identity_is_not_runtime_selectable() {
    let (_root, paths) = fixture();
    let production = LifecycleTarget::production(paths.clone());
    assert_eq!(production.unit_name, "codex-session-control.service");
    assert_eq!(production.paths.unit, paths.unit);

    let isolated = LifecycleTarget::suffixed(paths, "A1b2");
    assert_eq!(
        isolated.unit_name,
        "codex-session-control-test-A1b2.service"
    );
    assert_eq!(
        isolated.paths.unit.file_name().unwrap(),
        "codex-session-control-test-A1b2.service"
    );
}

#[test]
#[should_panic]
fn test_unit_suffix_rejects_non_alphanumeric_input() {
    let (_root, paths) = fixture();
    let _ = LifecycleTarget::suffixed(paths, "../other");
}

#[test]
fn exact_durable_paths_derive_only_from_home_and_runtime() {
    let (_root, paths) = fixture();
    assert_eq!(
        paths.binary,
        paths.home.join(".local/bin/codex-session-control")
    );
    assert_eq!(
        paths.config,
        paths.home.join(".config/codex-session-control/config.toml")
    );
    assert_eq!(
        paths.unit,
        paths
            .home
            .join(".config/systemd/user/codex-session-control.service")
    );
    assert_eq!(
        paths.data_root,
        paths.home.join(".local/share/codex-session-control")
    );
    assert_eq!(paths.codex_home, paths.home.join(".codex"));
    assert_eq!(paths.marketplace, paths.data_root.join("marketplace"));
    assert_eq!(
        paths.manifest,
        paths.data_root.join("installed-release.json")
    );
    assert_eq!(
        paths.runtime_dir,
        paths.runtime.join("codex-session-control")
    );
    assert_eq!(paths.socket, paths.runtime_dir.join("app-server.sock"));
}

#[test]
fn invocation_identity_mismatch_is_rejected_before_paths_are_mutated() {
    let (_root, paths) = fixture();
    let before = fs::read_dir(&paths.home).unwrap().count();
    for (home, runtime) in [
        (
            Some(OsStr::new("/wrong-home")),
            Some(paths.runtime.as_os_str()),
        ),
        (
            Some(paths.home.as_os_str()),
            Some(OsStr::new("/wrong-runtime")),
        ),
        (None, Some(paths.runtime.as_os_str())),
        (Some(paths.home.as_os_str()), None),
    ] {
        assert!(paths.validate_invocation_identity(home, runtime).is_err());
    }
    assert_eq!(fs::read_dir(&paths.home).unwrap().count(), before);
}

#[test]
fn existing_final_files_require_owner_safe_mode_and_exact_type() {
    let (root, paths) = fixture();
    let regular = root.path().join("regular");
    fs::write(&regular, b"data").unwrap();
    fs::set_permissions(&regular, fs::Permissions::from_mode(0o600)).unwrap();
    validate_existing(&regular, FileKind::RegularFile, paths.euid).unwrap();

    assert!(validate_existing(&regular, FileKind::RegularFile, paths.euid + 1).is_err());
    fs::set_permissions(&regular, fs::Permissions::from_mode(0o620)).unwrap();
    assert!(validate_existing(&regular, FileKind::RegularFile, paths.euid).is_err());

    let directory = root.path().join("directory");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(validate_existing(&directory, FileKind::RegularFile, paths.euid).is_err());
    validate_existing(&directory, FileKind::Directory, paths.euid).unwrap();

    let socket = root.path().join("socket");
    let _listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    validate_existing(&socket, FileKind::UnixSocket, paths.euid).unwrap();
    assert!(validate_existing(&socket, FileKind::RegularFile, paths.euid).is_err());

    let link = root.path().join("link");
    symlink(&regular, &link).unwrap();
    assert!(validate_existing(&link, FileKind::RegularFile, paths.euid).is_err());
}

#[test]
fn control_socket_requires_owner_read_write_and_no_group_or_other_bits() {
    let (root, paths) = fixture();
    let socket = root.path().join("control.sock");

    for mode in 0..=0o777 {
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(mode)).unwrap();
        let result = validate_control_socket(&socket, paths.euid);
        if matches!(mode, 0o600 | 0o700) {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().to_string(),
                format!("invalid socket: {SOCKET_SECURITY_REQUIREMENT}"),
                "mode {mode:04o}"
            );
        }
        drop(listener);
        fs::remove_file(&socket).unwrap();
    }

    fs::write(&socket, b"not a socket").unwrap();
    assert_eq!(
        validate_control_socket(&socket, paths.euid)
            .unwrap_err()
            .to_string(),
        format!("invalid socket: {SOCKET_SECURITY_REQUIREMENT}")
    );
}

#[tokio::test(start_paused = true)]
async fn control_socket_readiness_rejects_present_unsafe_path_immediately_without_mutation() {
    let (root, paths) = fixture();
    let socket = root.path().join("control.sock");
    fs::write(&socket, b"unsafe sentinel").unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::metadata(&socket).unwrap();
    let started = tokio::time::Instant::now();

    let error = wait_for_control_socket(&socket, paths.euid)
        .await
        .unwrap_err();

    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(
        error.to_string(),
        format!("invalid socket: {SOCKET_SECURITY_REQUIREMENT}")
    );
    assert_eq!(fs::read(&socket).unwrap(), b"unsafe sentinel");
    let after = fs::metadata(&socket).unwrap();
    assert_eq!(after.permissions().mode(), before.permissions().mode());
    assert_eq!(after.len(), before.len());
}

#[tokio::test(start_paused = true)]
async fn control_socket_readiness_times_out_at_fifteen_seconds_without_mutation() {
    let (root, paths) = fixture();
    let socket = root.path().join("control.sock");
    let entries_before = fs::read_dir(root.path()).unwrap().count();
    let started = tokio::time::Instant::now();
    let waited_socket = socket.clone();
    let task =
        tokio::spawn(async move { wait_for_control_socket(&waited_socket, paths.euid).await });
    tokio::task::yield_now().await;

    tokio::time::advance(CONTROL_SOCKET_READINESS_TIMEOUT - Duration::from_millis(1)).await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;

    assert_eq!(
        task.await.unwrap().unwrap_err().to_string(),
        "invalid socket: missing or inaccessible"
    );
    assert_eq!(started.elapsed(), CONTROL_SOCKET_READINESS_TIMEOUT);
    assert!(!socket.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), entries_before);
}

#[test]
fn codex_resolution_preserves_direct_and_external_symlink_invocation_paths() {
    let (root, _paths) = fixture();
    let cwd = root.path().join("cwd");
    let direct_bin = cwd.join("direct-bin");
    let linked_bin = cwd.join("linked-bin");
    fs::create_dir(&cwd).unwrap();
    fs::create_dir(&direct_bin).unwrap();
    fs::create_dir(&linked_bin).unwrap();
    write_executable(&direct_bin.join("codex"));
    write_executable(&cwd.join("real-codex"));
    symlink(cwd.join("real-codex"), linked_bin.join("codex")).unwrap();

    assert_eq!(
        resolve_codex_executable(OsStr::new("direct-bin"), &cwd).unwrap(),
        direct_bin.join("codex")
    );
    assert_eq!(
        resolve_codex_executable(OsStr::new("linked-bin"), &cwd).unwrap(),
        linked_bin.join("codex")
    );
}

#[test]
fn codex_resolution_skips_every_invalid_candidate() {
    let (root, _paths) = fixture();
    let cwd = root.path().join("cwd");
    fs::create_dir(&cwd).unwrap();
    for name in [
        "broken",
        "nonexec",
        "directory",
        "socket",
        "missing",
        "valid",
    ] {
        fs::create_dir(cwd.join(name)).unwrap();
    }
    symlink(cwd.join("does-not-exist"), cwd.join("broken/codex")).unwrap();
    fs::write(cwd.join("nonexec/codex"), b"not executable").unwrap();
    fs::set_permissions(cwd.join("nonexec/codex"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::create_dir(cwd.join("directory/codex")).unwrap();
    let _listener = UnixListener::bind(cwd.join("socket/codex")).unwrap();
    write_executable(&cwd.join("valid/codex"));

    let path = std::env::join_paths([
        Path::new("broken"),
        Path::new("nonexec"),
        Path::new("directory"),
        Path::new("socket"),
        Path::new("missing"),
        Path::new("valid"),
    ])
    .unwrap();
    assert_eq!(
        resolve_codex_executable(&path, &cwd).unwrap(),
        cwd.join("valid/codex")
    );

    assert!(
        resolve_codex_executable(
            &std::env::join_paths([
                Path::new("broken"),
                Path::new("nonexec"),
                Path::new("directory"),
                Path::new("socket"),
                Path::new("missing"),
            ])
            .unwrap(),
            &cwd,
        )
        .is_err()
    );
}

#[test]
fn product_directory_creation_preserves_safe_shared_ancestors() {
    let (_root, paths) = fixture();
    let shared = paths.home.join(".local");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();

    create_product_dir(&paths.data_root, paths.euid).unwrap();

    assert_eq!(mode(&shared), 0o755);
    assert_eq!(mode(&paths.home.join(".local/share")), 0o700);
    assert_eq!(mode(&paths.data_root), 0o700);
}

#[test]
fn product_directory_creation_rejects_unsafe_ancestors_without_chmod() {
    let (_root, paths) = fixture();
    let shared = paths.home.join(".config");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o720)).unwrap();

    assert!(create_product_dir(paths.config.parent().unwrap(), paths.euid).is_err());
    assert_eq!(mode(&shared), 0o720);
    assert!(!paths.config.parent().unwrap().exists());
}

#[test]
fn existing_product_directory_is_enforced_to_private_mode() {
    let (_root, paths) = fixture();
    fs::create_dir_all(&paths.data_root).unwrap();
    for directory in [
        paths.home.join(".local"),
        paths.home.join(".local/share"),
        paths.data_root.clone(),
    ] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
    }

    create_product_dir(&paths.data_root, paths.euid).unwrap();

    assert_eq!(mode(&paths.home.join(".local")), 0o755);
    assert_eq!(mode(&paths.home.join(".local/share")), 0o755);
    assert_eq!(mode(&paths.data_root), 0o700);
}

#[test]
fn atomic_write_replaces_without_backup_and_applies_exact_modes() {
    let (root, _paths) = fixture();
    let parent = root.path().join("atomic");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();

    for (name, bytes, expected_mode) in [
        ("config.toml", b"config".as_slice(), 0o600),
        ("projection.json", b"projection".as_slice(), 0o644),
        ("codex-session-control", b"binary".as_slice(), 0o755),
    ] {
        let destination = parent.join(name);
        fs::write(&destination, b"old").unwrap();
        atomic_write(&destination, bytes, expected_mode).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(mode(&destination), expected_mode);
    }

    let entries = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .all(|entry| !entry.to_string_lossy().contains("backup"))
    );
}

#[test]
fn atomic_write_uses_unique_stages_and_cleans_failed_stage_files() {
    let (root, _paths) = fixture();
    let parent = root.path().join("atomic");
    fs::create_dir(&parent).unwrap();
    let destination = parent.join("state");

    std::thread::scope(|scope| {
        for byte in [b'a', b'b'] {
            let destination = destination.clone();
            scope.spawn(move || {
                atomic_write(&destination, &[byte], 0o600).unwrap();
            });
        }
    });
    assert!(matches!(
        fs::read(&destination).unwrap().as_slice(),
        b"a" | b"b"
    ));
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);

    fs::remove_file(&destination).unwrap();
    fs::create_dir(&destination).unwrap();
    assert!(atomic_write(&destination, b"cannot replace directory", 0o600).is_err());
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
    assert!(destination.is_dir());
}
