use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

use super::support::Fixture;
use super::*;

const COMMANDS: [&str; 8] = [
    "setup",
    "update",
    "status",
    "enable",
    "disable",
    "uninstall",
    "mcp-server",
    "codex",
];

fn fixture() -> (TempDir, ResolvedUserPaths) {
    let root = crate::test_support::private_tempdir();
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let euid = rustix::process::geteuid().as_raw();
    (root, ResolvedUserPaths::for_test(euid, home, runtime))
}

fn write_owned(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn valid_config(paths: &ResolvedUserPaths, home: &Path) -> String {
    format!(
        "schema_version = 2\ncodex_executable = \"/usr/bin/codex\"\n\
                 codex_home = \"{}\"\nsocket_path = \"{}\"\n",
        home.display(),
        paths.socket.display()
    )
}

fn valid_manifest(paths: &ResolvedUserPaths, home: &Path) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 2,
        "productVersion": env!("CARGO_PKG_VERSION"),
        "target": test_target(),
        "binarySha256": "a".repeat(64),
        "serviceUnitSha256": "b".repeat(64),
        "projectionSha256": "c".repeat(64),
        "pluginVersion": env!("CARGO_PKG_VERSION"),
        "codexExecutable": "/usr/bin/codex",
        "codexVersion": TESTED_CODEX_VERSION,
        "codexHome": home,
        "socketPath": paths.socket,
        "desktopAttachment": null
    })
}

fn write_manifest(paths: &ResolvedUserPaths, manifest: serde_json::Value) {
    write_owned(
        &paths.manifest,
        format!("{}\n", serde_json::to_string(&manifest).unwrap()),
    );
}

fn seed_case(paths: &ResolvedUserPaths, case: InstalledEvidenceCase) {
    let selected_home = paths.home.join(".codex");
    match case {
        InstalledEvidenceCase::CoherentV2 => {
            write_owned(&paths.config, valid_config(paths, &selected_home));
            write_manifest(paths, valid_manifest(paths, &selected_home));
        }
        InstalledEvidenceCase::ConfigurationOnlyV2 => {
            write_owned(&paths.config, valid_config(paths, &selected_home));
        }
        InstalledEvidenceCase::ManifestOnlyV2 => {
            write_manifest(paths, valid_manifest(paths, &selected_home));
        }
        InstalledEvidenceCase::FirstInstall => {}
        InstalledEvidenceCase::PartialArtifactsWithoutIdentity => {
            write_owned(&paths.binary, b"partial product binary");
            fs::set_permissions(&paths.binary, fs::Permissions::from_mode(0o755)).unwrap();
        }
        InstalledEvidenceCase::InvalidConfiguration => {
            write_owned(
                &paths.config,
                valid_config(paths, &selected_home).replacen(
                    "schema_version = 2",
                    "schema_version = 1",
                    1,
                ),
            );
        }
        InstalledEvidenceCase::InvalidManifest => {
            let mut manifest = valid_manifest(paths, &selected_home);
            manifest["schemaVersion"] = serde_json::json!(1);
            write_manifest(paths, manifest);
        }
        InstalledEvidenceCase::ContradictoryV2 => {
            write_owned(&paths.config, valid_config(paths, &selected_home));
            write_manifest(paths, valid_manifest(paths, &paths.home.join("other-home")));
        }
    }
}

#[test]
fn table_drives_the_only_selected_home_evidence_states() {
    let cases = [
        (InstalledEvidenceCase::CoherentV2, true),
        (InstalledEvidenceCase::ConfigurationOnlyV2, true),
        (InstalledEvidenceCase::ManifestOnlyV2, true),
        (InstalledEvidenceCase::FirstInstall, false),
        (
            InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
            false,
        ),
        (InstalledEvidenceCase::InvalidConfiguration, false),
        (InstalledEvidenceCase::InvalidManifest, false),
        (InstalledEvidenceCase::ContradictoryV2, false),
    ];
    for (expected_case, has_selected_home) in cases {
        let (_root, paths) = fixture();
        seed_case(&paths, expected_case);
        let evidence = classify_selected_home_evidence(&paths);
        assert_eq!(
            evidence.case, expected_case,
            "{expected_case:?} was classified incorrectly"
        );
        assert_eq!(
            evidence.selected_home,
            has_selected_home.then(|| paths.home.join(".codex")),
            "{expected_case:?} selected the wrong home"
        );
    }
}

#[test]
fn only_true_first_install_consults_ambient_codex_home() {
    let (_root, paths) = fixture();
    let ambient = paths.home.join("ambient-codex-home");
    assert_eq!(
        selected_codex_home(&paths, Some(ambient.as_os_str())).unwrap(),
        Some(ambient.clone())
    );

    for case in [
        InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
        InstalledEvidenceCase::InvalidConfiguration,
        InstalledEvidenceCase::InvalidManifest,
        InstalledEvidenceCase::ContradictoryV2,
    ] {
        let (_root, paths) = fixture();
        seed_case(&paths, case);
        assert_eq!(
            selected_codex_home(&paths, Some(ambient.as_os_str())).unwrap(),
            None,
            "{case:?} must not reselect from ambient CODEX_HOME"
        );
    }
}

#[test]
fn invalid_explicit_first_install_home_is_not_replaced_with_the_default() {
    let (_root, mut paths) = fixture();
    let home = paths.home.clone();
    let runtime = paths.runtime.clone();
    paths
        .resolve_first_install_selection_with_environment(
            Some(home.as_os_str()),
            Some(runtime.as_os_str()),
            Some(OsStr::new("relative-codex-home")),
            NativeProductResidue::Absent,
        )
        .unwrap();

    assert_eq!(
        classify_selected_home_evidence(&paths).case,
        InstalledEvidenceCase::FirstInstall,
        "the production-path regression fixture requires a true first install"
    );
    assert!(
        require_selected_home_evidence(&paths, &[InstalledEvidenceCase::FirstInstall], "setup")
            .is_err(),
        "setup must receive the explicit CODEX_HOME validation error"
    );
}

#[test]
fn injected_exact_native_residue_blocks_ambient_first_install_selection() {
    let (_root, mut paths) = fixture();
    let ambient = paths.home.join("ambient-codex-home");
    let home = paths.home.clone();
    let runtime = paths.runtime.clone();

    paths
        .resolve_first_install_selection_with_environment(
            Some(home.as_os_str()),
            Some(runtime.as_os_str()),
            Some(ambient.as_os_str()),
            NativeProductResidue::ExactRegistrationAndCache,
        )
        .unwrap();

    assert_eq!(
        classify_selected_home_evidence_with_native_product_artifact(
            &paths,
            NativeProductResidue::ExactRegistrationAndCache,
        )
        .case,
        InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
    );
    assert_eq!(paths.codex_home, paths.home.join(".codex"));
    assert!(
        require_selected_home_evidence(&paths, &[InstalledEvidenceCase::FirstInstall], "setup",)
            .is_err(),
        "exact native product residue must block first-install selection"
    );
}

#[test]
fn first_install_selection_rejects_product_overlap_and_creates_only_one_safe_leaf() {
    let (_root, paths) = fixture();
    for rejected in [
        paths.config.parent().unwrap().to_path_buf(),
        paths.data_root.join("nested"),
        paths.runtime_dir.join("nested"),
        PathBuf::from("relative-codex-home"),
    ] {
        assert!(
            select_first_install_codex_home(Some(rejected.as_os_str()), &paths.home, &paths)
                .is_err(),
            "{} must be rejected",
            rejected.display()
        );
    }

    fs::create_dir_all(&paths.data_root).unwrap();
    let alias = paths.home.join("data-alias");
    std::os::unix::fs::symlink(&paths.data_root, &alias).unwrap();
    let canonical_descendant = alias.join("nested");
    assert!(
        select_first_install_codex_home(
            Some(canonical_descendant.as_os_str()),
            &paths.home,
            &paths
        )
        .is_err()
    );

    let selected = select_first_install_codex_home(None, &paths.home, &paths).unwrap();
    assert_eq!(selected, paths.home.join(".codex"));
    assert!(!selected.exists());
    let entries_before = fs::read_dir(&paths.home).unwrap().count();
    create_missing_selected_codex_home(
        &selected,
        &paths.config,
        &paths.home,
        &paths.data_root,
        &paths.runtime_dir,
        paths.euid,
    )
    .unwrap();
    assert_eq!(
        fs::symlink_metadata(&selected)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::read_dir(&paths.home).unwrap().count(),
        entries_before + 1
    );
    create_missing_selected_codex_home(
        &selected,
        &paths.config,
        &paths.home,
        &paths.data_root,
        &paths.runtime_dir,
        paths.euid,
    )
    .unwrap();
    assert_eq!(
        fs::read_dir(&paths.home).unwrap().count(),
        entries_before + 1
    );
}

#[test]
fn selected_home_selection_rejects_unsafe_ancestors_symlinks_and_lexical_product_descendants() {
    let (root, paths) = fixture();
    let safe = paths.home.join("safe");
    fs::create_dir(&safe).unwrap();
    fs::set_permissions(&safe, fs::Permissions::from_mode(0o700)).unwrap();

    let unsafe_parent = safe.join("unsafe");
    fs::create_dir(&unsafe_parent).unwrap();
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o722)).unwrap();

    let symlink_target = paths.home.join("symlink-target");
    fs::create_dir(&symlink_target).unwrap();
    fs::set_permissions(&symlink_target, fs::Permissions::from_mode(0o700)).unwrap();
    let symlink_parent = paths.home.join("symlink-parent");
    std::os::unix::fs::symlink(&symlink_target, &symlink_parent).unwrap();

    fs::create_dir_all(&paths.data_root).unwrap();
    let lexical_product_descendant = safe
        .join("missing")
        .join("..")
        .join("..")
        .join(".local/share/codex-session-control/inside");
    let non_normal_but_safe = safe.join("selected").join("..").join("selected-home");

    for rejected in [
        unsafe_parent.join("selected"),
        symlink_parent.join("selected"),
        lexical_product_descendant,
        non_normal_but_safe,
    ] {
        assert!(
            select_first_install_codex_home(Some(rejected.as_os_str()), &paths.home, &paths,)
                .is_err(),
            "{} must fail before selected-home creation",
            rejected.display()
        );
    }

    let root_paths = ResolvedUserPaths::for_test(0, paths.home.clone(), paths.runtime.clone());
    assert!(
        select_first_install_codex_home(Some(OsStr::new("/")), &root_paths.home, &root_paths,)
            .is_err(),
        "root must never become a selected Codex home, including for euid 0"
    );

    let shared = root.path().join("outside-home-shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o733)).unwrap();
    let root_owned_style_paths = ResolvedUserPaths::for_test(
        paths.euid.saturating_add(1),
        paths.home.clone(),
        paths.runtime.clone(),
    );
    let error = select_first_install_codex_home(
        Some(shared.join("selected-home").as_os_str()),
        &root_owned_style_paths.home,
        &root_owned_style_paths,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("existing ancestor has unsafe mode"),
        "an arbitrary explicit home must check root-to-leaf shared ancestors before ownership"
    );
}

#[test]
fn selected_home_native_state_never_counts_as_a_product_release_artifact() {
    let (_root, paths) = fixture();
    let selected = paths.home.join(".codex");
    fs::create_dir(&selected).unwrap();
    for (relative, bytes) in [
        ("auth.json", b"credentials".as_slice()),
        ("tasks/task-1.jsonl", b"task".as_slice()),
        ("rollouts/rollout-1.jsonl", b"rollout".as_slice()),
        ("config.toml", b"native config".as_slice()),
        (
            "plugins/unrelated/plugin.json",
            b"unrelated plugin".as_slice(),
        ),
    ] {
        let path = selected.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    let evidence = classify_selected_home_evidence(&paths);
    assert_eq!(evidence.case, InstalledEvidenceCase::FirstInstall);
    assert_eq!(
        selected_codex_home(&paths, None).unwrap(),
        Some(selected.clone())
    );
}

#[test]
fn schema_one_evidence_is_invalid_and_never_selects_a_home() {
    let (_root, paths) = fixture();
    write_owned(
        &paths.config,
        format!(
            "schema_version = 1\ncodex_executable = \"/usr/bin/codex\"\n\
                     codex_home = \"{}\"\nsocket_path = \"{}\"\n",
            paths.codex_home.display(),
            paths.socket.display(),
        ),
    );
    assert_eq!(
        classify_selected_home_evidence(&paths).case,
        InstalledEvidenceCase::InvalidConfiguration,
        "schema-1 configuration cannot enter the selected-home classifier"
    );
    assert_eq!(selected_codex_home(&paths, None).unwrap(), None);

    let (_root, paths) = fixture();
    let mut manifest = valid_manifest(&paths, &paths.codex_home);
    manifest["schemaVersion"] = serde_json::json!(1);
    write_manifest(&paths, manifest);
    assert_eq!(
        classify_selected_home_evidence(&paths).case,
        InstalledEvidenceCase::InvalidManifest,
        "schema-1 manifest cannot enter the selected-home classifier"
    );
    assert_eq!(selected_codex_home(&paths, None).unwrap(), None);
}

#[test]
fn malformed_or_incompatible_schema_two_manifest_is_invalid_evidence() {
    for (field, value) in [
        ("productVersion", serde_json::json!("")),
        ("productVersion", serde_json::json!("not-a-version")),
        ("target", serde_json::json!("incompatible-target")),
        ("pluginVersion", serde_json::json!("")),
        ("pluginVersion", serde_json::json!("not-a-version")),
        ("codexVersion", serde_json::json!("")),
        ("codexVersion", serde_json::json!("not-a-version")),
    ] {
        let (_root, paths) = fixture();
        let mut manifest = valid_manifest(&paths, &paths.home.join(".codex"));
        manifest[field] = value;
        write_manifest(&paths, manifest);

        assert_eq!(
            classify_selected_home_evidence(&paths).case,
            InstalledEvidenceCase::InvalidManifest,
            "{field} must not authorize a schema-2 identity"
        );
        for (operation, permitted) in [
            (
                "setup",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ConfigurationOnlyV2,
                    InstalledEvidenceCase::ManifestOnlyV2,
                    InstalledEvidenceCase::FirstInstall,
                    InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
                ][..],
            ),
            (
                "update",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ManifestOnlyV2,
                ][..],
            ),
            ("enable", &[InstalledEvidenceCase::CoherentV2][..]),
            (
                "mcp",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ConfigurationOnlyV2,
                ][..],
            ),
        ] {
            assert!(
                require_selected_home_evidence(&paths, permitted, operation).is_err(),
                "{field} must block {operation}"
            );
        }
    }
}

#[test]
fn symlinked_evidence_source_ancestors_are_invalid_and_block_identity_loaders() {
    enum UnsafeSource {
        ConfigurationParent,
        ManifestDataRoot,
    }

    for source in [
        UnsafeSource::ConfigurationParent,
        UnsafeSource::ManifestDataRoot,
    ] {
        let (_root, paths) = fixture();
        let expected_case = match source {
            UnsafeSource::ConfigurationParent => {
                let target = paths.home.join("config-target");
                fs::create_dir(&target).unwrap();
                fs::create_dir(target.join("codex-session-control")).unwrap();
                write_owned(
                    &target.join("codex-session-control/config.toml"),
                    valid_config(&paths, &paths.home.join(".codex")),
                );
                std::os::unix::fs::symlink(&target, paths.home.join(".config")).unwrap();
                InstalledEvidenceCase::InvalidConfiguration
            }
            UnsafeSource::ManifestDataRoot => {
                let target = paths.home.join("data-target");
                fs::create_dir(&target).unwrap();
                fs::create_dir(target.join("share")).unwrap();
                fs::create_dir(target.join("share/codex-session-control")).unwrap();
                write_owned(
                    &target.join("share/codex-session-control/installed-release.json"),
                    serde_json::to_vec(&valid_manifest(&paths, &paths.home.join(".codex")))
                        .unwrap(),
                );
                std::os::unix::fs::symlink(&target, paths.home.join(".local")).unwrap();
                InstalledEvidenceCase::InvalidManifest
            }
        };

        assert_eq!(
            classify_selected_home_evidence(&paths).case,
            expected_case,
            "unsafe source ancestry must be invalid evidence"
        );
        for (operation, permitted) in [
            (
                "setup",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ConfigurationOnlyV2,
                    InstalledEvidenceCase::ManifestOnlyV2,
                    InstalledEvidenceCase::FirstInstall,
                    InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
                ][..],
            ),
            (
                "update",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ManifestOnlyV2,
                ][..],
            ),
            ("enable", &[InstalledEvidenceCase::CoherentV2][..]),
            (
                "mcp",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ConfigurationOnlyV2,
                ][..],
            ),
        ] {
            assert!(
                require_selected_home_evidence(&paths, permitted, operation).is_err(),
                "unsafe source ancestry must block {operation}"
            );
        }
    }
}

#[test]
fn intermediate_symlink_before_effective_home_invalidates_all_product_evidence() {
    let root = crate::test_support::private_tempdir();
    let actual_parent = root.path().join("actual-parent");
    let actual_home = actual_parent.join("home");
    let linked_parent = root.path().join("linked-parent");
    let runtime = root.path().join("runtime");
    fs::create_dir(&actual_parent).unwrap();
    fs::create_dir(&actual_home).unwrap();
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&actual_home, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&actual_parent, &linked_parent).unwrap();
    let paths = ResolvedUserPaths::for_test(
        rustix::process::geteuid().as_raw(),
        linked_parent.join("home"),
        runtime,
    );
    let selected_home = paths.home.join(".codex");
    write_owned(&paths.config, valid_config(&paths, &selected_home));
    write_manifest(&paths, valid_manifest(&paths, &selected_home));

    assert!(matches!(
        read_configuration_evidence(&paths),
        StoredEvidence::Invalid(InvalidEvidence::File(StatusFileError::Unsafe))
    ));
    assert!(matches!(
        read_manifest_evidence(&paths),
        StoredEvidence::Invalid(InvalidEvidence::File(StatusFileError::Unsafe))
    ));
    assert_eq!(
        classify_selected_home_evidence(&paths).case,
        InstalledEvidenceCase::InvalidConfiguration
    );
    for (operation, permitted) in [
        (
            "setup",
            &[
                InstalledEvidenceCase::CoherentV2,
                InstalledEvidenceCase::ConfigurationOnlyV2,
                InstalledEvidenceCase::ManifestOnlyV2,
                InstalledEvidenceCase::FirstInstall,
                InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
            ][..],
        ),
        (
            "update",
            &[
                InstalledEvidenceCase::CoherentV2,
                InstalledEvidenceCase::ManifestOnlyV2,
            ][..],
        ),
        ("enable", &[InstalledEvidenceCase::CoherentV2][..]),
        (
            "mcp",
            &[
                InstalledEvidenceCase::CoherentV2,
                InstalledEvidenceCase::ConfigurationOnlyV2,
            ][..],
        ),
    ] {
        assert!(
            require_selected_home_evidence(&paths, permitted, operation).is_err(),
            "intermediate HOME symlink must block {operation}"
        );
    }
}

#[tokio::test]
async fn status_does_not_probe_identity_from_an_unsafe_configuration_ancestor() {
    let fixture = Fixture::new();
    let target = fixture.paths.home.join("config-target");
    write_owned(
        &target.join("codex-session-control/config.toml"),
        valid_config(&fixture.paths, &fixture.paths.home.join(".codex")),
    );
    std::os::unix::fs::symlink(target, fixture.paths.home.join(".config")).unwrap();
    fixture.clear_logs();

    let setup = fixture.context(true);
    let report = status_with_context(StatusContext {
        target: setup.target,
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    })
    .await
    .unwrap();

    assert!(!report.healthy);
    assert!(report.stdout.contains("configuration:"));
    assert!(fixture.codex_log().is_empty(), "{:#?}", report.stdout);
}

#[tokio::test]
async fn invalid_evidence_command_matrix_preserves_identity_state_and_service_boundaries() {
    #[derive(Debug)]
    enum MatrixEvidence {
        Case(InstalledEvidenceCase),
        SymlinkedConfigurationParent,
        SymlinkedManifestDataRoot,
    }

    #[derive(Debug, Eq, PartialEq)]
    enum FilesystemSnapshot {
        Missing,
        Regular {
            mode: u32,
            bytes: Vec<u8>,
        },
        Directory {
            mode: u32,
            entries: Vec<(String, FilesystemSnapshot)>,
        },
        Symlink {
            mode: u32,
            target: PathBuf,
        },
        Other {
            mode: u32,
        },
    }

    fn snapshot(path: &Path) -> FilesystemSnapshot {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return FilesystemSnapshot::Missing;
            }
            Err(error) => panic!("cannot snapshot {}: {error}", path.display()),
        };
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.file_type().is_file() {
            return FilesystemSnapshot::Regular {
                mode,
                bytes: fs::read(path).unwrap(),
            };
        }
        if metadata.file_type().is_dir() {
            let mut entries = fs::read_dir(path)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        snapshot(&entry.path()),
                    )
                })
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            return FilesystemSnapshot::Directory { mode, entries };
        }
        if metadata.file_type().is_symlink() {
            return FilesystemSnapshot::Symlink {
                mode,
                target: fs::read_link(path).unwrap(),
            };
        }
        FilesystemSnapshot::Other { mode }
    }

    fn write_sentinel(path: &Path, bytes: &[u8], mode: u32) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn seed_protected_sentinels(fixture: &Fixture) {
        let paths = &fixture.paths;
        write_sentinel(
            &paths.codex_home.join("identity-sentinel"),
            b"selected-home-sentinel",
            0o600,
        );
        write_sentinel(
            &paths
                .codex_home
                .join("cache/codex-session-control-native-sentinel"),
            b"native-cache-sentinel",
            0o600,
        );
        write_sentinel(&paths.binary, b"binary-sentinel", 0o755);
        write_sentinel(&paths.unit, b"unit-sentinel", 0o644);
        write_sentinel(
            &paths.marketplace.join(".agents/plugins/marketplace.json"),
            b"marketplace-projection-sentinel",
            0o644,
        );
        write_sentinel(
            &paths
                .marketplace
                .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
            b"plugin-projection-sentinel",
            0o644,
        );
        write_sentinel(
            &paths
                .marketplace
                .join("plugins/codex-session-control/.mcp.json"),
            b"mcp-projection-sentinel",
            0o644,
        );
        write_sentinel(
            &fixture.marketplace_state,
            b"native-marketplace-sentinel",
            0o600,
        );
        write_sentinel(&fixture.plugin_state, b"native-plugin-sentinel", 0o600);
        write_sentinel(
            &fixture.plugin_version_state,
            b"native-plugin-version-sentinel",
            0o600,
        );
    }

    fn seed_symlinked_configuration_parent(fixture: &Fixture) {
        let paths = &fixture.paths;
        let target = paths.home.join("config-target");
        write_owned(
            &target.join("codex-session-control/config.toml"),
            valid_config(paths, &paths.home.join(".codex")),
        );
        std::os::unix::fs::symlink(target, paths.home.join(".config")).unwrap();
    }

    fn seed_symlinked_manifest_data_root(fixture: &Fixture) {
        let paths = &fixture.paths;
        let target = paths.home.join("data-target");
        write_owned(
            &target.join("share/codex-session-control/installed-release.json"),
            serde_json::to_vec(&valid_manifest(paths, &paths.home.join(".codex"))).unwrap(),
        );
        fs::remove_dir_all(paths.home.join(".local")).unwrap();
        std::os::unix::fs::symlink(target, paths.home.join(".local")).unwrap();
    }

    fn protected_state(fixture: &Fixture) -> Vec<(PathBuf, FilesystemSnapshot)> {
        let paths = &fixture.paths;
        [
            paths.codex_home.clone(),
            paths.binary.clone(),
            paths.config.clone(),
            paths.unit.clone(),
            paths.marketplace.clone(),
            paths.manifest.clone(),
            fixture.marketplace_state.clone(),
            fixture.plugin_state.clone(),
            fixture.plugin_version_state.clone(),
        ]
        .into_iter()
        .map(|path| {
            let state = snapshot(&path);
            (path, state)
        })
        .collect()
    }

    fn lifecycle_context(fixture: &Fixture) -> LifecycleContext {
        let setup = fixture.context(true);
        LifecycleContext {
            target: setup.target,
            path_environment: setup.path_environment,
            desktop_environment: setup.desktop_environment,
            cwd: setup.cwd,
        }
    }

    fn status_context(fixture: &Fixture) -> StatusContext {
        let setup = fixture.context(true);
        StatusContext {
            target: setup.target,
            path_environment: setup.path_environment,
            desktop_environment: setup.desktop_environment,
            cwd: setup.cwd,
        }
    }

    for evidence in [
        MatrixEvidence::Case(InstalledEvidenceCase::InvalidConfiguration),
        MatrixEvidence::Case(InstalledEvidenceCase::InvalidManifest),
        MatrixEvidence::Case(InstalledEvidenceCase::ContradictoryV2),
        MatrixEvidence::SymlinkedConfigurationParent,
        MatrixEvidence::SymlinkedManifestDataRoot,
    ] {
        let fixture = Fixture::new();
        match evidence {
            MatrixEvidence::Case(case) => seed_case(&fixture.paths, case),
            MatrixEvidence::SymlinkedConfigurationParent => {
                seed_symlinked_configuration_parent(&fixture);
            }
            MatrixEvidence::SymlinkedManifestDataRoot => {
                seed_symlinked_manifest_data_root(&fixture);
            }
        }
        seed_protected_sentinels(&fixture);
        let before = protected_state(&fixture);
        fixture.clear_logs();
        let unit = "codex-session-control-test-Setup1.service";
        for command in COMMANDS {
            match command {
                "setup" => {
                    assert!(
                        setup_with_context(fixture.context(true)).await.is_err(),
                        "{evidence:?}"
                    );
                    assert!(fixture.systemctl_log().is_empty(), "{evidence:?}");
                    assert!(fixture.codex_log().is_empty(), "{evidence:?}");
                }
                "update" => {
                    assert!(
                        outer_update_with_endpoints(
                            lifecycle_context(&fixture),
                            ReleaseEndpoints {
                                api: "http://127.0.0.1:9".to_owned(),
                                downloads: "http://127.0.0.1:9".to_owned(),
                            },
                        )
                        .await
                        .is_err(),
                        "{evidence:?}"
                    );
                    assert!(fixture.systemctl_log().is_empty(), "{evidence:?}");
                    assert!(fixture.codex_log().is_empty(), "{evidence:?}");
                }
                "status" => {
                    let _ = status_with_context(status_context(&fixture)).await.unwrap();
                    assert!(
                        fixture
                            .systemctl_log()
                            .lines()
                            .all(|line| line.starts_with("--user is-enabled")
                                || line.starts_with("--user is-active")),
                        "{evidence:?}: status must only probe systemd"
                    );
                    assert!(
                        fixture
                            .codex_log()
                            .lines()
                            .all(|line| line.starts_with("--version|")),
                        "{evidence:?}: status must only probe Codex"
                    );
                    fixture.clear_logs();
                }
                "enable" => {
                    assert!(
                        enable_with_context(lifecycle_context(&fixture))
                            .await
                            .is_err(),
                        "{evidence:?}"
                    );
                    assert!(fixture.systemctl_log().is_empty(), "{evidence:?}");
                    assert!(fixture.codex_log().is_empty(), "{evidence:?}");
                }
                "disable" => {
                    let error = disable_with_context(lifecycle_context(&fixture))
                        .await
                        .unwrap_err();
                    assert!(
                        error.to_string().contains(
                            "completed: service-disable\n\
completed: service-verify\n\
failed at descriptor-remove:"
                        ),
                        "{evidence:?}: {error}"
                    );
                    assert_eq!(
                        fixture.systemctl_log(),
                        format!(
                            "--user disable --now {unit}\n\
                                     --user is-enabled {unit}\n\
                                     --user is-active {unit}\n"
                        )
                    );
                    assert!(fixture.codex_log().is_empty(), "{evidence:?}");
                    fixture.clear_logs();
                }
                "uninstall" => {
                    assert!(
                        uninstall_with_context(lifecycle_context(&fixture))
                            .await
                            .is_err(),
                        "{evidence:?}"
                    );
                    assert_eq!(
                        fixture.systemctl_log(),
                        format!(
                            "--user disable --now {unit}\n\
                                     --user is-enabled {unit}\n\
                                     --user is-active {unit}\n"
                        )
                    );
                    assert!(fixture.codex_log().is_empty(), "{evidence:?}");
                    fixture.clear_logs();
                }
                "mcp-server" | "codex" => {
                    assert!(
                        load_config_from_paths(&fixture.paths).is_err(),
                        "{evidence:?} {command}"
                    );
                    assert!(fixture.systemctl_log().is_empty(), "{evidence:?}");
                    assert!(fixture.codex_log().is_empty(), "{evidence:?}");
                }
                _ => unreachable!("command surface is frozen by COMMANDS"),
            }
            for (path, expected) in &before {
                assert_eq!(
                    snapshot(path),
                    *expected,
                    "{evidence:?}: {}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn invalid_or_contradictory_evidence_blocks_each_identity_dependent_loader_without_mutation() {
    for case in [
        InstalledEvidenceCase::InvalidConfiguration,
        InstalledEvidenceCase::InvalidManifest,
        InstalledEvidenceCase::ContradictoryV2,
    ] {
        let (_root, paths) = fixture();
        seed_case(&paths, case);
        let before = [
            paths.binary.clone(),
            paths.config.clone(),
            paths.unit.clone(),
            paths.manifest.clone(),
            paths.marketplace.clone(),
        ]
        .map(|path| (path.clone(), fs::read(&path).ok()));

        for (operation, permitted) in [
            (
                "setup",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ConfigurationOnlyV2,
                    InstalledEvidenceCase::ManifestOnlyV2,
                    InstalledEvidenceCase::FirstInstall,
                    InstalledEvidenceCase::PartialArtifactsWithoutIdentity,
                ][..],
            ),
            (
                "update",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ManifestOnlyV2,
                ][..],
            ),
            ("enable", &[InstalledEvidenceCase::CoherentV2][..]),
            (
                "mcp",
                &[
                    InstalledEvidenceCase::CoherentV2,
                    InstalledEvidenceCase::ConfigurationOnlyV2,
                ][..],
            ),
        ] {
            assert!(
                require_selected_home_evidence(&paths, permitted, operation).is_err(),
                "{case:?} must block {operation}"
            );
        }

        for (path, expected) in before {
            assert_eq!(
                fs::read(&path).ok(),
                expected,
                "{case:?}: {}",
                path.display()
            );
        }
    }
}
