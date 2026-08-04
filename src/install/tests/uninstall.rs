use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use super::support::{FakeAuthority, Fixture};
use super::*;

fn context(fixture: &Fixture) -> LifecycleContext {
    let setup = fixture.context(true);
    LifecycleContext {
        target: setup.target,
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    }
}

fn expected_receipt(selected_home: &Path) -> String {
    format!(
        "Codex app-server service: removed\n\
Product descriptor: removed\n\
Product projection: removed\n\
Product configuration: removed\n\
Product executable: removed\n\
Codex home preserved: {}\n\
Authentication preserved: yes\n\
Tasks preserved: yes\n\
Rollouts preserved: yes\n",
        selected_home.display()
    )
}

fn expected_stages() -> &'static str {
    "completed: service-stop\n\
completed: service-stop-verify\n\
completed: descriptor-remove\n\
completed: service-unit-remove\n\
completed: plugin-remove\n\
completed: marketplace-remove\n\
completed: projection-remove\n\
completed: configuration-remove\n\
completed: manifest-remove\n\
completed: binary-remove\n"
}

#[tokio::test]
async fn uninstall_is_service_first_uses_exact_order_and_preserves_native_home() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let session = fixture
        .paths
        .codex_home
        .join("sessions/native-session.jsonl");
    fs::create_dir_all(session.parent().unwrap()).unwrap();
    fs::write(&session, b"native session").unwrap();
    let config_dir = fixture.paths.config.parent().unwrap().to_path_buf();
    let shared_sentinels = [
        (
            fixture.paths.home.join(".local/bin/unrelated-tool"),
            b"unrelated bin".as_slice(),
        ),
        (
            fixture.paths.home.join(".local/share/unrelated-data"),
            b"unrelated data".as_slice(),
        ),
        (
            fixture
                .paths
                .home
                .join(".config/systemd/user/unrelated.service"),
            b"unrelated unit".as_slice(),
        ),
        (
            fixture
                .paths
                .home
                .join(".config/codex-desktop/unrelated.json"),
            b"unrelated desktop".as_slice(),
        ),
    ];
    for (path, bytes) in &shared_sentinels {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fixture.clear_logs();

    let report = uninstall_with_context(context(&fixture)).await.unwrap();

    assert_eq!(report.stdout, expected_receipt(&fixture.paths.codex_home));
    assert_eq!(report.stderr, expected_stages());
    assert!(!config_dir.exists());
    assert!(!fixture.paths.data_root.exists());
    assert!(fixture.paths.data_root.parent().unwrap().is_dir());
    assert!(
        fixture
            .paths
            .data_root
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        fixture.systemctl_log(),
        " --user disable --now codex-session-control-test-Setup1.service\n"
            .trim_start()
            .to_owned()
            + "--user is-enabled codex-session-control-test-Setup1.service\n"
            + "--user is-active codex-session-control-test-Setup1.service\n"
            + "--user daemon-reload\n"
    );
    let codex_log = fixture.codex_log();
    let native_commands: Vec<&str> = codex_log
        .lines()
        .map(|line| line.split('|').next().unwrap())
        .collect();
    assert_eq!(
        native_commands,
        [
            "--version",
            "plugin list --json",
            "plugin remove codex-session-control@codex-session-control-local --json",
            "plugin list --json",
            "plugin marketplace list --json",
            "plugin marketplace remove codex-session-control-local --json",
            "plugin marketplace list --json",
        ]
    );
    for removed in [
        &fixture.paths.unit,
        &fixture.paths.marketplace,
        &fixture.paths.config,
        &fixture.paths.manifest,
        &fixture.paths.binary,
    ] {
        assert!(!removed.exists(), "{}", removed.display());
    }
    assert!(fixture.paths.codex_home.is_dir());
    assert_eq!(fs::read(session).unwrap(), b"native session");
    for (path, bytes) in shared_sentinels {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    drop(authority);
}

#[tokio::test]
async fn empty_config_root_with_special_mode_fails_closed_at_configuration_remove() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let config_dir = fixture.paths.config.parent().unwrap();
    fs::set_permissions(config_dir, fs::Permissions::from_mode(0o1700)).unwrap();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed at configuration-remove:"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("retry: codex-session-control uninstall")
    );
    assert!(config_dir.is_dir());
    assert_eq!(fs::read_dir(config_dir).unwrap().count(), 0);
    assert_eq!(
        fs::symlink_metadata(config_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o1700
    );
    assert!(!fixture.paths.config.exists());
    assert!(fixture.paths.manifest.exists());
    assert!(fixture.paths.binary.exists());
    assert!(fixture.paths.codex_home.is_dir());
    drop(authority);
}

#[tokio::test]
async fn nonempty_config_root_preserves_unknown_content_at_configuration_remove() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let config_dir = fixture.paths.config.parent().unwrap();
    let unknown = config_dir.join("unknown-operator-content");
    fs::write(&unknown, b"preserve exactly").unwrap();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed at configuration-remove:"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("retry: codex-session-control uninstall")
    );
    assert_eq!(fs::read(&unknown).unwrap(), b"preserve exactly");
    assert!(config_dir.is_dir());
    assert!(!fixture.paths.config.exists());
    assert!(fixture.paths.manifest.exists());
    assert!(fixture.paths.binary.exists());
    assert!(fixture.paths.codex_home.is_dir());
    drop(authority);
}

#[test]
fn empty_product_root_removal_is_idempotent_when_absent() {
    let fixture = Fixture::new();
    fs::remove_dir(&fixture.paths.data_root).unwrap();

    remove_owned_empty_dir(&fixture.paths.data_root, fixture.paths.euid).unwrap();

    assert!(!fixture.paths.data_root.exists());
    assert!(fixture.paths.data_root.parent().unwrap().is_dir());
}

#[tokio::test]
async fn empty_product_root_with_special_mode_is_preserved_as_terminal_partial() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fs::set_permissions(&fixture.paths.data_root, fs::Permissions::from_mode(0o1700)).unwrap();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed at manifest-remove: terminal partial uninstall:"),
        "{error}"
    );
    assert!(error.to_string().contains(&format!(
        "remaining product root: {}",
        fixture.paths.data_root.display()
    )));
    assert_eq!(
        fs::symlink_metadata(&fixture.paths.data_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o1700
    );
    assert_eq!(fs::read_dir(&fixture.paths.data_root).unwrap().count(), 0);
    assert!(!fixture.paths.manifest.exists());
    assert!(!fixture.paths.binary.exists());
    drop(authority);
}

#[tokio::test]
async fn nonempty_product_root_preserves_unknown_content_as_terminal_partial() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let unknown = fixture.paths.data_root.join("unknown-operator-content");
    fs::write(&unknown, b"preserve exactly").unwrap();
    assert_eq!(
        fs::symlink_metadata(&fixture.paths.data_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed at manifest-remove: terminal partial uninstall:"),
        "{error}"
    );
    assert!(error.to_string().contains(&format!(
        "remaining product root: {}",
        fixture.paths.data_root.display()
    )));
    assert!(!error.to_string().contains("remaining product executable:"));
    assert!(!error.to_string().contains("retry:"));
    assert_eq!(fs::read(&unknown).unwrap(), b"preserve exactly");
    assert!(fixture.paths.data_root.is_dir());
    assert!(!fixture.paths.manifest.exists());
    assert!(!fixture.paths.binary.exists());
    assert!(fixture.paths.codex_home.is_dir());
    drop(authority);
}

#[tokio::test]
async fn stop_or_verification_failure_removes_nothing_after_service_boundary() {
    for verification_failure in [false, true] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
        setup_with_context(fixture.context(true)).await.unwrap();
        if verification_failure {
            fs::write(&fixture.preserve_service_state, b"preserve").unwrap();
        } else {
            fs::write(
                &fixture.systemctl_fail,
                "--user disable --now codex-session-control-test-Setup1.service",
            )
            .unwrap();
        }
        fixture.clear_logs();

        let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

        let stage = if verification_failure {
            "service-stop-verify"
        } else {
            "service-stop"
        };
        assert!(error.to_string().contains(&format!("failed at {stage}:")));
        for retained in [
            &fixture.paths.unit,
            &fixture.paths.marketplace,
            &fixture.paths.config,
            &fixture.paths.manifest,
            &fixture.paths.binary,
        ] {
            assert!(retained.exists(), "{}", retained.display());
        }
        assert!(fixture.codex_log().is_empty());
    }
}

#[tokio::test]
async fn late_failure_is_retryable_without_reordering_or_restoring_state() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fs::write(
        &fixture.codex_fail,
        "plugin marketplace remove codex-session-control-local --json",
    )
    .unwrap();
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(error.to_string().contains("failed at marketplace-remove:"));
    assert!(!fixture.paths.unit.exists());
    assert!(!fixture.plugin_state.exists());
    assert!(fixture.marketplace_state.exists());
    for retained in [
        &fixture.paths.marketplace,
        &fixture.paths.config,
        &fixture.paths.manifest,
        &fixture.paths.binary,
    ] {
        assert!(retained.exists(), "{}", retained.display());
    }

    fs::remove_file(&fixture.codex_fail).unwrap();
    fixture.clear_logs();
    let report = uninstall_with_context(context(&fixture)).await.unwrap();

    assert_eq!(report.stdout, expected_receipt(&fixture.paths.codex_home));
    assert_eq!(report.stderr, expected_stages());
    assert!(
        !fixture
            .codex_log()
            .contains("plugin remove codex-session-control@")
    );
    drop(authority);
}

#[tokio::test]
async fn missing_config_and_manifest_stop_after_the_service_boundary() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fs::remove_file(&fixture.paths.config).unwrap();
    fs::remove_file(&fixture.paths.manifest).unwrap();
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(error.to_string().contains("completed: service-stop\n"));
    assert!(
        error
            .to_string()
            .contains("completed: service-stop-verify\n")
    );
    assert!(error.to_string().contains("failed at descriptor-remove:"));
    assert!(fixture.paths.unit.exists());
    assert!(fixture.codex_log().is_empty());
    assert!(fixture.paths.codex_home.is_dir());
    drop(authority);
}

#[tokio::test]
async fn unprovable_native_absence_stops_with_exact_manual_command() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let codex = std::env::split_paths(&fixture.context(true).path_environment)
        .next()
        .unwrap()
        .join("codex");
    fs::remove_file(codex).unwrap();
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("completed: service-unit-remove\n")
    );
    assert!(
        error
            .to_string()
            .contains("failed at plugin-remove: native plugin absence cannot be proven\n")
    );
    assert!(error.to_string().contains(&format!(
        "manual: CODEX_HOME='{}' '{}' plugin remove",
        fixture.paths.codex_home.display(),
        fixture.fake_bin.join("codex").display()
    )));
    assert!(!fixture.paths.unit.exists());
    assert!(fixture.paths.marketplace.exists());
    assert!(fixture.paths.binary.exists());
    assert!(fixture.paths.codex_home.is_dir());
    drop(authority);
}

#[tokio::test]
async fn filesystem_cleanup_order_is_state_based_across_partial_retry() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let manifest = fs::read(&fixture.paths.manifest).unwrap();
    fs::remove_file(&fixture.paths.manifest).unwrap();
    fs::create_dir(&fixture.paths.manifest).unwrap();
    fs::set_permissions(&fixture.paths.manifest, fs::Permissions::from_mode(0o700)).unwrap();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(error.to_string().contains("failed at descriptor-remove:"));
    assert!(fixture.paths.marketplace.exists());
    assert!(fixture.paths.config.exists());
    assert!(fixture.paths.manifest.is_dir());
    assert!(fixture.paths.binary.exists());

    fs::remove_dir(&fixture.paths.manifest).unwrap();
    fs::write(&fixture.paths.manifest, manifest).unwrap();
    fs::set_permissions(&fixture.paths.manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let report = uninstall_with_context(context(&fixture)).await.unwrap();
    assert_eq!(report.stdout, expected_receipt(&fixture.paths.codex_home));
    assert!(!fixture.paths.binary.exists());
    drop(authority);
}

#[tokio::test]
async fn untested_codex_version_adds_one_advisory_without_changing_receipt() {
    let fixture = Fixture::new();
    fs::write(&fixture.codex_version, b"codex-cli 0.147.0\n").unwrap();
    let authority = FakeAuthority::start(&fixture.paths, "0.147.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();

    let report = uninstall_with_context(context(&fixture)).await.unwrap();

    assert_eq!(report.stdout, expected_receipt(&fixture.paths.codex_home));
    assert_eq!(
        report.stderr,
        format!(
            "{}Compatibility warning: Codex app-server 0.147.0 has not been tested with codex-session-control {}; native results remain authoritative.\n",
            expected_stages(),
            env!("CARGO_PKG_VERSION")
        )
    );
    drop(authority);
}
