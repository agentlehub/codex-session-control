use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use tempfile::TempDir;

use super::*;

fn fixture() -> (TempDir, ResolvedUserPaths) {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    fs::create_dir(home.join(".config")).unwrap();
    fs::create_dir(home.join(".config/codex-session-control")).unwrap();
    fs::create_dir(home.join(".local")).unwrap();
    fs::create_dir(home.join(".local/share")).unwrap();
    fs::create_dir(home.join(".local/share/codex-session-control")).unwrap();
    fs::create_dir(home.join(".codex")).unwrap();
    fs::create_dir(runtime.join("codex-session-control")).unwrap();
    for directory in [
        home.as_path(),
        home.join(".config").as_path(),
        home.join(".config/codex-session-control").as_path(),
    ] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let euid = rustix::process::geteuid().as_raw();
    (root, ResolvedUserPaths::for_test(euid, home, runtime))
}

fn valid_toml(paths: &ResolvedUserPaths) -> String {
    format!(
        "schema_version = 2\ncodex_executable = \"/usr/bin/codex\"\n\
                 codex_home = \"{}\"\nsocket_path = \"{}\"\n",
        paths.codex_home.display(),
        paths.socket.display()
    )
}

fn write_config(paths: &ResolvedUserPaths, text: &str) {
    fs::write(&paths.config, text).unwrap();
    fs::set_permissions(&paths.config, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn canonical_paths_and_identity_are_fixed() {
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

    paths
        .validate_invocation_identity(
            Some(paths.home.as_os_str()),
            Some(paths.runtime.as_os_str()),
        )
        .unwrap();
    assert!(
        paths
            .validate_invocation_identity(
                Some(OsStr::new("/wrong-home")),
                Some(paths.runtime.as_os_str())
            )
            .is_err()
    );
    assert!(
        paths
            .validate_invocation_identity(
                Some(paths.home.as_os_str()),
                Some(OsStr::new("/wrong-runtime"))
            )
            .is_err()
    );
}

#[test]
fn effective_uid_passwd_home_and_run_user_runtime_are_authoritative() {
    let (_root, expected) = fixture();
    let paths = ResolvedUserPaths::from_injected_effective_user(
        expected.euid,
        expected.home.clone(),
        expected.runtime.clone(),
        Some(expected.home.as_os_str()),
        Some(expected.runtime.as_os_str()),
        None,
        NativeProductResidue::Absent,
    )
    .unwrap();
    assert_eq!(paths.euid, expected.euid);
    assert_eq!(paths.home, expected.home);
    assert_eq!(paths.runtime, expected.runtime);
}

#[test]
fn missing_and_unsafe_config_files_are_rejected_without_repair() {
    let (_root, paths) = fixture();
    assert!(load_config_from_paths(&paths).is_err());
    assert!(!paths.config.exists());

    fs::create_dir(&paths.config).unwrap();
    assert!(load_config_from_paths(&paths).is_err());
    fs::remove_dir(&paths.config).unwrap();

    let target = paths.home.join("elsewhere.toml");
    fs::write(&target, valid_toml(&paths)).unwrap();
    symlink(&target, &paths.config).unwrap();
    assert!(load_config_from_paths(&paths).is_err());
    fs::remove_file(&paths.config).unwrap();

    write_config(&paths, &valid_toml(&paths));
    fs::set_permissions(&paths.config, fs::Permissions::from_mode(0o620)).unwrap();
    assert!(load_config_from_paths(&paths).is_err());
    assert_eq!(
        fs::metadata(&paths.config).unwrap().permissions().mode() & 0o777,
        0o620
    );

    fs::set_permissions(&paths.config, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(validate_config_file(&paths.config, paths.euid + 1).is_err());
    let mut wrong_owner = paths.clone();
    wrong_owner.euid += 1;
    assert!(load_config_from_paths(&wrong_owner).is_err());
}

#[test]
fn unsafe_ancestors_are_rejected_without_chmod() {
    let (_root, paths) = fixture();
    write_config(&paths, &valid_toml(&paths));
    let config_parent = paths.config.parent().unwrap();
    fs::set_permissions(config_parent, fs::Permissions::from_mode(0o720)).unwrap();
    assert!(load_config_from_paths(&paths).is_err());
    assert_eq!(
        fs::metadata(config_parent).unwrap().permissions().mode() & 0o777,
        0o720
    );
}

#[test]
fn mode_0600_strict_toml_and_canonical_identity_are_accepted() {
    let (_root, paths) = fixture();
    write_config(&paths, &valid_toml(&paths));
    let config = load_config_from_paths(&paths).unwrap();
    assert_eq!(config.schema_version, 2);
    assert_eq!(config.codex_executable, Path::new("/usr/bin/codex"));
    assert_eq!(config.codex_home, paths.codex_home);
    assert_eq!(config.socket_path, paths.socket);
}

#[test]
fn unknown_fields_schema_versions_and_identity_mismatches_are_rejected() {
    let (_root, paths) = fixture();
    for text in [
        format!("{}unknown = true\n", valid_toml(&paths)),
        valid_toml(&paths).replacen("schema_version = 2", "schema_version = 3", 1),
        valid_toml(&paths).replacen(
            &paths.codex_home.display().to_string(),
            "/tmp/other-codex-home",
            1,
        ),
        valid_toml(&paths).replacen(&paths.socket.display().to_string(), "/tmp/other.sock", 1),
    ] {
        write_config(&paths, &text);
        assert!(load_config_from_paths(&paths).is_err(), "{text}");
    }
}
