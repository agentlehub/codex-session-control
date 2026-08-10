use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use crate::model::DesktopAttachmentIdentity;

use super::{
    super::{
        DESCRIPTOR_FILE_NAME, DescriptorInspectionFailure, DescriptorState,
        descriptor::{
            DescriptorPublicationResidue, DescriptorPublicationTestPoint, inspect_open_descriptor,
            open_descriptor_parent, parse_descriptor, publish_descriptor_with_test_point,
        },
        inspect_descriptor, inspect_descriptor_classified, preflight_descriptor_switch,
        prepare_descriptor_parent, remove_expected_descriptor, render_descriptor,
    },
    write_file,
};

fn publication_identity(root: &Path, app_id: &str) -> DesktopAttachmentIdentity {
    DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: app_id.to_owned(),
        descriptor_path: root.join("config").join(app_id).join(DESCRIPTOR_FILE_NAME),
    }
}

#[test]
fn descriptor_publication_reports_only_exact_managed_residue() {
    let root = crate::test_support::private_tempdir();
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();

    let before_identity = publication_identity(root.path(), "before-stage");
    let failure_before_stage = publish_descriptor_with_test_point(
        &before_identity,
        &expected,
        DescriptorPublicationTestPoint::BeforeStage,
    )
    .unwrap_err();
    assert_eq!(failure_before_stage.residue, None);

    let stage_identity = publication_identity(root.path(), "after-stage");
    let unverified_stage_cleanup = publish_descriptor_with_test_point(
        &stage_identity,
        &expected,
        DescriptorPublicationTestPoint::AfterStage {
            cleanup_unverified: true,
        },
    )
    .unwrap_err();
    let stage_path = match unverified_stage_cleanup.residue {
        Some(DescriptorPublicationResidue::Stage(path)) => path,
        residue => panic!("expected exact stage residue, got {residue:?}"),
    };
    assert_eq!(stage_path.parent(), stage_identity.descriptor_path.parent());
    assert!(stage_path.exists());
    assert_ne!(stage_path, stage_identity.descriptor_path);

    let final_identity = publication_identity(root.path(), "after-rename");
    let post_rename_failure = publish_descriptor_with_test_point(
        &final_identity,
        &expected,
        DescriptorPublicationTestPoint::AfterRename,
    )
    .unwrap_err();
    assert_eq!(
        post_rename_failure.residue,
        Some(DescriptorPublicationResidue::Final(
            final_identity.descriptor_path.clone()
        ))
    );
    assert_eq!(fs::read(&final_identity.descriptor_path).unwrap(), expected);
}

#[test]
fn descriptor_rendering_and_inspection_are_value_exact_and_safe() {
    let root = crate::test_support::private_tempdir();
    let config = root.path().join("config");
    let identity = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "codex-desktop".to_owned(),
        descriptor_path: config.join("codex-desktop").join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&identity).unwrap();
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    let parent = open_descriptor_parent(&identity).unwrap().unwrap();
    let file_name = identity.descriptor_path.file_name().unwrap();
    let expected_document = parse_descriptor(&expected).unwrap();
    assert_eq!(
        inspect_descriptor(&identity, &expected).unwrap(),
        DescriptorState::Absent
    );

    write_file(
        &identity.descriptor_path,
        br#"{ "transport":"unix", "socketPath":"/run/user/1000/app-server.sock", "schemaVersion":1 }"#,
        0o600,
    );
    assert_eq!(
        inspect_descriptor(&identity, &expected).unwrap(),
        DescriptorState::Expected
    );
    write_file(
        &identity.descriptor_path,
        br#"{"schemaVersion":1,"transport":"unix","socketPath":"/run/user/1000/other.sock"}"#,
        0o600,
    );
    assert_eq!(
        inspect_descriptor(&identity, &expected).unwrap(),
        DescriptorState::Foreign
    );
    write_file(&identity.descriptor_path, &expected, 0o600);
    for mode in [0o644, 0o1600, 0o2600, 0o4600] {
        fs::set_permissions(&identity.descriptor_path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            inspect_open_descriptor(&parent, file_name, &expected_document).is_err(),
            "{mode:04o}"
        );
        assert_eq!(fs::read(&identity.descriptor_path).unwrap(), expected);
        assert_eq!(
            fs::metadata(&identity.descriptor_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            mode
        );
    }
    assert!(remove_expected_descriptor(&identity, &expected).is_err());
    assert_eq!(fs::read(&identity.descriptor_path).unwrap(), expected);
    assert_eq!(
        fs::metadata(&identity.descriptor_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o4600
    );
}

#[test]
fn descriptor_parent_creation_is_limited_to_the_root_and_app_child() {
    let root = crate::test_support::private_tempdir();
    let config = root.path().join("config");
    let identity = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "codex-desktop".to_owned(),
        descriptor_path: config.join("codex-desktop").join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&identity).unwrap();
    for directory in [&config, &config.join("codex-desktop")] {
        let metadata = fs::metadata(directory).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    }
    assert!(!identity.descriptor_path.exists());
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    assert!(preflight_descriptor_switch(None, &identity, &expected).is_ok());
}

#[test]
fn switch_preflight_never_mutates_and_rejects_foreign_paths() {
    let root = crate::test_support::private_tempdir();
    let old = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "old-desktop".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("old-desktop")
            .join(DESCRIPTOR_FILE_NAME),
    };
    let new = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "new-desktop".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("new-desktop")
            .join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&old).unwrap();
    prepare_descriptor_parent(&new).unwrap();
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    write_file(&old.descriptor_path, &expected, 0o600);
    let old_bytes = fs::read(&old.descriptor_path).unwrap();
    assert!(preflight_descriptor_switch(Some(&old), &new, &expected).is_ok());
    assert_eq!(fs::read(&old.descriptor_path).unwrap(), old_bytes);
    assert!(!new.descriptor_path.exists());
    write_file(
        &new.descriptor_path,
        br#"{"schemaVersion":1,"transport":"unix","socketPath":"/run/user/1000/foreign.sock"}"#,
        0o600,
    );
    assert!(preflight_descriptor_switch(Some(&old), &new, &expected).is_err());
    assert_eq!(fs::read(&old.descriptor_path).unwrap(), old_bytes);
}

#[test]
fn descriptor_parent_creation_requests_private_new_directories_without_chmodding_existing_ones() {
    let root = crate::test_support::private_tempdir();
    let config = root.path().join("config");
    let app = config.join("codex-desktop");
    fs::create_dir(&config).unwrap();
    fs::create_dir(&app).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o710)).unwrap();
    let identity = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "codex-desktop".to_owned(),
        descriptor_path: app.join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&identity).unwrap();
    assert_eq!(fs::metadata(config).unwrap().mode() & 0o777, 0o750);
    assert_eq!(fs::metadata(app).unwrap().mode() & 0o777, 0o710);
}

#[test]
fn descriptor_inspection_and_switch_preflight_reject_unsafe_and_foreign_states_without_writes() {
    let root = crate::test_support::private_tempdir();
    let first = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "first".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("first")
            .join(DESCRIPTOR_FILE_NAME),
    };
    let second = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "second".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("second")
            .join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&first).unwrap();
    prepare_descriptor_parent(&second).unwrap();
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    write_file(&first.descriptor_path, &expected, 0o600);
    let first_before = fs::read(&first.descriptor_path).unwrap();
    assert!(preflight_descriptor_switch(Some(&first), &first, &expected).is_ok());
    assert_eq!(fs::read(&first.descriptor_path).unwrap(), first_before);
    write_file(
        &second.descriptor_path,
        br#"{"schemaVersion":1,"transport":"unix","socketPath":"/run/user/1000/foreign.sock"}"#,
        0o600,
    );
    let second_before = fs::read(&second.descriptor_path).unwrap();
    assert!(preflight_descriptor_switch(Some(&first), &second, &expected).is_err());
    assert_eq!(fs::read(&first.descriptor_path).unwrap(), first_before);
    assert_eq!(fs::read(&second.descriptor_path).unwrap(), second_before);
    fs::remove_file(&second.descriptor_path).unwrap();
    std::os::unix::fs::symlink(&first.descriptor_path, &second.descriptor_path).unwrap();
    assert!(inspect_descriptor(&second, &expected).is_err());
    fs::remove_file(&second.descriptor_path).unwrap();
    fs::create_dir(&second.descriptor_path).unwrap();
    assert!(inspect_descriptor(&second, &expected).is_err());
}

#[test]
fn descriptor_bytes_and_open_parent_race_checks_are_exact_and_non_following() {
    let root = crate::test_support::private_tempdir();
    let identity = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "codex-desktop".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("codex-desktop")
            .join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&identity).unwrap();
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    assert_eq!(
        expected,
        br#"{"schemaVersion":1,"transport":"unix","socketPath":"/run/user/1000/app-server.sock"}"#
    );
    let parent = open_descriptor_parent(&identity).unwrap().unwrap();
    let file_name = identity.descriptor_path.file_name().unwrap();
    write_file(&identity.descriptor_path, &expected, 0o600);
    assert_eq!(
        inspect_open_descriptor(&parent, file_name, &parse_descriptor(&expected).unwrap()).unwrap(),
        DescriptorState::Expected
    );
    fs::remove_file(&identity.descriptor_path).unwrap();
    std::os::unix::fs::symlink(root.path().join("foreign"), &identity.descriptor_path).unwrap();
    assert!(
        inspect_open_descriptor(&parent, file_name, &parse_descriptor(&expected).unwrap()).is_err()
    );
    assert!(matches!(
        inspect_descriptor_classified(&identity, &expected),
        Err(DescriptorInspectionFailure::Inconclusive(_))
    ));
    fs::remove_file(&identity.descriptor_path).unwrap();
    fs::set_permissions(
        identity.descriptor_path.parent().unwrap(),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(inspect_descriptor(&identity, &expected).is_err());
    assert!(matches!(
        inspect_descriptor_classified(&identity, &expected),
        Err(DescriptorInspectionFailure::Fault(_))
    ));
}

#[test]
fn switch_preflight_rejects_old_foreign_and_unsafe_paths_without_mutating_either_descriptor() {
    let root = crate::test_support::private_tempdir();
    let old = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "old".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("old")
            .join(DESCRIPTOR_FILE_NAME),
    };
    let new = DesktopAttachmentIdentity {
        launcher_path: PathBuf::from("/bin/true"),
        app_id: "new".to_owned(),
        descriptor_path: root
            .path()
            .join("config")
            .join("new")
            .join(DESCRIPTOR_FILE_NAME),
    };
    prepare_descriptor_parent(&old).unwrap();
    prepare_descriptor_parent(&new).unwrap();
    let expected = render_descriptor(Path::new("/run/user/1000/app-server.sock")).unwrap();
    write_file(
        &old.descriptor_path,
        br#"{"schemaVersion":1,"transport":"unix","socketPath":"/run/user/1000/foreign.sock"}"#,
        0o600,
    );
    let old_before = fs::read(&old.descriptor_path).unwrap();
    assert!(preflight_descriptor_switch(Some(&old), &new, &expected).is_err());
    assert_eq!(fs::read(&old.descriptor_path).unwrap(), old_before);
    fs::remove_file(&old.descriptor_path).unwrap();
    fs::set_permissions(
        old.descriptor_path.parent().unwrap(),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(preflight_descriptor_switch(Some(&old), &new, &expected).is_err());
    assert!(!new.descriptor_path.exists());
}
