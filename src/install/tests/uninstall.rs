use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

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

async fn setup_attached(fixture: &Fixture) -> FakeAuthority {
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher);
    setup_with_context(setup).await.unwrap();
    authority
}

fn snapshot_tree(path: &Path, root: &Path, snapshot: &mut Vec<(PathBuf, u32, Option<Vec<u8>>)>) {
    let metadata = fs::symlink_metadata(path).unwrap();
    let relative = path.strip_prefix(root).unwrap().to_path_buf();
    let mode = metadata.permissions().mode() & 0o777;
    if metadata.is_dir() {
        snapshot.push((relative, mode, None));
        let mut children: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        children.sort();
        for child in children {
            snapshot_tree(&child, root, snapshot);
        }
    } else {
        snapshot.push((relative, mode, Some(fs::read(path).unwrap())));
    }
}

fn tree_snapshot(path: &Path) -> Vec<(PathBuf, u32, Option<Vec<u8>>)> {
    let mut snapshot = Vec::new();
    snapshot_tree(path, path, &mut snapshot);
    snapshot
}

fn protected_paths(fixture: &Fixture) -> Vec<(PathBuf, Vec<u8>, u32)> {
    [
        ("auth.json", b"auth".as_slice()),
        ("tasks/tasks.db", b"tasks".as_slice()),
        ("rollouts/rollout.jsonl", b"rollout".as_slice()),
        ("config.toml", b"unrelated config".as_slice()),
        (
            "plugins/unrelated/plugin.json",
            b"unrelated plugin".as_slice(),
        ),
    ]
    .into_iter()
    .map(|(relative, bytes)| {
        let path = fixture.paths.codex_home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        (path, bytes.to_vec(), mode)
    })
    .collect()
}

fn file_snapshot(path: &Path) -> (Vec<u8>, u32) {
    (
        fs::read(path).unwrap(),
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
    )
}

fn assert_file_snapshot(path: &Path, expected: &(Vec<u8>, u32)) {
    assert_eq!(file_snapshot(path), *expected, "{}", path.display());
}

struct StopRefusalSnapshot {
    descriptor: (Vec<u8>, u32),
    unit: (Vec<u8>, u32),
    marketplace: Vec<(PathBuf, u32, Option<Vec<u8>>)>,
    configuration: (Vec<u8>, u32),
    manifest: (Vec<u8>, u32),
    binary: (Vec<u8>, u32),
    plugin_state: (Vec<u8>, u32),
    socket: (u32, u64),
    protected: Vec<(PathBuf, Vec<u8>, u32)>,
}

fn snapshot_stop_refusal_state(fixture: &Fixture, descriptor: &Path) -> StopRefusalSnapshot {
    let socket = fs::symlink_metadata(&fixture.paths.socket).unwrap();
    StopRefusalSnapshot {
        descriptor: file_snapshot(descriptor),
        unit: file_snapshot(&fixture.paths.unit),
        marketplace: tree_snapshot(&fixture.paths.marketplace),
        configuration: file_snapshot(&fixture.paths.config),
        manifest: file_snapshot(&fixture.paths.manifest),
        binary: file_snapshot(&fixture.paths.binary),
        plugin_state: file_snapshot(&fixture.plugin_state),
        socket: (socket.permissions().mode(), socket.ino()),
        protected: protected_paths(fixture),
    }
}

fn assert_stop_refusal_state(fixture: &Fixture, descriptor: &Path, expected: StopRefusalSnapshot) {
    assert_file_snapshot(descriptor, &expected.descriptor);
    assert_file_snapshot(&fixture.paths.unit, &expected.unit);
    assert_eq!(
        tree_snapshot(&fixture.paths.marketplace),
        expected.marketplace
    );
    assert_file_snapshot(&fixture.paths.config, &expected.configuration);
    assert_file_snapshot(&fixture.paths.manifest, &expected.manifest);
    assert_file_snapshot(&fixture.paths.binary, &expected.binary);
    assert_file_snapshot(&fixture.plugin_state, &expected.plugin_state);
    let socket = fs::symlink_metadata(&fixture.paths.socket).unwrap();
    assert_eq!((socket.permissions().mode(), socket.ino()), expected.socket);
    for (path, bytes, mode) in expected.protected {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{}", path.display());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
}

#[tokio::test]
async fn active_self_hosted_uninstall_refuses_before_every_removal() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(
        &fixture.whoami_unit,
        b"codex-session-control-test-Setup1.service\n",
    )
    .unwrap();
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    let before = snapshot_stop_refusal_state(&fixture, &descriptor);
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("running inside the managed app-server")
    );
    assert!(
        error
            .to_string()
            .contains("codex-session-control uninstall")
    );
    assert_eq!(
        fixture.systemctl_log(),
        "--user is-active codex-session-control-test-Setup1.service\n--user whoami\n"
    );
    assert!(!fixture.systemctl_log().contains("disable --now"));
    assert!(!fixture.systemctl_log().contains("daemon-reload"));
    assert_stop_refusal_state(&fixture, &descriptor, before);
}

#[tokio::test]
async fn unproven_active_uninstall_refuses_before_every_removal() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(&fixture.systemctl_fail, b"--user whoami").unwrap();
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    let before = snapshot_stop_refusal_state(&fixture, &descriptor);
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("caller independence cannot be proven")
    );
    assert!(error.to_string().contains(
        "systemctl --user stop codex-session-control-test-Setup1.service\ncodex-session-control uninstall"
    ));
    assert!(!fixture.systemctl_log().contains("disable --now"));
    assert!(!fixture.systemctl_log().contains("daemon-reload"));
    assert_stop_refusal_state(&fixture, &descriptor, before);
}

#[tokio::test]
async fn uninstall_is_service_first_uses_exact_order_and_preserves_native_home() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
        "--user is-active codex-session-control-test-Setup1.service\n".to_owned()
            + "--user whoami\n"
            + "--user disable --now codex-session-control-test-Setup1.service\n"
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        if verification_failure {
            fs::write(&fixture.fail_service_verify_after_stop, b"fail").unwrap();
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    fs::write(
        &fixture.codex_version,
        format!("codex-cli {untested_version}\n"),
    )
    .unwrap();
    let authority = FakeAuthority::start(&fixture.paths, &untested_version).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    let report = uninstall_with_context(context(&fixture)).await.unwrap();

    assert_eq!(report.stdout, expected_receipt(&fixture.paths.codex_home));
    assert_eq!(
        report.stderr,
        format!(
            "{}Compatibility warning: Codex app-server {untested_version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
            expected_stages(),
            env!("CARGO_PKG_VERSION"),
        )
    );
    drop(authority);
}
