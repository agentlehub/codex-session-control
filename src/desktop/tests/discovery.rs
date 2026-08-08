use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use crate::error::ControllerError;

use super::{
    super::{
        DESCRIPTOR_FILE_NAME, DESKTOP_ENTRY_NAME, DesktopAvailability,
        discovery::{
            application_search_roots, desktop_entry_path, effective_config_root,
            inspect_build_info, validate_launcher,
        },
        probe_desktop_capability, probe_persisted_desktop_capability, publish_descriptor,
        render_descriptor,
    },
    environment, write_environment_launcher, write_executable_fixture, write_file, write_launcher,
};

#[test]
fn xdg_resolution_shadows_lower_entries_including_hidden_entries() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let data_home = root.path().join("data-home");
    let data_dir = root.path().join("data-dir");
    fs::create_dir_all(data_home.join("applications")).unwrap();
    fs::create_dir_all(data_dir.join("applications")).unwrap();
    write_file(
        &data_dir.join("applications").join(DESKTOP_ENTRY_NAME),
        b"[Desktop Entry]\nType=Application\nExec=/bin/true\n",
        0o600,
    );
    let mut env = environment(&home);
    env.insert("XDG_DATA_HOME".into(), data_home.as_os_str().to_owned());
    env.insert("XDG_DATA_DIRS".into(), data_dir.as_os_str().to_owned());
    assert_eq!(
        desktop_entry_path(&env).unwrap(),
        Some(data_dir.join("applications").join(DESKTOP_ENTRY_NAME))
    );

    write_file(
        &data_home.join("applications").join(DESKTOP_ENTRY_NAME),
        b"[Desktop Entry]\nHidden=true\n",
        0o600,
    );
    assert_eq!(
        desktop_entry_path(&env).unwrap(),
        Some(data_home.join("applications").join(DESKTOP_ENTRY_NAME))
    );
    assert!(matches!(
        super::super::entry::parse_desktop_entry(
            &fs::read(data_home.join("applications").join(DESKTOP_ENTRY_NAME)).unwrap()
        ),
        Err(super::super::DiscoveryFailure::Unavailable(_))
    ));

    fs::remove_file(data_home.join("applications").join(DESKTOP_ENTRY_NAME)).unwrap();
    env.insert("XDG_DATA_HOME".into(), OsString::from("relative-data-home"));
    env.insert(
        "XDG_DATA_DIRS".into(),
        std::env::join_paths([PathBuf::from("relative-data-dir"), data_dir.clone()]).unwrap(),
    );
    assert_eq!(
        desktop_entry_path(&env).unwrap(),
        Some(data_dir.join("applications").join(DESKTOP_ENTRY_NAME))
    );
}

#[tokio::test]
async fn discovery_uses_direct_argv_and_requires_complete_capable_build_info() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let launcher = root.path().join("codex-desktop");
    let log = root.path().join("argv.log");
    write_launcher(
        &launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    let mut env = environment(&home);
    env.insert("XDG_CONFIG_HOME".into(), config.as_os_str().to_owned());
    let target = probe_desktop_capability(Some(&launcher), &env)
        .await
        .unwrap();
    let DesktopAvailability::Verified(target) = target else {
        panic!("valid launcher must verify");
    };
    assert_eq!(fs::read_to_string(&log).unwrap(), "--print-build-info\n");
    assert_eq!(
        target.identity.descriptor_path,
        config.join("codex-desktop").join(DESCRIPTOR_FILE_NAME)
    );
    assert_eq!(target.command.environment, BTreeMap::new());
    assert!(matches!(
        probe_persisted_desktop_capability(&target.identity, &env)
            .await
            .unwrap(),
        DesktopAvailability::Verified(_)
    ));

    write_launcher(
        &launcher,
        &log,
        "../unsafe",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    assert!(matches!(
        probe_desktop_capability(Some(&launcher), &env)
            .await
            .unwrap(),
        DesktopAvailability::Unavailable { .. }
    ));
    write_launcher(&launcher, &log, "codex-desktop", "[]");
    assert!(matches!(
        probe_desktop_capability(Some(&launcher), &env)
            .await
            .unwrap(),
        DesktopAvailability::Unavailable { .. }
    ));
    assert!(matches!(
        probe_desktop_capability(Some(Path::new("relative-launcher")), &env)
            .await
            .unwrap(),
        DesktopAvailability::Unavailable { .. }
    ));

    let data_home = root.path().join("data-home");
    let entry_config = root.path().join("entry-config");
    fs::create_dir_all(data_home.join("applications")).unwrap();
    write_launcher(
        &launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    write_file(
        &data_home.join("applications").join(DESKTOP_ENTRY_NAME),
        format!(
            "[Desktop Entry]\nType=Application\nExec=env XDG_CONFIG_HOME={} {} --fixed \"two words\" %% %U\n",
            entry_config.display(),
            launcher.display()
        ),
        0o600,
    );
    env.insert("XDG_DATA_HOME".into(), data_home.as_os_str().to_owned());
    let target = probe_desktop_capability(None, &env).await.unwrap();
    let DesktopAvailability::Verified(target) = target else {
        panic!("Desktop entry must verify");
    };
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "--print-build-info\n--fixed\ntwo words\n%\n"
    );
    assert_eq!(target.command.effective_config_root, entry_config);
    assert_eq!(
        target.command.environment,
        BTreeMap::from([(
            OsString::from("XDG_CONFIG_HOME"),
            target.command.effective_config_root.as_os_str().to_owned()
        )])
    );
    let persisted = probe_persisted_desktop_capability(&target.identity, &env)
        .await
        .unwrap();
    let DesktopAvailability::Verified(persisted) = persisted else {
        panic!("persisted Desktop identity must retain the entry config root");
    };
    assert_eq!(persisted.command.effective_config_root, entry_config);
}

#[tokio::test]
async fn build_info_child_receives_only_the_supplied_environment() {
    let root = crate::test_support::private_tempdir();
    let launcher = root.path().join("codex-desktop");
    let observed = root.path().join("observed");
    write_executable_fixture(
        &launcher,
        format!(
            "#!/bin/sh\nprintf '%s' \"${{CSC_AMBIENT_ONLY-unset}}\" > '{}'\nprintf '%s\\n' '{{\"appIdentity\":{{\"id\":\"codex-desktop\"}},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}}'\n",
            observed.display()
        ),
    );
    let environment = BTreeMap::from([(
        OsString::from("HOME"),
        root.path().join("home").into_os_string(),
    )]);

    assert_eq!(
        inspect_build_info(&launcher, &[], &environment)
            .await
            .unwrap(),
        "codex-desktop"
    );
    assert_eq!(fs::read_to_string(observed).unwrap(), "unset");
}

#[test]
fn xdg_empty_values_default_and_relative_roots_are_ignored_without_other_filename_scans() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let data_home = home.join(".local/share");
    fs::create_dir_all(data_home.join("applications")).unwrap();
    write_file(
        &data_home.join("applications").join("some-other.desktop"),
        b"[Desktop Entry]\nType=Application\nExec=/bin/true\n",
        0o600,
    );
    let mut env = environment(&home);
    assert_eq!(
        application_search_roots(&env).unwrap(),
        vec![
            home.join(".local/share"),
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]
    );
    env.insert("XDG_DATA_HOME".into(), OsString::new());
    env.insert("XDG_DATA_DIRS".into(), OsString::new());
    assert_eq!(
        application_search_roots(&env).unwrap(),
        vec![
            data_home.clone(),
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]
    );
    let isolated_data_dir = root.path().join("isolated-data-dir");
    fs::create_dir_all(isolated_data_dir.join("applications")).unwrap();
    env.insert(
        "XDG_DATA_DIRS".into(),
        isolated_data_dir.as_os_str().to_owned(),
    );
    assert_eq!(desktop_entry_path(&env).unwrap(), None);
    let ordered_first = root.path().join("ordered-first");
    let ordered_second = root.path().join("ordered-second");
    env.insert(
        "XDG_DATA_DIRS".into(),
        std::env::join_paths([
            PathBuf::from("relative-root"),
            ordered_first.clone(),
            ordered_second.clone(),
        ])
        .unwrap(),
    );
    assert_eq!(
        application_search_roots(&env).unwrap(),
        vec![data_home.clone(), ordered_first, ordered_second]
    );
    assert_eq!(effective_config_root(&env).unwrap(), home.join(".config"));
    env.insert("XDG_CONFIG_HOME".into(), OsString::from("relative-config"));
    assert!(effective_config_root(&env).is_err());
    env.insert("XDG_CONFIG_HOME".into(), OsString::new());
    assert_eq!(effective_config_root(&env).unwrap(), home.join(".config"));
}

#[tokio::test]
async fn capability_probe_leaves_descriptor_parent_for_publication() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let launcher = root.path().join("codex-desktop");
    let log = root.path().join("argv.log");
    write_launcher(
        &launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    let mut env = environment(&home);
    env.insert("XDG_CONFIG_HOME".into(), config.as_os_str().to_owned());

    let DesktopAvailability::Verified(target) = probe_desktop_capability(Some(&launcher), &env)
        .await
        .unwrap()
    else {
        panic!("capable launcher must probe successfully");
    };
    assert!(
        !config.exists(),
        "capability probing must not create the descriptor config root"
    );
    assert!(
        !config.join("codex-desktop").exists(),
        "capability probing must leave descriptor-parent creation to publication"
    );
    let descriptor = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    assert!(publish_descriptor(&target.identity, &descriptor).unwrap());
    assert!(config.is_dir());
    assert_eq!(
        fs::read(&target.identity.descriptor_path).unwrap(),
        descriptor
    );
}

#[tokio::test]
async fn capability_probe_rejects_lexically_unnormalized_config_root() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let launcher = root.path().join("codex-desktop");
    let log = root.path().join("argv.log");
    write_launcher(
        &launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    let mut env = environment(&home);
    env.insert(
        "XDG_CONFIG_HOME".into(),
        root.path()
            .join("safe")
            .join("..")
            .join("other")
            .into_os_string(),
    );

    let error = probe_desktop_capability(Some(&launcher), &env)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ControllerError::InvalidData {
            field: "desktop_attachment.descriptor_path",
            reason: "path must be lexically normalized",
        }
    ));
}

#[tokio::test]
async fn override_and_launcher_validation_require_a_direct_owner_executable_not_a_desktop_entry() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let launcher = root.path().join("launcher");
    let spaced_launcher = root.path().join("launcher with spaces");
    let desktop_file = root.path().join("codex-desktop.desktop");
    let log = root.path().join("argv.log");
    let mut env = environment(&home);
    env.insert("XDG_CONFIG_HOME".into(), config.as_os_str().to_owned());
    write_launcher(
        &launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    assert!(matches!(
        probe_desktop_capability(Some(&launcher), &env)
            .await
            .unwrap(),
        DesktopAvailability::Verified(_)
    ));
    write_launcher(
        &spaced_launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    assert!(matches!(
        probe_desktop_capability(Some(&spaced_launcher), &env)
            .await
            .unwrap(),
        DesktopAvailability::Verified(_)
    ));
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o001)).unwrap();
    assert!(validate_launcher(&launcher).is_err());
    write_launcher(
        &desktop_file,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    assert!(matches!(
        probe_desktop_capability(Some(&desktop_file), &env)
            .await
            .unwrap(),
        DesktopAvailability::Unavailable { .. }
    ));
    assert!(validate_launcher(&root.path().join("missing")).is_err());
    assert!(validate_launcher(root.path()).is_err());
}

#[test]
fn launcher_validation_accepts_a_safe_root_owned_executable() {
    let launcher = Path::new("/bin/sh");
    let metadata = fs::metadata(launcher).unwrap();
    assert_eq!(metadata.uid(), 0, "test fixture must be root-owned");
    assert_ne!(
        metadata.mode() & 0o001,
        0,
        "test fixture must be executable by ordinary users"
    );

    assert_eq!(
        validate_launcher(launcher).unwrap(),
        fs::canonicalize(launcher).unwrap()
    );
}

#[test]
fn launcher_validation_rejects_group_or_world_writable_executables() {
    let root = crate::test_support::private_tempdir();
    let launcher = root.path().join("launcher");
    write_executable_fixture(&launcher, "#!/bin/sh\nexit 0\n");

    for mode in [0o720, 0o702] {
        fs::set_permissions(&launcher, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            validate_launcher(&launcher).is_err(),
            "launcher mode {mode:o} must be rejected"
        );
    }
}

#[tokio::test]
async fn launcher_rejects_cross_uid_replaceable_ancestors_before_spawning_a_replacement() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let unsafe_parent = root.path().join("non-sticky-writable");
    let unsafe_launcher = unsafe_parent.join("launcher");
    let replacement_marker = root.path().join("replacement-executed");
    fs::create_dir(&unsafe_parent).unwrap();
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777)).unwrap();
    write_executable_fixture(
        &unsafe_launcher,
        format!(
            "#!/bin/sh\nprintf replacement > '{}'\nprintf '%s\\n' '{{\"appIdentity\":{{\"id\":\"codex-desktop\"}},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}}'\n",
            replacement_marker.display(),
        ),
    );
    let mut env = environment(&home);
    env.insert("XDG_CONFIG_HOME".into(), config.as_os_str().to_owned());
    let availability = probe_desktop_capability(Some(&unsafe_launcher), &env)
        .await
        .unwrap();
    assert!(
        !replacement_marker.exists(),
        "a launcher below a cross-UID replaceable ancestor must not execute"
    );
    assert!(matches!(
        availability,
        DesktopAvailability::Unavailable { .. }
    ));

    let sticky_parent = root.path().join("sticky-writable");
    let sticky_launcher = sticky_parent.join("launcher with spaces");
    fs::create_dir(&sticky_parent).unwrap();
    fs::set_permissions(&sticky_parent, fs::Permissions::from_mode(0o1777)).unwrap();
    write_launcher(
        &sticky_launcher,
        &root.path().join("sticky.log"),
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    assert!(validate_launcher(&sticky_launcher).is_ok());
}

#[tokio::test]
async fn discovery_classifies_absent_hidden_malformed_unreadable_and_unexecutable_entries_without_lifecycle_work()
 {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let data_home = root.path().join("data-home");
    let launcher = root.path().join("launcher");
    let log = root.path().join("argv.log");
    let entry = data_home.join("applications").join(DESKTOP_ENTRY_NAME);
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    let mut env = environment(&home);
    env.insert("XDG_DATA_HOME".into(), data_home.as_os_str().to_owned());
    for bytes in [
        b"[Desktop Entry]\nHidden=true\n".as_slice(),
        b"[Desktop Entry]\nType=Application\nExec=\"unterminated\n".as_slice(),
    ] {
        write_file(&entry, bytes, 0o600);
        assert!(matches!(
            probe_desktop_capability(None, &env).await.unwrap(),
            DesktopAvailability::Unavailable { .. }
        ));
    }
    write_file(
        &entry,
        b"[Desktop Entry]\nType=Application\nExec=/bin/does-not-exist\n",
        0o600,
    );
    assert!(matches!(
        probe_desktop_capability(None, &env).await.unwrap(),
        DesktopAvailability::Unavailable { .. }
    ));
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(matches!(
        probe_desktop_capability(None, &env).await.unwrap(),
        DesktopAvailability::Unavailable { .. }
    ));
    write_launcher(
        &launcher,
        &log,
        "codex-desktop",
        r#"["external-app-server-attachment-descriptor-v1"]"#,
    );
    let unsafe_config = root.path().join("unsafe-config");
    fs::create_dir(&unsafe_config).unwrap();
    fs::set_permissions(&unsafe_config, fs::Permissions::from_mode(0o777)).unwrap();
    env.insert(
        "XDG_CONFIG_HOME".into(),
        unsafe_config.as_os_str().to_owned(),
    );
    assert!(matches!(
        probe_desktop_capability(Some(&launcher), &env)
            .await
            .unwrap(),
        DesktopAvailability::Verified(_)
    ));
    assert!(!unsafe_config.join("codex-desktop").exists());
}

#[tokio::test]
async fn desktop_entry_environment_is_the_exact_child_environment_used_for_build_info() {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let child_home = root.path().join("child-home");
    let child_config = root.path().join("child-config");
    let data_home = root.path().join("data-home");
    let launcher = root.path().join("launcher");
    let argv_log = root.path().join("argv.log");
    let environment_log = root.path().join("environment.log");
    fs::create_dir_all(data_home.join("applications")).unwrap();
    write_environment_launcher(&launcher, &argv_log, &environment_log);
    write_file(
        &data_home.join("applications").join(DESKTOP_ENTRY_NAME),
        format!(
            "[Desktop Entry]\nType=Application\nExec=env HOME={} XDG_CONFIG_HOME={} {}\n",
            child_home.display(),
            child_config.display(),
            launcher.display(),
        ),
        0o600,
    );
    let mut env = environment(&home);
    env.insert("XDG_DATA_HOME".into(), data_home.as_os_str().to_owned());
    let target = probe_desktop_capability(None, &env).await.unwrap();
    let DesktopAvailability::Verified(target) = target else {
        panic!("valid Desktop entry must verify");
    };
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        "--print-build-info\n"
    );
    assert_eq!(
        fs::read_to_string(environment_log).unwrap(),
        format!(
            "HOME={}\nXDG_CONFIG_HOME={}\n",
            child_home.display(),
            child_config.display()
        )
    );
    assert_eq!(target.command.effective_config_root, child_config);
    assert_eq!(
        target.identity.descriptor_path,
        child_config
            .join("codex-desktop")
            .join(DESCRIPTOR_FILE_NAME)
    );
}
