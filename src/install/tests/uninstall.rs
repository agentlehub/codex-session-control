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

fn expected_receipt(desktop_removed: bool) -> &'static str {
    if desktop_removed {
        "Codex Session Control was uninstalled.\n\n\
Your Codex data is unchanged.\n\
If Codex Desktop is already running, restart it to continue without Codex Session Control.\n"
    } else {
        "Codex Session Control was uninstalled.\n\nYour Codex data is unchanged.\n"
    }
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
async fn uninstall_producer_boundaries_select_complete_failures() {
    use crate::cli_output::{
        IndependentTerminal, ManualCleanup, NativeCleanupCommand, StopThenRetry, UserFailure,
    };

    let self_hosted = Fixture::new();
    let _authority = FakeAuthority::start(&self_hosted.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(self_hosted.context(true)).await.unwrap();
    fs::write(
        &self_hosted.whoami_unit,
        b"codex-session-control-test-Setup1.service\n",
    )
    .unwrap();
    assert_eq!(
        uninstall_with_context(context(&self_hosted))
            .await
            .unwrap_err(),
        UserFailure::IndependentTerminal(IndependentTerminal::Uninstall)
    );

    let unproven = Fixture::new();
    let _authority = FakeAuthority::start(&unproven.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(unproven.context(true)).await.unwrap();
    fs::write(
        &unproven.systemctl_fail,
        "--user is-active codex-session-control-test-Setup1.service",
    )
    .unwrap();
    assert_eq!(
        uninstall_with_context(context(&unproven))
            .await
            .unwrap_err(),
        UserFailure::StopThenRetry(StopThenRetry::UninstallUnsafeStopThenUninstall)
    );

    let stop_state = Fixture::new();
    let _authority = FakeAuthority::start(&stop_state.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(stop_state.context(true)).await.unwrap();
    fs::write(
        &stop_state.systemctl_fail,
        "--user disable --now codex-session-control-test-Setup1.service",
    )
    .unwrap();
    assert_eq!(
        uninstall_with_context(context(&stop_state))
            .await
            .unwrap_err(),
        UserFailure::StopThenRetry(StopThenRetry::UninstallServiceStateStopThenUninstall)
    );

    let resolution = Fixture::new();
    let _authority = FakeAuthority::start(&resolution.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(resolution.context(true)).await.unwrap();
    let empty_bin = resolution._root.path().join("empty-bin");
    fs::create_dir(&empty_bin).unwrap();
    let mut resolution_context = context(&resolution);
    resolution_context.path_environment = std::env::join_paths([empty_bin]).unwrap();
    assert_eq!(
        uninstall_with_context(resolution_context)
            .await
            .unwrap_err(),
        UserFailure::Ordinary(crate::cli_output::OrdinaryFailure::UninstallServiceStopRetry)
    );

    let identity = Fixture::new();
    let _authority = FakeAuthority::start(&identity.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(identity.context(true)).await.unwrap();
    fs::remove_file(&identity.paths.config).unwrap();
    fs::remove_file(&identity.paths.manifest).unwrap();
    assert!(matches!(
        uninstall_with_context(context(&identity))
            .await
            .unwrap_err(),
        UserFailure::RollbackIncomplete(_)
    ));

    let descriptor = Fixture::new();
    let _authority = setup_attached(&descriptor).await;
    let descriptor_path = descriptor
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    fs::set_permissions(&descriptor_path, fs::Permissions::from_mode(0o644)).unwrap();
    let descriptor_error = uninstall_with_context(context(&descriptor))
        .await
        .unwrap_err();
    assert!(matches!(
        descriptor_error,
        UserFailure::RollbackIncomplete(_)
    ));
    assert!(
        descriptor_error
            .render()
            .stderr
            .contains(&descriptor_path.display().to_string())
    );

    let cleanup = Fixture::new();
    let _authority = FakeAuthority::start(&cleanup.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(cleanup.context(true)).await.unwrap();
    fs::set_permissions(&cleanup.paths.unit, fs::Permissions::from_mode(0o666)).unwrap();
    let cleanup_error = uninstall_with_context(context(&cleanup)).await.unwrap_err();
    assert!(matches!(cleanup_error, UserFailure::RollbackIncomplete(_)));
    assert!(
        cleanup_error
            .render()
            .stderr
            .contains(&cleanup.paths.unit.display().to_string())
    );

    let plugin = Fixture::new();
    let _authority = FakeAuthority::start(&plugin.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(plugin.context(true)).await.unwrap();
    fs::write(
        &plugin.codex_fail,
        "plugin remove codex-session-control@codex-session-control-local --json",
    )
    .unwrap();
    let plugin_error = uninstall_with_context(context(&plugin)).await.unwrap_err();
    assert!(matches!(plugin_error, UserFailure::ManualCleanup(_)));
    assert!(
        plugin_error
            .render()
            .stderr
            .contains("plugin remove codex-session-control@codex-session-control-local --json")
    );

    let marketplace = Fixture::new();
    let _authority = FakeAuthority::start(&marketplace.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(marketplace.context(true)).await.unwrap();
    fs::write(
        &marketplace.codex_fail,
        "plugin marketplace remove codex-session-control-local --json",
    )
    .unwrap();
    let marketplace_error = uninstall_with_context(context(&marketplace))
        .await
        .unwrap_err();
    assert!(matches!(marketplace_error, UserFailure::ManualCleanup(_)));
    assert!(
        marketplace_error
            .render()
            .stderr
            .contains("plugin marketplace remove codex-session-control-local --json")
    );

    let manifest = Fixture::new();
    let _authority = FakeAuthority::start(&manifest.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(manifest.context(true)).await.unwrap();
    let mut manifest_context = context(&manifest);
    manifest_context.target = manifest_context
        .target
        .fail_after_completed_stage("manifest-remove");
    let manifest_error = uninstall_with_context(manifest_context).await.unwrap_err();
    assert!(matches!(manifest_error, UserFailure::RollbackIncomplete(_)));
    assert!(
        manifest_error
            .render()
            .stderr
            .contains(&manifest.paths.manifest.display().to_string())
    );
    assert!(manifest.paths.manifest.exists());

    let terminal = Fixture::new();
    let _authority = FakeAuthority::start(&terminal.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(terminal.context(true)).await.unwrap();
    fs::write(
        terminal.paths.data_root.join("operator-content"),
        b"preserve",
    )
    .unwrap();
    let terminal_error = uninstall_with_context(context(&terminal))
        .await
        .unwrap_err();
    assert!(matches!(
        terminal_error,
        UserFailure::TerminalPartialUninstall(_)
    ));
    let terminal_rendered = terminal_error.render();
    assert!(
        terminal_rendered
            .stderr
            .contains(&terminal.paths.data_root.display().to_string())
    );
    assert!(terminal_rendered.stderr.contains("Do not rerun"));
    assert!(!terminal_rendered.stderr.contains("Try again:"));
    assert!(!terminal.paths.manifest.exists());

    for (command, executable) in [
        (NativeCleanupCommand::RemovePlugin, None),
        (
            NativeCleanupCommand::RemoveMarketplace,
            Some(PathBuf::from("/opt/Codex CLI/codex")),
        ),
    ] {
        let quoted_executable = executable
            .as_ref()
            .map(|path| crate::install::shell_quote_path(path).unwrap());
        let failure = UserFailure::ManualCleanup(ManualCleanup::new(
            command,
            PathBuf::from("/home/test/Codex Home's"),
            executable,
        ));
        let rendered = failure.render();
        assert!(rendered.stderr.contains(
            &crate::install::shell_quote_path(Path::new("/home/test/Codex Home's")).unwrap()
        ));
        if let Some(quoted_executable) = quoted_executable {
            assert!(rendered.stderr.contains(&quoted_executable));
        } else {
            assert!(rendered.stderr.contains(" codex plugin"));
        }
        assert_eq!(rendered.exit_code, 1);
    }
}

#[tokio::test]
async fn uninstall_default_and_verbose_are_behaviorally_identical() {
    use crate::diagnostics::{DiagnosticCommand, Diagnostics};

    let default_fixture = Fixture::new();
    let verbose_fixture = Fixture::new();
    let _default_authority =
        FakeAuthority::start(&default_fixture.paths, TESTED_CODEX_VERSION).await;
    let _verbose_authority =
        FakeAuthority::start(&verbose_fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(default_fixture.context(true))
        .await
        .unwrap();
    setup_with_context(verbose_fixture.context(true))
        .await
        .unwrap();
    default_fixture.clear_logs();
    verbose_fixture.clear_logs();
    let mut off = Diagnostics::new(false, DiagnosticCommand::Uninstall);
    let mut verbose = Diagnostics::record(DiagnosticCommand::Uninstall);

    let default = uninstall_with_context_and_diagnostics(context(&default_fixture), &mut off)
        .await
        .unwrap()
        .render();
    let recorded = uninstall_with_context_and_diagnostics(context(&verbose_fixture), &mut verbose)
        .await
        .unwrap()
        .render();

    let recorded = with_recorded_diagnostics(recorded, &verbose);

    assert_eq!(default.stdout, recorded.stdout);
    assert_eq!(
        default.stderr,
        without_verbose_diagnostics(&recorded.stderr)
    );
    assert_eq!(default.exit_code, recorded.exit_code);
    assert!(
        verbose
            .recorded_lines()
            .iter()
            .all(|line| line.starts_with("[verbose] uninstall:"))
    );
    assert!(
        verbose
            .recorded_lines()
            .iter()
            .any(|line| line.ends_with("completed service-stop\n"))
    );
    assert!(
        verbose
            .recorded_lines()
            .iter()
            .any(|line| line.ends_with("completed service-stop-verify\n"))
    );
    assert_eq!(
        default_fixture.systemctl_log(),
        verbose_fixture.systemctl_log()
    );
    let native_commands = |log: String| {
        log.lines()
            .map(|line| line.split('|').next().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        native_commands(default_fixture.codex_log()),
        native_commands(verbose_fixture.codex_log())
    );
    assert_eq!(
        default_fixture.paths.manifest.exists(),
        verbose_fixture.paths.manifest.exists()
    );
    assert_eq!(
        default_fixture.paths.binary.exists(),
        verbose_fixture.paths.binary.exists()
    );
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

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::IndependentTerminal(
            crate::cli_output::IndependentTerminal::Uninstall
        )
    ));
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

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::StopThenRetry(
            crate::cli_output::StopThenRetry::UninstallUnsafeStopThenUninstall
        )
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

    let report = uninstall_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert_eq!(report.stdout, expected_receipt(false));
    assert!(report.stderr.is_empty());
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

    let rendered = error.render();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert!(
        rendered
            .stderr
            .contains("Try again:\n  codex-session-control uninstall")
    );
    assert!(rendered.stderr.contains(&config_dir.display().to_string()));
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

    let rendered = error.render();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert!(
        rendered
            .stderr
            .contains("Try again:\n  codex-session-control uninstall")
    );
    assert!(rendered.stderr.contains(&config_dir.display().to_string()));
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

    let rendered = error.render();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::TerminalPartialUninstall(_)
    ));
    assert!(
        rendered
            .stderr
            .contains(&fixture.paths.data_root.display().to_string())
    );
    assert!(
        rendered
            .stderr
            .contains("Do not rerun `codex-session-control uninstall`")
    );
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

    let rendered = error.render();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::TerminalPartialUninstall(_)
    ));
    assert!(
        rendered
            .stderr
            .contains(&fixture.paths.data_root.display().to_string())
    );
    assert!(
        !rendered
            .stderr
            .contains(&fixture.paths.binary.display().to_string())
    );
    assert!(!rendered.stderr.contains("Try again:"));
    assert!(
        rendered
            .stderr
            .contains("Do not rerun `codex-session-control uninstall`")
    );
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

        assert!(matches!(
            error,
            crate::cli_output::UserFailure::StopThenRetry(
                crate::cli_output::StopThenRetry::UninstallServiceStateStopThenUninstall
            )
        ));
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

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::ManualCleanup(_)
    ));
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
    let report = uninstall_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert_eq!(report.stdout, expected_receipt(false));
    assert!(report.stderr.is_empty());
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

    let rendered = error.render();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert!(
        rendered
            .stderr
            .contains("installed Codex Session Control state")
    );
    assert!(
        rendered
            .stderr
            .contains(&fixture.paths.config.display().to_string())
    );
    assert!(
        rendered
            .stderr
            .contains(&fixture.paths.manifest.display().to_string())
    );
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

    let rendered = error.render();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::ManualCleanup(_)
    ));
    assert!(rendered.stderr.contains(&format!(
        "CODEX_HOME={} codex plugin remove",
        shell_quote_path(&fixture.paths.codex_home).unwrap()
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

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert!(fixture.paths.marketplace.exists());
    assert!(fixture.paths.config.exists());
    assert!(fixture.paths.manifest.is_dir());
    assert!(fixture.paths.binary.exists());

    fs::remove_dir(&fixture.paths.manifest).unwrap();
    fs::write(&fixture.paths.manifest, manifest).unwrap();
    fs::set_permissions(&fixture.paths.manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let report = uninstall_with_context(context(&fixture))
        .await
        .unwrap()
        .render();
    assert_eq!(report.stdout, expected_receipt(false));
    assert!(!fixture.paths.binary.exists());
    drop(authority);
}

#[tokio::test]
async fn untested_codex_version_does_not_change_approved_uninstall_output() {
    let fixture = Fixture::new();
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    fs::write(
        &fixture.codex_version,
        format!("codex-cli {untested_version}\n"),
    )
    .unwrap();
    let authority = FakeAuthority::start(&fixture.paths, &untested_version).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    let report = uninstall_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert_eq!(report.stdout, expected_receipt(false));
    assert!(report.stderr.is_empty());
    drop(authority);
}
