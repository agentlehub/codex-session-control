use std::fs;

use super::support::{FakeAuthority, Fixture};
use super::*;

const SERVICE_STATES: [(&str, bool, bool, bool); 4] = [
    ("enabled_active_socket", true, true, true),
    ("enabled_inactive_no_socket", true, false, false),
    ("disabled_inactive_no_socket", false, false, false),
    ("disabled_active_socket", false, true, true),
];

fn context(fixture: &Fixture) -> LifecycleContext {
    let setup = fixture.context(true);
    LifecycleContext {
        target: setup.target,
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    }
}

#[test]
fn enable_guidance_precedence_is_exact() {
    use crate::cli_output::{DesktopAvailability, EnableSuccess, RunningClientFacts, UserSuccess};

    let cases = [
        (
            RunningClientFacts {
                cli: true,
                desktop: false,
            },
            DesktopAvailability::SetupRequired,
            false,
            "Codex CLI is already running without Codex Session Control.",
        ),
        (
            RunningClientFacts {
                cli: false,
                desktop: true,
            },
            DesktopAvailability::Available,
            false,
            "Codex Desktop is already running without Codex Session Control.",
        ),
        (
            RunningClientFacts {
                cli: true,
                desktop: true,
            },
            DesktopAvailability::Available,
            true,
            "Codex Desktop is already running without Codex Session Control.",
        ),
        (
            RunningClientFacts::default(),
            DesktopAvailability::Available,
            true,
            "If Codex Desktop is already running, restart it",
        ),
    ];
    for (running, desktop, changed, expected) in cases {
        let rendered =
            UserSuccess::Enable(EnableSuccess::new(running, desktop, changed, Vec::new()).unwrap())
                .render();
        assert!(rendered.stdout.contains(expected));
        assert_eq!(
            rendered
                .stdout
                .matches("Codex CLI is already running without Codex Session Control.")
                .count(),
            usize::from(running.cli)
        );
        assert_eq!(
            rendered
                .stdout
                .matches("Codex Desktop is already running without Codex Session Control.")
                .count(),
            usize::from(running.desktop)
        );
    }

    for desktop in [
        DesktopAvailability::Unavailable,
        DesktopAvailability::CouldNotVerify,
        DesktopAvailability::SetupRequired,
    ] {
        assert!(
            EnableSuccess::new(RunningClientFacts::default(), desktop, true, Vec::new()).is_none()
        );
    }
}

#[test]
fn enable_disable_pure_failure_mappings_are_exact() {
    use crate::{
        cli_output::{OrdinaryFailure, StopThenRetry, UserFailure},
        desktop::{DescriptorPublicationFailure, DescriptorPublicationResidue},
    };

    assert_eq!(
        enable_context_failure(),
        UserFailure::Ordinary(OrdinaryFailure::EnableUnexpectedRetry)
    );
    assert_eq!(
        disable_context_failure(),
        UserFailure::Ordinary(OrdinaryFailure::DisableUnexpectedRetry)
    );

    assert_eq!(
        enable_publication_failure(DescriptorPublicationFailure {
            source: ControllerError::Operational("sentinel".to_owned()),
            residue: None,
        }),
        UserFailure::Ordinary(OrdinaryFailure::EnableDesktopIntegrationRetry)
    );
    for residue in [
        DescriptorPublicationResidue::Stage(PathBuf::from("/managed/stage")),
        DescriptorPublicationResidue::Final(PathBuf::from("/managed/final")),
    ] {
        assert!(matches!(
            enable_publication_failure(DescriptorPublicationFailure {
                source: ControllerError::Operational("sentinel".to_owned()),
                residue: Some(residue),
            }),
            UserFailure::RollbackIncomplete(_)
        ));
    }
    assert_eq!(
        enable_service_failure(
            StopThenRetry::EnableServiceStartStopThenEnable,
            false,
            Ok(()),
        ),
        UserFailure::StopThenRetry(StopThenRetry::EnableServiceStartStopThenEnable)
    );
    assert_eq!(
        enable_service_failure(
            StopThenRetry::EnableServiceStateStopThenEnable,
            true,
            Ok(()),
        ),
        UserFailure::Ordinary(OrdinaryFailure::EnableServiceStateRetry)
    );
    assert!(matches!(
        enable_service_failure(
            StopThenRetry::EnableServiceStateStopThenEnable,
            true,
            Err(DescriptorPublicationFailure {
                source: ControllerError::Operational("sentinel".to_owned()),
                residue: Some(DescriptorPublicationResidue::Final(PathBuf::from(
                    "/managed/final",
                ))),
            }),
        ),
        UserFailure::RollbackIncomplete(_)
    ));
}

#[tokio::test]
async fn enable_default_and_verbose_are_behaviorally_identical() {
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
    let mut off = Diagnostics::new(false, DiagnosticCommand::Enable);
    let mut verbose = Diagnostics::record(DiagnosticCommand::Enable);

    let default = enable_with_context_and_diagnostics(context(&default_fixture), &mut off)
        .await
        .unwrap()
        .render();
    let recorded = enable_with_context_and_diagnostics(context(&verbose_fixture), &mut verbose)
        .await
        .unwrap()
        .render();

    let recorded = with_recorded_diagnostics(recorded, &verbose);

    assert_default_verbose_parity(&default, &recorded, &verbose, "[verbose] enable:");
    assert_eq!(
        default_fixture.systemctl_log(),
        verbose_fixture.systemctl_log()
    );
    assert_eq!(
        default_fixture.enabled.exists(),
        verbose_fixture.enabled.exists()
    );
    assert_eq!(
        default_fixture.active.exists(),
        verbose_fixture.active.exists()
    );
}

#[tokio::test]
async fn disable_default_and_verbose_are_behaviorally_identical() {
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
    let mut off = Diagnostics::new(false, DiagnosticCommand::Disable);
    let mut verbose = Diagnostics::record(DiagnosticCommand::Disable);

    let default = disable_with_context_and_diagnostics(context(&default_fixture), &mut off)
        .await
        .unwrap()
        .render();
    let recorded = disable_with_context_and_diagnostics(context(&verbose_fixture), &mut verbose)
        .await
        .unwrap()
        .render();

    let recorded = with_recorded_diagnostics(recorded, &verbose);

    assert_default_verbose_parity(&default, &recorded, &verbose, "[verbose] disable:");
    assert_eq!(
        default_fixture.systemctl_log(),
        verbose_fixture.systemctl_log()
    );
    assert_eq!(
        default_fixture.enabled.exists(),
        verbose_fixture.enabled.exists()
    );
    assert_eq!(
        default_fixture.active.exists(),
        verbose_fixture.active.exists()
    );
}

fn apply_state(
    fixture: &Fixture,
    authority: FakeAuthority,
    enabled: bool,
    active: bool,
    socket: bool,
) -> Option<FakeAuthority> {
    if !enabled {
        fs::remove_file(&fixture.enabled).unwrap();
    }
    if !active {
        fs::remove_file(&fixture.active).unwrap();
    }
    if socket {
        Some(authority)
    } else {
        drop(authority);
        fs::remove_file(&fixture.paths.socket).unwrap();
        None
    }
}

#[tokio::test(start_paused = true)]
async fn enable_converges_every_service_state_with_exact_receipt_and_argv() {
    for (name, enabled, active, socket) in SERVICE_STATES {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let authority = apply_state(&fixture, authority, enabled, active, socket);
        fixture.clear_logs();

        let starter = if socket {
            None
        } else {
            let paths = fixture.paths.clone();
            let active = fixture.active.clone();
            Some(tokio::spawn(async move {
                while !active.exists() {
                    tokio::task::yield_now().await;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
            }))
        };

        let report = enable_with_context(context(&fixture))
            .await
            .unwrap()
            .render();
        let started_authority = match starter {
            Some(starter) => Some(starter.await.unwrap()),
            None => None,
        };

        assert_eq!(
            report.stdout,
            "Codex Session Control is running and will start automatically.\n\n\
Codex Desktop integration is unavailable.\n\
Run `codex-session-control setup` to set it up.\n",
            "{name}"
        );
        assert!(report.stderr.is_empty(), "{name}");
        assert!(fixture.enabled.is_file(), "{name}");
        assert!(fixture.active.is_file(), "{name}");
        assert!(fixture.paths.socket.exists(), "{name}");
        assert_eq!(
            fixture.systemctl_log(),
            " --user enable --now codex-session-control-test-Setup1.service\n"
                .trim_start()
                .to_owned()
                + "--user is-enabled codex-session-control-test-Setup1.service\n"
                + "--user is-active codex-session-control-test-Setup1.service\n",
            "{name}"
        );
        let codex = fixture.codex_log();
        assert!(codex.lines().all(|line| line.starts_with("--version|")));
        drop(started_authority);
        drop(authority);
    }
}

#[tokio::test]
async fn enable_verification_reports_untrustworthy_systemctl_state_as_operational_failure() {
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

        let error = enable_with_context(context(&fixture)).await.unwrap_err();

        assert_eq!(error.render().exit_code, 1, "{operation}");
        assert!(matches!(
            error,
            crate::cli_output::UserFailure::StopThenRetry(
                crate::cli_output::StopThenRetry::EnableServiceStateStopThenEnable
            )
        ));
        let active_query = if operation == "is-active" {
            "--user is-active codex-session-control-test-Setup1.service\n"
        } else {
            ""
        };
        assert_eq!(
            fixture.systemctl_log(),
            format!(
                "--user enable --now codex-session-control-test-Setup1.service\n\
--user is-enabled codex-session-control-test-Setup1.service\n{active_query}"
            ),
            "{operation}"
        );
    }
}

#[tokio::test]
async fn enable_with_null_attachment_does_not_auto_select_desktop() {
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
    fixture.clear_logs();

    let report = enable_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert!(
        report
            .stdout
            .contains("Codex Desktop integration is unavailable.")
    );
    assert!(report.stdout.contains("Run `codex-session-control setup`"));
    assert!(!descriptor.exists());
    assert!(
        fixture
            .codex_log()
            .lines()
            .all(|line| line.starts_with("--version|"))
    );
}

#[tokio::test]
async fn enable_publishes_a_verified_persisted_descriptor_before_service_enable() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    manifest["desktopAttachment"] = serde_json::json!({
        "launcherPath": launcher,
        "appId": "codex-desktop",
        "descriptorPath": descriptor,
    });
    fs::write(
        &fixture.paths.manifest,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(
        &fixture.required_descriptor,
        descriptor.display().to_string(),
    )
    .unwrap();
    fixture.clear_logs();

    let report = enable_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert_eq!(
        fs::read(&descriptor).unwrap(),
        render_descriptor(&fixture.paths.socket).unwrap(),
    );
    assert!(report.stdout.contains(
        "If Codex Desktop is already running, restart it to make Codex Session Control available there."
    ));
}

#[tokio::test]
async fn enable_start_failure_preserves_service_state_and_cleans_a_published_descriptor_when_inactive_absent()
 {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    manifest["desktopAttachment"] = serde_json::json!({
        "launcherPath": launcher,
        "appId": "codex-desktop",
        "descriptorPath": descriptor,
    });
    fs::write(
        &fixture.paths.manifest,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    drop(authority);
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();
    fs::write(
        &fixture.systemctl_fail,
        "--user enable --now codex-session-control-test-Setup1.service",
    )
    .unwrap();
    fixture.clear_logs();

    let error = enable_with_context(context(&fixture)).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::EnableServiceStartRetry
        )
    ));
    assert!(fixture.enabled.exists());
    assert!(!fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!descriptor.exists());
    assert!(!fixture.systemctl_log().contains("disable --now"));
}

#[tokio::test]
async fn enable_inspects_a_known_descriptor_before_service_mutation_when_launcher_is_unavailable() {
    for unsafe_descriptor in [false, true] {
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
        fs::rename(&launcher, launcher.with_extension("missing")).unwrap();
        if unsafe_descriptor {
            fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o644)).unwrap();
        } else {
            fs::write(
                &descriptor,
                b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
            )
            .unwrap();
            fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fixture.clear_logs();

        let error = enable_with_context(context(&fixture)).await.unwrap_err();

        assert!(matches!(
            error,
            crate::cli_output::UserFailure::Ordinary(
                crate::cli_output::OrdinaryFailure::EnableDesktopIntegrationCheckStatus
            )
        ));
        assert!(fixture.systemctl_log().is_empty());
    }
}

#[tokio::test]
async fn enable_rejects_config_or_unit_drift_before_service_mutation() {
    for drift in ["configuration", "service-unit"] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        if drift == "configuration" {
            fs::write(&fixture.paths.config, b"invalid").unwrap();
        } else {
            fs::write(&fixture.paths.unit, b"drift").unwrap();
        }
        fixture.clear_logs();

        let error = enable_with_context(context(&fixture)).await.unwrap_err();

        assert!(matches!(
            (drift, error),
            (
                "configuration",
                crate::cli_output::UserFailure::Ordinary(
                    crate::cli_output::OrdinaryFailure::EnableInstalledStateRepairSetup
                )
            ) | (
                "service-unit",
                crate::cli_output::UserFailure::Ordinary(
                    crate::cli_output::OrdinaryFailure::EnableServiceConfigurationRepairSetup
                )
            )
        ));
        assert!(fixture.systemctl_log().is_empty());
    }
}

#[tokio::test]
async fn enable_prints_only_the_approved_compatibility_advisory() {
    let fixture = Fixture::new();
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    fs::write(
        &fixture.codex_version,
        format!("codex-cli {untested_version}\n"),
    )
    .unwrap();
    let _authority = FakeAuthority::start(&fixture.paths, &untested_version).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fixture.clear_logs();

    let report = enable_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert_eq!(
        report.stderr,
        format!(
            "Warning: Codex {untested_version} has not been tested with Codex Session Control {}.\n\
Some features may not work as expected.\n",
            env!("CARGO_PKG_VERSION"),
        )
    );
}

#[tokio::test]
async fn disable_without_install_metadata_stops_safely_and_reports_incomplete_descriptor_cleanup() {
    for (name, enabled, active, socket) in SERVICE_STATES {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let authority = apply_state(&fixture, authority, enabled, active, socket);
        for path in [
            fixture.paths.binary.clone(),
            fixture.paths.config.clone(),
            fixture.paths.manifest.clone(),
        ] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir_all(&fixture.paths.marketplace).unwrap();
        fixture.clear_logs();

        let error = disable_with_context(context(&fixture)).await.unwrap_err();

        assert!(
            matches!(error, crate::cli_output::UserFailure::PartialDisable(_)),
            "{name}: {error:?}"
        );
        assert!(!fixture.enabled.exists(), "{name}");
        assert!(!fixture.active.exists(), "{name}");
        assert!(!fixture.paths.socket.exists(), "{name}");
        let preflight = if active {
            "--user is-active codex-session-control-test-Setup1.service\n--user whoami\n"
        } else {
            "--user is-active codex-session-control-test-Setup1.service\n"
        };
        assert_eq!(
            fixture.systemctl_log(),
            preflight.to_owned()
                + "--user disable --now codex-session-control-test-Setup1.service\n"
                + "--user is-enabled codex-session-control-test-Setup1.service\n"
                + "--user is-active codex-session-control-test-Setup1.service\n",
            "{name}"
        );
        assert!(fixture.codex_log().is_empty(), "{name}");
        assert!(fixture.paths.codex_home.is_dir());
        drop(authority);
    }
}

#[tokio::test]
async fn lifecycle_stage_failure_has_exact_exit_one_error_and_no_false_receipt() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fs::write(
        &fixture.systemctl_fail,
        "--user disable --now codex-session-control-test-Setup1.service",
    )
    .unwrap();
    fixture.clear_logs();

    let error = disable_with_context(context(&fixture)).await.unwrap_err();

    assert_eq!(error.render().exit_code, 1);
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::StopThenRetry(
            crate::cli_output::StopThenRetry::DisableServiceStopThenDisable
        )
    ));
    assert!(fixture.enabled.is_file());
    assert!(fixture.active.is_file());
    assert!(fixture.paths.socket.exists());
}
