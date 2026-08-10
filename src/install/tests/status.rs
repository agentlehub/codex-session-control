use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    time::Duration,
};

use super::support::{FakeAuthority, Fixture};
use super::*;
use tokio::{net::UnixListener, sync::oneshot, time::timeout};

const SERVICE_STATES: [(&str, bool, bool, bool); 4] = [
    ("enabled_active_socket", true, true, true),
    ("enabled_inactive_no_socket", true, false, false),
    ("disabled_inactive_no_socket", false, false, false),
    ("disabled_active_socket", false, true, true),
];

fn context(fixture: &Fixture) -> StatusContext {
    let setup = fixture.context(true);
    StatusContext {
        target: setup.target,
        path_environment: Some(setup.path_environment),
        desktop_environment: setup.desktop_environment,
        cwd: Some(setup.cwd),
    }
}

async fn inspect_status(context: StatusContext) -> StatusResult {
    let mut diagnostics =
        crate::diagnostics::Diagnostics::new(false, crate::diagnostics::DiagnosticCommand::Status);
    status_with_context(context, &mut diagnostics).await
}

fn assert_read_only_logs(fixture: &Fixture) {
    let codex = fixture.codex_log();
    for forbidden in [
        " login|",
        "plugin marketplace add ",
        "plugin marketplace remove ",
        "plugin add ",
        "plugin remove ",
    ] {
        assert!(!codex.contains(forbidden), "{codex}");
    }
    let systemctl = fixture.systemctl_log();
    for forbidden in [
        "daemon-reload",
        " enable ",
        " disable ",
        " restart ",
        " start ",
        " stop ",
    ] {
        assert!(!systemctl.contains(forbidden), "{systemctl}");
    }
}

fn render_status(result: StatusResult) -> crate::cli_output::RenderedCli {
    UserSuccess::Status(result).render()
}

#[tokio::test]
async fn pre_context_failures_still_render_one_four_state_status_result() {
    use crate::{diagnostics::DiagnosticCommand, error::ControllerError};

    let mut diagnostics = crate::diagnostics::Diagnostics::record(DiagnosticCommand::Status);
    let result = status_from_paths(
        Err(ControllerError::InvalidData {
            field: "effective user",
            reason: "sentinel context failure",
        }),
        &mut diagnostics,
    )
    .await;

    assert_eq!(
        result,
        StatusResult::new(
            StatusState::Unhealthy,
            None,
            Some(ServiceSummary::CouldNotVerify),
            IntegrationState::CouldNotVerify,
            IntegrationState::CouldNotVerify,
            vec![StatusProblem::InstalledStateCouldNotBeVerified],
        )
    );
    assert_eq!(
        render_status(result),
        crate::cli_output::RenderedCli {
            stdout: concat!(
                "Status: unhealthy\n",
                "Service: could not verify\n",
                "Codex CLI integration: could not verify\n",
                "Codex Desktop integration: could not verify\n\n",
                "Problems:\n",
                "- The installed Codex Session Control state could not be verified.\n\n",
                "Check what needs attention:\n",
                "  codex-session-control status\n",
            )
            .to_owned(),
            stderr: String::new(),
            exit_code: 1,
        }
    );
    assert_eq!(
        diagnostics.recorded_lines(),
        ["[verbose] status: failed preflight (validation failed)\n"]
    );
}

#[tokio::test]
async fn missing_invocation_evidence_becomes_typed_inconclusive_status() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fixture.clear_logs();
    let mut missing = context(&fixture);
    missing.path_environment = None;
    missing.cwd = None;

    let result = inspect_status(missing).await;

    assert_eq!(
        result,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::CouldNotVerify),
            IntegrationState::CouldNotVerify,
            IntegrationState::Unavailable,
            vec![
                StatusProblem::InvocationContextCouldNotBeVerified,
                StatusProblem::ServiceEnablementCouldNotBeVerified,
                StatusProblem::ServiceActivityCouldNotBeVerified,
            ],
        )
    );
    assert!(fixture.systemctl_log().is_empty());
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn not_installed_status_is_exact_and_read_only() {
    let fixture = Fixture::new();
    fixture.clear_logs();

    let result = inspect_status(context(&fixture)).await;

    assert_eq!(
        result,
        StatusResult::new(
            StatusState::NotInstalled,
            None,
            None,
            IntegrationState::Unavailable,
            IntegrationState::Unavailable,
            Vec::new(),
        )
    );
    assert_eq!(render_status(result).exit_code, 1);
    assert_read_only_logs(&fixture);
    assert!(fixture.codex_log().is_empty());
}

#[tokio::test]
async fn status_default_and_verbose_are_behaviorally_identical() {
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
    let mut off = Diagnostics::new(false, DiagnosticCommand::Status);
    let mut verbose = Diagnostics::record(DiagnosticCommand::Status);

    let default = render_status(status_with_context(context(&default_fixture), &mut off).await);
    let recorded =
        render_status(status_with_context(context(&verbose_fixture), &mut verbose).await);
    let recorded = with_recorded_diagnostics(recorded, &verbose);

    assert_default_verbose_parity(&default, &recorded, &verbose, "[verbose] status:");
    assert_read_only_logs(&default_fixture);
    assert_read_only_logs(&verbose_fixture);
    assert_eq!(
        default_fixture.systemctl_log(),
        verbose_fixture.systemctl_log()
    );
    assert_eq!(
        default_fixture
            .codex_log()
            .lines()
            .map(|line| line.split('|').next().unwrap())
            .collect::<Vec<_>>(),
        verbose_fixture
            .codex_log()
            .lines()
            .map(|line| line.split('|').next().unwrap())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn native_registration_distinguishes_fault_from_could_not_verify() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let expected_plugin = fs::read(&fixture.plugin_state).unwrap();
    fixture.clear_logs();

    fs::write(&fixture.plugin_state, b"/wrong/plugin").unwrap();
    let fault = inspect_status(context(&fixture)).await;
    assert_eq!(
        fault,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::Unhealthy,
            IntegrationState::Unavailable,
            vec![StatusProblem::NativeRegistrationFault],
        )
    );

    fs::write(&fixture.plugin_state, expected_plugin).unwrap();
    fs::write(&fixture.codex_fail, "plugin list --json").unwrap();
    fixture.clear_logs();
    let unverified = inspect_status(context(&fixture)).await;
    assert_eq!(
        unverified,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::CouldNotVerify,
            IntegrationState::Unavailable,
            vec![StatusProblem::NativeRegistrationCouldNotBeVerified],
        )
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn projection_distinguishes_fault_from_could_not_verify() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let mcp = fixture
        .paths
        .marketplace
        .join("plugins/codex-session-control/.mcp.json");
    let expected_mcp = fs::read(&mcp).unwrap();
    fixture.clear_logs();

    fs::write(&mcp, b"projection drift").unwrap();
    let fault = inspect_status(context(&fixture)).await;
    assert_eq!(
        fault,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::Unhealthy,
            IntegrationState::Unavailable,
            vec![StatusProblem::ProjectionFault],
        )
    );

    fs::write(&mcp, expected_mcp).unwrap();
    fs::set_permissions(&mcp, fs::Permissions::from_mode(0o000)).unwrap();
    fixture.clear_logs();
    let unverified = inspect_status(context(&fixture)).await;
    assert_eq!(
        unverified,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::CouldNotVerify,
            IntegrationState::Unavailable,
            vec![StatusProblem::ProjectionCouldNotBeVerified],
        )
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn service_state_table_has_exact_health_stdout_and_read_only_argv() {
    for (name, enabled, active, socket) in SERVICE_STATES {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        fixture.clear_logs();
        if !enabled {
            fs::remove_file(&fixture.enabled).unwrap();
        }
        if !active {
            fs::remove_file(&fixture.active).unwrap();
        }
        let authority = if socket {
            Some(authority)
        } else {
            drop(authority);
            fs::remove_file(&fixture.paths.socket).unwrap();
            None
        };

        let report = inspect_status(context(&fixture)).await;

        let (state, service, cli, problems) = match (enabled, active, socket) {
            (true, true, true) => (
                StatusState::Healthy,
                ServiceSummary::RunningAutomatic,
                IntegrationState::Ready,
                Vec::new(),
            ),
            (false, false, false) => (
                StatusState::Disabled,
                ServiceSummary::StoppedAutomaticOff,
                IntegrationState::Unavailable,
                Vec::new(),
            ),
            (true, false, false) => (
                StatusState::Unhealthy,
                ServiceSummary::StoppedUnexpectedAutomaticOn,
                IntegrationState::Unhealthy,
                vec![
                    StatusProblem::ServiceConfiguredButStopped,
                    StatusProblem::SocketMissing,
                ],
            ),
            (false, true, true) => (
                StatusState::Unhealthy,
                ServiceSummary::CouldNotVerify,
                IntegrationState::Unhealthy,
                vec![StatusProblem::ServiceActivityCouldNotBeVerified],
            ),
            _ => unreachable!("the service-state fixture table is closed"),
        };
        assert_eq!(
            report,
            StatusResult::new(
                state,
                Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
                Some(service),
                cli,
                IntegrationState::Unavailable,
                problems,
            ),
            "{name}",
        );
        assert_read_only_logs(&fixture);
        assert!(
            fixture
                .systemctl_log()
                .contains("--user is-enabled codex-session-control-test-Setup1.service")
        );
        assert!(
            fixture
                .systemctl_log()
                .contains("--user is-active codex-session-control-test-Setup1.service")
        );
        drop(authority);
    }
}

#[tokio::test]
async fn app_server_health_does_not_connect_to_an_unsafe_socket() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    drop(authority);
    fs::remove_file(&fixture.paths.socket).unwrap();
    let listener = UnixListener::bind(&fixture.paths.socket).unwrap();
    fs::set_permissions(&fixture.paths.socket, fs::Permissions::from_mode(0o666)).unwrap();
    fixture.clear_logs();
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let listener_task = tokio::spawn(async move {
        if listener.accept().await.is_ok() {
            let _ = accepted_tx.send(());
        }
    });

    let report = inspect_status(context(&fixture)).await;

    assert!(
        timeout(Duration::from_millis(50), accepted_rx)
            .await
            .is_err()
    );
    assert!(
        !render_status(report.clone())
            .stdout
            .contains("- app-server-initialize:")
    );
    listener_task.abort();
}

#[tokio::test]
async fn absent_unit_exit_four_is_trusted_inactive_status_evidence() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    drop(authority);
    for path in [
        &fixture.enabled,
        &fixture.active,
        &fixture.paths.socket,
        &fixture.paths.unit,
    ] {
        fs::remove_file(path).unwrap();
    }
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert_eq!(
        report,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::StoppedAutomaticOff),
            IntegrationState::Unavailable,
            IntegrationState::Unavailable,
            vec![StatusProblem::InstalledStateCouldNotBeVerified],
        )
    );
    assert_eq!(
        fixture.systemctl_log(),
        "--user is-enabled codex-session-control-test-Setup1.service\n\
--user is-active codex-session-control-test-Setup1.service\n"
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_reports_untrustworthy_systemctl_state_as_operational_drift() {
    for operation in ["is-enabled", "is-active"] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        fs::write(
            &fixture.systemctl_fail,
            format!("--user {operation} codex-session-control-test-Setup1.service"),
        )
        .unwrap();
        fixture.clear_logs();

        let report = inspect_status(context(&fixture)).await;

        let problem = if operation == "is-enabled" {
            StatusProblem::ServiceEnablementCouldNotBeVerified
        } else {
            StatusProblem::ServiceActivityCouldNotBeVerified
        };
        assert_eq!(
            report,
            StatusResult::new(
                StatusState::Unhealthy,
                Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
                Some(ServiceSummary::CouldNotVerify),
                IntegrationState::CouldNotVerify,
                IntegrationState::Unavailable,
                vec![problem],
            ),
            "{operation}"
        );
        assert_eq!(
            fixture.systemctl_log(),
            "--user is-enabled codex-session-control-test-Setup1.service\n\
--user is-active codex-session-control-test-Setup1.service\n",
            "{operation}"
        );
        assert_read_only_logs(&fixture);
    }
}

#[tokio::test]
async fn status_inspects_structural_desktop_without_launch_or_mutation() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let launcher = fixture._root.path().join("desktop-launcher");
    let launcher_marker = fixture._root.path().join("desktop-launcher-ran");
    let desktop_entry = fixture
        .paths
        .home
        .join(".local/share/applications/codex-desktop.desktop");
    fs::create_dir_all(desktop_entry.parent().unwrap()).unwrap();
    super::write_executable_fixture(
        &launcher,
        format!(
            "#!/bin/sh\nprintf launched > '{}'\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{{\"appIdentity\":{{\"id\":\"codex-desktop\"}},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}}'; exit 0; fi\nexit 64\n",
            launcher_marker.display()
        ),
    );
    fs::write(
        &desktop_entry,
        format!(
            "[Desktop Entry]\nType=Application\nExec={}\n",
            launcher.display()
        ),
    )
    .unwrap();
    let desktop_entry_before = fs::read(&desktop_entry).unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert!(
        matches!(report.state(), StatusState::Healthy | StatusState::Disabled),
        "{}",
        render_status(report.clone()).stdout
    );
    assert!(
        !launcher_marker.exists(),
        "status must not execute the Desktop launcher"
    );
    assert!(render_status(report).stdout.contains("could not verify\n"));
    assert!(!fixture.paths.home.join(".config/codex-desktop").exists());
    assert_eq!(fs::read(&desktop_entry).unwrap(), desktop_entry_before);
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_uses_persisted_desktop_evidence_without_launch_or_mutation() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let launcher_marker = fixture._root.path().join("desktop-launcher-ran");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        format!(
            "#!/bin/sh\nprintf launched > '{}'\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{{\"appIdentity\":{{\"id\":\"codex-desktop\"}},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}}'; exit 0; fi\nexit 64\n",
            launcher_marker.display()
        ),
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher);
    setup_with_context(setup).await.unwrap();
    fs::remove_file(&launcher_marker).unwrap();
    let descriptor_before = fs::read(&descriptor).unwrap();
    let descriptor_mode = fs::metadata(&descriptor).unwrap().permissions().mode();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert!(
        matches!(report.state(), StatusState::Healthy | StatusState::Disabled),
        "{}",
        render_status(report.clone()).stdout
    );
    assert!(
        !launcher_marker.exists(),
        "status must not execute the persisted Desktop launcher"
    );
    assert_eq!(report.state(), StatusState::Healthy);
    assert!(
        render_status(report)
            .stdout
            .contains("Codex Desktop integration: ready\n")
    );
    assert_eq!(fs::read(&descriptor).unwrap(), descriptor_before);
    assert_eq!(
        fs::metadata(&descriptor).unwrap().permissions().mode(),
        descriptor_mode
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_leaves_unknown_descriptor_unverified_after_null_setup_without_mutation() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let launcher = fixture._root.path().join("desktop-launcher");
    let desktop_entry = fixture
        .paths
        .home
        .join(".local/share/applications/codex-desktop.desktop");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    fs::create_dir_all(desktop_entry.parent().unwrap()).unwrap();
    fs::create_dir_all(descriptor.parent().unwrap()).unwrap();
    fs::set_permissions(
        fixture.paths.home.join(".config"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::set_permissions(
        descriptor.parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    fs::write(
        &desktop_entry,
        format!(
            "[Desktop Entry]\nType=Application\nExec={}\n",
            launcher.display()
        ),
    )
    .unwrap();
    fs::write(
        &descriptor,
        b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
    )
    .unwrap();
    fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o600)).unwrap();
    let descriptor_before = fs::read(&descriptor).unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert!(
        matches!(report.state(), StatusState::Healthy | StatusState::Disabled),
        "{}",
        render_status(report.clone()).stdout
    );
    assert!(render_status(report).stdout.contains("could not verify\n"));
    assert_eq!(fs::read(&descriptor).unwrap(), descriptor_before);
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_reports_descriptor_service_matrix_without_mutation() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher);
    setup_with_context(setup).await.unwrap();
    let expected = fs::read(&descriptor).unwrap();
    fixture.clear_logs();

    let structurally_unverified = inspect_status(context(&fixture)).await;
    assert!(
        matches!(
            structurally_unverified.state(),
            StatusState::Healthy | StatusState::Disabled
        ),
        "{}",
        render_status(structurally_unverified.clone()).stdout
    );
    assert!(
        render_status(structurally_unverified)
            .stdout
            .contains("Codex Desktop integration: ready\n")
    );
    assert_eq!(fs::read(&descriptor).unwrap(), expected);

    fs::remove_file(&descriptor).unwrap();
    let absent_active = inspect_status(context(&fixture)).await;
    assert!(
        !matches!(
            absent_active.state(),
            StatusState::Healthy | StatusState::Disabled
        ),
        "{}",
        render_status(absent_active.clone()).stdout
    );
    assert!(
        render_status(absent_active.clone())
            .stdout
            .contains("Codex Desktop integration: unhealthy\n")
    );
    assert!(
        render_status(absent_active.clone())
            .stdout
            .contains("- Codex Desktop integration is incorrectly configured.")
    );
    assert!(render_status(absent_active).stdout.contains("Problems:\n"));

    drop(authority);
    fs::remove_file(&fixture.enabled).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();
    let absent_inactive = inspect_status(context(&fixture)).await;
    assert!(
        matches!(
            absent_inactive.state(),
            StatusState::Healthy | StatusState::Disabled
        ),
        "{}",
        render_status(absent_inactive.clone()).stdout
    );
    assert!(
        render_status(absent_inactive)
            .stdout
            .contains("unavailable\n")
    );

    fs::write(
        &descriptor,
        b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
    )
    .unwrap();
    let foreign = inspect_status(context(&fixture)).await;
    assert!(!matches!(
        foreign.state(),
        StatusState::Healthy | StatusState::Disabled
    ));
    assert!(
        render_status(foreign.clone())
            .stdout
            .contains("- Codex Desktop integration is incorrectly configured.")
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_keeps_a_missing_persisted_launcher_unverified_and_read_only() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher.clone());
    setup_with_context(setup).await.unwrap();
    let expected = fs::read(&descriptor).unwrap();
    fs::rename(&launcher, launcher.with_extension("missing")).unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert!(
        matches!(report.state(), StatusState::Healthy | StatusState::Disabled),
        "{}",
        render_status(report.clone()).stdout
    );
    assert_eq!(report.state(), StatusState::Healthy);
    assert!(
        render_status(report)
            .stdout
            .contains("Codex Desktop integration: ready\n")
    );
    assert_eq!(fs::read(&descriptor).unwrap(), expected);
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_detects_unsafe_descriptor_when_persisted_launcher_is_unavailable() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher.clone());
    setup_with_context(setup).await.unwrap();
    let descriptor_before = fs::read(&descriptor).unwrap();
    fs::rename(&launcher, launcher.with_extension("missing")).unwrap();
    fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o1600)).unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert_eq!(
        report,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::Ready,
            IntegrationState::Unhealthy,
            vec![StatusProblem::DesktopDescriptorFault],
        )
    );
    assert_eq!(fs::read(&descriptor).unwrap(), descriptor_before);
    assert_eq!(
        fs::metadata(&descriptor).unwrap().permissions().mode() & 0o7777,
        0o1600
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_leaves_descriptor_safe_open_failure_unverified() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher);
    setup_with_context(setup).await.unwrap();
    fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o000)).unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert_eq!(
        report,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::Ready,
            IntegrationState::CouldNotVerify,
            vec![StatusProblem::DesktopCouldNotBeVerified],
        )
    );
    assert_read_only_logs(&fixture);

    fs::remove_file(&fixture.active).unwrap();
    fixture.clear_logs();
    let inactive = inspect_status(context(&fixture)).await;

    assert!(render_status(inactive).stdout.contains("unhealthy\n"));
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_accumulates_every_failure_with_exact_routes_and_advisory() {
    let fixture = Fixture::new();
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    drop(authority);
    fs::remove_file(&fixture.paths.socket).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fs::write(&fixture.paths.binary, b"binary drift").unwrap();
    fs::write(&fixture.paths.config, b"invalid config").unwrap();
    fs::write(
        fixture
            .paths
            .marketplace
            .join("plugins/codex-session-control/.mcp.json"),
        b"projection drift",
    )
    .unwrap();
    fs::write(&fixture.plugin_state, b"/wrong/plugin").unwrap();
    fs::write(&fixture.paths.unit, b"unit drift").unwrap();
    fs::write(
        &fixture.codex_version,
        format!("codex-cli {untested_version}\n"),
    )
    .unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert_eq!(
        report,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::StoppedUnexpectedAutomaticOn),
            IntegrationState::Unhealthy,
            IntegrationState::Unavailable,
            vec![
                StatusProblem::InstalledStateCouldNotBeVerified,
                StatusProblem::ProjectionFault,
                StatusProblem::NativeRegistrationFault,
                StatusProblem::ServiceConfiguredButStopped,
                StatusProblem::SocketMissing,
            ],
        )
    );
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn unsafe_path_drift_names_the_path_and_invariant_without_repair() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let original = fs::read(&fixture.paths.config).unwrap();
    let target = fixture.paths.home.join("unsafe-config-target");
    fs::write(&target, original).unwrap();
    fs::remove_file(&fixture.paths.config).unwrap();
    symlink(&target, &fixture.paths.config).unwrap();
    fixture.clear_logs();

    let report = inspect_status(context(&fixture)).await;

    assert_eq!(
        report,
        StatusResult::new(
            StatusState::Unhealthy,
            Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
            Some(ServiceSummary::RunningAutomatic),
            IntegrationState::Ready,
            IntegrationState::Unavailable,
            vec![StatusProblem::InstalledStateCouldNotBeVerified],
        )
    );
    assert_eq!(fs::read_link(&fixture.paths.config).unwrap(), target);
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_accepts_only_owner_read_write_socket_modes() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    for mode in [0o600, 0o700] {
        fs::set_permissions(&fixture.paths.socket, fs::Permissions::from_mode(mode)).unwrap();
        let report = inspect_status(context(&fixture)).await;
        assert!(
            matches!(report.state(), StatusState::Healthy | StatusState::Disabled),
            "mode {mode:04o}: {}",
            render_status(report.clone()).stdout
        );
    }

    for mode in [0o000, 0o200, 0o400, 0o500, 0o601, 0o660, 0o711, 0o777] {
        fs::set_permissions(&fixture.paths.socket, fs::Permissions::from_mode(mode)).unwrap();
        let report = inspect_status(context(&fixture)).await;
        assert!(
            !matches!(report.state(), StatusState::Healthy | StatusState::Disabled),
            "mode {mode:04o}"
        );
        assert!(
            render_status(report.clone())
                .stdout
                .contains("- The service connection is unsafe."),
            "mode {mode:04o}: {}",
            render_status(report).stdout
        );
    }
}

#[tokio::test]
async fn tested_and_unparseable_versions_preserve_the_typed_disabled_result() {
    for version_output in [TESTED_CODEX_CLI_VERSION_OUTPUT, "codex-cli not-semver\n"] {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        drop(authority);
        fs::remove_file(&fixture.paths.socket).unwrap();
        fs::remove_file(&fixture.enabled).unwrap();
        fs::remove_file(&fixture.active).unwrap();
        fs::write(&fixture.codex_version, version_output).unwrap();
        fixture.clear_logs();

        let report = inspect_status(context(&fixture)).await;

        assert_eq!(
            report,
            StatusResult::new(
                StatusState::Disabled,
                Some(semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()),
                Some(ServiceSummary::StoppedAutomaticOff),
                IntegrationState::Unavailable,
                IntegrationState::Unavailable,
                Vec::new(),
            )
        );
        assert_read_only_logs(&fixture);
    }
}
