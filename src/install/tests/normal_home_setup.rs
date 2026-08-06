use std::{fs, os::unix::fs::PermissionsExt};

use super::support::{FakeAuthority, Fixture};
use super::*;
use crate::model::InstalledRelease;

const SETUP_STAGES: [&str; 13] = [
    "preflight",
    "binary",
    "configuration",
    "projection",
    "plugin-marketplace",
    "plugin-install",
    "desktop-discovery",
    "descriptor",
    "service-unit",
    "daemon-reload",
    "service-enable",
    "service-verify",
    "manifest",
];

#[tokio::test]
async fn setup_keeps_normal_home_shared_and_never_invokes_login() {
    let fixture = Fixture::new();
    let selected_home = fixture.paths.codex_home.clone();
    let preserved = [
        (selected_home.join("auth.json"), b"credentials".as_slice()),
        (selected_home.join("tasks.json"), b"tasks".as_slice()),
        (selected_home.join("rollouts.json"), b"rollouts".as_slice()),
        (
            selected_home.join("config.toml"),
            b"unrelated-config".as_slice(),
        ),
        (
            selected_home.join("plugins/unrelated.json"),
            b"unrelated-plugin".as_slice(),
        ),
    ];
    for (path, bytes) in &preserved {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;

    let report = setup_with_context(fixture.context(true)).await.unwrap();

    assert_eq!(
        report
            .stderr
            .lines()
            .filter_map(|line| line.strip_prefix("completed: "))
            .collect::<Vec<_>>(),
        SETUP_STAGES
    );
    let log = fixture.codex_log();
    assert!(!log.contains("login"), "{log}");
    for line in log.lines() {
        assert!(
            line.contains(&format!("CODEX_HOME={}", selected_home.display())),
            "native command lost the persisted selected home: {line}"
        );
    }
    for (path, bytes) in preserved {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    let config: ProductConfig =
        toml::from_str(&fs::read_to_string(&fixture.paths.config).unwrap()).unwrap();
    let manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    assert_eq!(config.codex_home, selected_home);
    assert_eq!(manifest.codex_home, selected_home);
    let unit = fs::read_to_string(&fixture.paths.unit).unwrap();
    assert!(unit.contains(&format!(
        "Environment=CODEX_HOME={}",
        selected_home.display()
    )));
    assert!(unit.contains(" app-server --listen unix://"));

    fixture.clear_logs();
    setup_with_context(fixture.context(true)).await.unwrap();
    let log = fixture.codex_log();
    for command in [
        "plugin marketplace add ",
        "plugin marketplace remove ",
        "plugin add ",
        "plugin remove ",
    ] {
        assert!(!log.contains(command), "{log}");
    }
}

#[tokio::test]
async fn exact_native_residue_keeps_the_normal_home_when_ambient_home_differs() {
    let mut fixture = Fixture::new();
    let selected_home = fixture.paths.codex_home.clone();
    let ambient_home = fixture.paths.data_root.join("ambient-codex-home");
    fs::write(
        &fixture.marketplace_state,
        fixture.paths.marketplace.display().to_string(),
    )
    .unwrap();
    fs::write(
        &fixture.plugin_state,
        fixture.paths.marketplace.display().to_string(),
    )
    .unwrap();
    fs::write(&fixture.plugin_version_state, env!("CARGO_PKG_VERSION")).unwrap();
    fixture.paths.ambient_codex_home = Some(ambient_home.clone());
    fixture.paths.native_selection_pending = true;
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;

    setup_with_context(fixture.context(true)).await.unwrap();

    let config: ProductConfig =
        toml::from_str(&fs::read_to_string(&fixture.paths.config).unwrap()).unwrap();
    assert_eq!(config.codex_home, selected_home);
    assert!(!ambient_home.exists());
}

#[tokio::test]
async fn same_release_partial_identity_repairs_without_ambient_reselection() {
    let mut fixture = Fixture::new();
    let selected_home = fixture.paths.codex_home.clone();
    let ambient_home = fixture.paths.home.join("ambient-codex-home");
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    fixture.paths.ambient_codex_home = Some(ambient_home.clone());
    fixture.paths.native_selection_pending = true;
    fs::remove_file(&fixture.paths.manifest).unwrap();
    setup_with_context(fixture.context(true)).await.unwrap();
    let repaired_manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    assert_eq!(repaired_manifest.codex_home, selected_home);

    fs::remove_file(&fixture.paths.config).unwrap();
    setup_with_context(fixture.context(true)).await.unwrap();
    let repaired_config: ProductConfig =
        toml::from_str(&fs::read_to_string(&fixture.paths.config).unwrap()).unwrap();
    assert_eq!(repaired_config.codex_home, selected_home);
    assert!(!ambient_home.exists());
}

fn context_with_persisted_codex(fixture: &Fixture, persisted_bin: &Path) -> SetupContext {
    let mut context = fixture.context(true);
    context.path_environment = std::env::join_paths([
        persisted_bin.to_path_buf(),
        fixture.fake_bin.clone(),
        fixture.paths.home.join(".local/bin"),
    ])
    .unwrap();
    context
}

fn persist_codex_executable(fixture: &Fixture) -> std::path::PathBuf {
    let persisted_bin = fixture.paths.home.join("persisted-codex-bin");
    fs::create_dir(&persisted_bin).unwrap();
    let persisted_codex = persisted_bin.join("codex");
    fs::copy(fixture.fake_bin.join("codex"), &persisted_codex).unwrap();
    fs::set_permissions(&persisted_codex, fs::Permissions::from_mode(0o755)).unwrap();
    persisted_codex
}

fn block_ambient_codex(fixture: &Fixture, ambient_log: &Path) {
    super::write_executable_fixture(
        &fixture.fake_bin.join("codex"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 70\n",
            ambient_log.display()
        ),
    );
}

fn assert_repaired_persisted_codex(fixture: &Fixture, persisted_codex: &Path, ambient_log: &Path) {
    let config: ProductConfig =
        toml::from_str(&fs::read_to_string(&fixture.paths.config).unwrap()).unwrap();
    let manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    assert_eq!(config.codex_executable, persisted_codex);
    assert_eq!(manifest.codex_executable, persisted_codex);
    assert_eq!(
        fs::read(&fixture.paths.unit).unwrap(),
        render_unit(&fixture.paths, persisted_codex).unwrap()
    );
    assert!(
        !ambient_log.exists(),
        "setup called the ambient Codex executable: {}",
        fs::read_to_string(ambient_log).unwrap_or_default()
    );
}

#[tokio::test]
async fn config_only_repair_keeps_the_persisted_codex_executable() {
    let fixture = Fixture::new();
    let persisted_codex = persist_codex_executable(&fixture);
    let persisted_bin = persisted_codex.parent().unwrap();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(context_with_persisted_codex(&fixture, persisted_bin))
        .await
        .unwrap();

    fs::remove_file(&fixture.paths.manifest).unwrap();
    let ambient_log = fixture.paths.home.join("ambient-codex.log");
    block_ambient_codex(&fixture, &ambient_log);

    setup_with_context(fixture.context(true)).await.unwrap();

    assert_repaired_persisted_codex(&fixture, &persisted_codex, &ambient_log);
}

#[tokio::test]
async fn manifest_only_repair_keeps_the_persisted_codex_executable() {
    let fixture = Fixture::new();
    let persisted_codex = persist_codex_executable(&fixture);
    let persisted_bin = persisted_codex.parent().unwrap();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(context_with_persisted_codex(&fixture, persisted_bin))
        .await
        .unwrap();

    fs::remove_file(&fixture.paths.config).unwrap();
    let ambient_log = fixture.paths.home.join("ambient-codex.log");
    block_ambient_codex(&fixture, &ambient_log);

    setup_with_context(fixture.context(true)).await.unwrap();

    assert_repaired_persisted_codex(&fixture, &persisted_codex, &ambient_log);
}

#[tokio::test]
async fn foreign_native_sources_block_repair_without_removal() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    for (foreign_state, field) in [
        (&fixture.marketplace_state, "marketplace"),
        (&fixture.plugin_state, "plugin"),
    ] {
        fs::write(
            &fixture.marketplace_state,
            fixture.paths.marketplace.display().to_string(),
        )
        .unwrap();
        fs::write(
            &fixture.plugin_state,
            fixture.paths.marketplace.display().to_string(),
        )
        .unwrap();
        fs::write(foreign_state, b"/foreign/product-source").unwrap();
        fixture.clear_logs();

        let error = setup_with_context(fixture.context(true)).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains(&format!("invalid {field}: foreign native product source"))
        );
        let log = fixture.codex_log();
        assert!(!log.contains(" remove "), "{log}");
        assert!(!log.contains(" add "), "{log}");
    }
}
