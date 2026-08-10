use std::{fs, os::unix::fs::PermissionsExt};

use super::support::{FakeAuthority, Fixture};
use super::*;

fn write_compatible_launcher(path: &Path) {
    write_compatible_launcher_with_app_id(path, "codex-desktop");
}

fn write_compatible_launcher_with_app_id(path: &Path, app_id: &str) {
    super::write_executable_fixture(
        path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{{\"appIdentity\":{{\"id\":\"{app_id}\"}},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}}'; exit 0; fi\nexit 64\n"
        ),
    );
}

fn write_incompatible_launcher(path: &Path) {
    super::write_executable_fixture(
        path,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[]}'; exit 0; fi\nexit 64\n",
    );
}

#[tokio::test]
async fn publishes_descriptor_before_service_enable() {
    let fixture = Fixture::new();
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
    write_compatible_launcher(&launcher);
    fs::write(
        &desktop_entry,
        format!(
            "[Desktop Entry]\nType=Application\nExec={}\n",
            launcher.display()
        ),
    )
    .unwrap();
    fs::write(
        &fixture.required_descriptor,
        descriptor.display().to_string(),
    )
    .unwrap();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;

    let report = setup_with_context(fixture.context(true))
        .await
        .unwrap()
        .render();

    assert_eq!(
        fs::read(&descriptor).unwrap(),
        crate::desktop::render_descriptor(&fixture.paths.socket).unwrap(),
    );
    assert!(report.stdout.contains(
        "If Codex Desktop is already running, restart it to make Codex Session Control available there."
    ));
    assert!(
        fixture
            .systemctl_log()
            .contains("--user enable --now codex-session-control-test-Setup1.service")
    );
}

#[tokio::test]
async fn unavailable_explicit_launcher_keeps_cli_mcp_setup_and_null_attachment() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_incompatible_launcher(&launcher);
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);

    let report = setup_with_context(context).await.unwrap().render();
    let manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();

    assert_eq!(manifest.desktop_attachment, None);
    assert!(!descriptor.exists());
    assert!(!report.stdout.contains("Codex Desktop"));
    assert!(
        report
            .stderr
            .contains("Codex Desktop integration is unavailable")
    );
    assert!(
        fixture
            .systemctl_log()
            .contains("--user enable --now codex-session-control-test-Setup1.service")
    );
}

#[tokio::test]
async fn missing_absolute_explicit_launcher_keeps_cli_mcp_setup_and_null_attachment() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("missing-desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);

    let report = setup_with_context(context).await.unwrap().render();
    assert!(!descriptor.exists());
    assert!(!report.stdout.contains("Codex Desktop"));
    assert!(
        report
            .stderr
            .contains("Codex Desktop integration is unavailable")
    );
    assert!(
        fixture
            .systemctl_log()
            .contains("--user enable --now codex-session-control-test-Setup1.service")
    );
}

#[tokio::test]
async fn setup_records_desktop_discovery_and_noop_descriptor_after_plugin_install() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;

    let mut diagnostics =
        crate::diagnostics::Diagnostics::record(crate::diagnostics::DiagnosticCommand::Setup);
    setup_with_context_and_diagnostics(fixture.context(true), &mut diagnostics)
        .await
        .unwrap();
    let stages: Vec<&str> = diagnostics
        .recorded_lines()
        .iter()
        .filter_map(|line| {
            line.strip_prefix("[verbose] setup: completed ")
                .map(str::trim_end)
        })
        .collect();

    assert_eq!(
        stages,
        [
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
        ]
    );
}

#[tokio::test]
async fn setup_failures_before_descriptor_leave_existing_routing_intent_and_manifest_unchanged() {
    for stage in [
        "binary",
        "configuration",
        "projection",
        "plugin-marketplace",
        "plugin-install",
    ] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        let original_launcher = fixture._root.path().join("desktop-launcher");
        let replacement_launcher = fixture._root.path().join("replacement-launcher");
        let original_descriptor = fixture
            .paths
            .home
            .join(".config/codex-desktop/app-server-attachment.json");
        let replacement_descriptor = fixture
            .paths
            .home
            .join(".config/codex-desktop-replacement/app-server-attachment.json");
        write_compatible_launcher(&original_launcher);
        write_compatible_launcher_with_app_id(&replacement_launcher, "codex-desktop-replacement");
        let mut initial = fixture.context(true);
        initial.desktop_launcher = Some(original_launcher);
        setup_with_context(initial).await.unwrap();
        let manifest = fs::read(&fixture.paths.manifest).unwrap();
        let descriptor = fs::read(&original_descriptor).unwrap();
        fixture.clear_logs();
        let mut replacement = fixture.context(true);
        replacement.desktop_launcher = Some(replacement_launcher);
        replacement.target = replacement.target.fail_after_completed_stage(stage);

        let error = setup_with_context(replacement).await.unwrap_err();

        assert!(matches!(
            error,
            crate::cli_output::UserFailure::Ordinary(
                crate::cli_output::OrdinaryFailure::SetupUnexpectedRetry
            )
        ));
        assert_eq!(
            fs::read(&fixture.paths.manifest).unwrap(),
            manifest,
            "{stage}"
        );
        assert!(original_descriptor.exists(), "{stage}");
        assert_eq!(
            fs::read(&original_descriptor).unwrap(),
            descriptor,
            "{stage}"
        );
        assert!(!replacement_descriptor.exists(), "{stage}");
        assert!(fixture.systemctl_log().is_empty(), "{stage}");
    }
}

#[tokio::test]
async fn failed_reverification_or_explicit_replacement_preserves_persisted_intent() {
    for explicit_replacement in [false, true] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        let launcher = fixture._root.path().join("desktop-launcher");
        let descriptor = fixture
            .paths
            .home
            .join(".config/codex-desktop/app-server-attachment.json");
        write_compatible_launcher(&launcher);
        let mut initial = fixture.context(true);
        initial.desktop_launcher = Some(launcher.clone());
        setup_with_context(initial).await.unwrap();
        let original_manifest = fs::read(&fixture.paths.manifest).unwrap();
        let original_descriptor = fs::read(&descriptor).unwrap();
        let replacement = fixture._root.path().join("replacement-launcher");
        write_incompatible_launcher(if explicit_replacement {
            &replacement
        } else {
            &launcher
        });
        fixture.clear_logs();
        let mut retry = fixture.context(true);
        if explicit_replacement {
            retry.desktop_launcher = Some(replacement);
        }

        let report = setup_with_context(retry).await.unwrap().render();

        assert_eq!(
            fs::read(&fixture.paths.manifest).unwrap(),
            original_manifest
        );
        assert_eq!(fs::read(&descriptor).unwrap(), original_descriptor);
        assert!(!report.stdout.contains("Codex Desktop"));
        assert!(
            report
                .stderr
                .contains("Codex Desktop integration is unavailable")
        );
        assert!(
            fixture
                .systemctl_log()
                .contains("--user enable --now codex-session-control-test-Setup1.service")
        );
    }
}

#[tokio::test]
async fn successful_explicit_replacement_publishes_new_before_removing_old_intent() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let original_launcher = fixture._root.path().join("desktop-launcher");
    let replacement_launcher = fixture._root.path().join("replacement-launcher");
    let original_descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    let replacement_descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop-replacement/app-server-attachment.json");
    write_compatible_launcher(&original_launcher);
    write_compatible_launcher_with_app_id(&replacement_launcher, "codex-desktop-replacement");
    let mut initial = fixture.context(true);
    initial.desktop_launcher = Some(original_launcher);
    setup_with_context(initial).await.unwrap();
    fs::write(
        &fixture.required_descriptor,
        replacement_descriptor.display().to_string(),
    )
    .unwrap();
    fixture.clear_logs();
    let mut replacement = fixture.context(true);
    replacement.desktop_launcher = Some(replacement_launcher.clone());

    let report = setup_with_context(replacement).await.unwrap().render();
    let manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();

    assert!(!original_descriptor.exists());
    assert_eq!(
        fs::read(&replacement_descriptor).unwrap(),
        render_descriptor(&fixture.paths.socket).unwrap(),
    );
    assert_eq!(
        manifest.desktop_attachment.unwrap().launcher_path,
        replacement_launcher
    );
    assert!(report.stdout.contains(
        "If Codex Desktop is already running, restart it to make Codex Session Control available there."
    ));
    assert!(
        fixture
            .systemctl_log()
            .contains("--user enable --now codex-session-control-test-Setup1.service")
    );
}

#[tokio::test]
async fn same_descriptor_path_replacement_keeps_live_routing_and_requires_no_restart() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let first_launcher = fixture._root.path().join("first-launcher");
    let second_launcher = fixture._root.path().join("second-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&first_launcher);
    write_compatible_launcher(&second_launcher);
    let mut initial = fixture.context(true);
    initial.desktop_launcher = Some(first_launcher);
    setup_with_context(initial).await.unwrap();
    let expected = fs::read(&descriptor).unwrap();
    fixture.clear_logs();

    let mut replacement = fixture.context(true);
    replacement.desktop_launcher = Some(second_launcher.clone());
    let report = setup_with_context(replacement).await.unwrap().render();
    let manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();

    assert_eq!(fs::read(&descriptor).unwrap(), expected);
    assert_eq!(
        manifest.desktop_attachment.unwrap().launcher_path,
        second_launcher
    );
    assert!(!report.stdout.contains("restart Codex Desktop"));
}

#[tokio::test]
async fn replacement_old_descriptor_removal_race_cleans_new_publication() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let original_launcher = fixture._root.path().join("desktop-launcher");
    let replacement_launcher = fixture._root.path().join("replacement-launcher");
    let original_descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    let replacement_descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop-replacement/app-server-attachment.json");
    write_compatible_launcher(&original_launcher);
    write_compatible_launcher_with_app_id(&replacement_launcher, "codex-desktop-replacement");
    let mut initial = fixture.context(true);
    initial.desktop_launcher = Some(original_launcher);
    setup_with_context(initial).await.unwrap();
    let manifest = fs::read(&fixture.paths.manifest).unwrap();
    let original = fs::read(&original_descriptor).unwrap();
    fixture.clear_logs();
    let mut replacement = fixture.context(true);
    replacement.desktop_launcher = Some(replacement_launcher);
    replacement.target = replacement.target.force_old_descriptor_removal_race();

    let error = setup_with_context(replacement).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert_eq!(fs::read(&fixture.paths.manifest).unwrap(), manifest);
    assert!(!replacement_descriptor.exists());
    assert_ne!(fs::read(&original_descriptor).unwrap(), original);
    assert!(fixture.systemctl_log().is_empty());
}

#[tokio::test]
async fn foreign_descriptor_stops_before_service_mutation() {
    let fixture = Fixture::new();
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&launcher);
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
    fs::write(
        &descriptor,
        b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
    )
    .unwrap();
    fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o600)).unwrap();
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);

    let error = setup_with_context(context).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupDesktopIntegrationCheckStatus
        )
    ));
    assert!(fixture.systemctl_log().is_empty());
}

#[tokio::test]
async fn unsafe_descriptor_stops_before_service_mutation() {
    let fixture = Fixture::new();
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&launcher);
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
    fs::write(
        &descriptor,
        render_descriptor(&fixture.paths.socket).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o644)).unwrap();
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);

    let error = setup_with_context(context).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupDesktopIntegrationCheckStatus
        )
    ));
    assert!(fixture.systemctl_log().is_empty());
}

#[tokio::test]
async fn inactive_absent_post_publication_failure_removes_only_invocation_changed_descriptor() {
    for failure in ["service-unit", "daemon-reload"] {
        let fixture = Fixture::new();
        let launcher = fixture._root.path().join("desktop-launcher");
        let descriptor = fixture
            .paths
            .home
            .join(".config/codex-desktop/app-server-attachment.json");
        write_compatible_launcher(&launcher);
        let mut context = fixture.context(true);
        context.desktop_launcher = Some(launcher);
        if failure == "service-unit" {
            context.target = context.target.fail_service_unit_write();
        } else {
            fs::write(&fixture.systemctl_fail, "--user daemon-reload").unwrap();
        }

        let error = setup_with_context(context).await.unwrap_err();

        assert!(matches!(
            error,
            crate::cli_output::UserFailure::Ordinary(
                crate::cli_output::OrdinaryFailure::SetupServiceConfigurationRetry
            )
        ));
        assert!(!descriptor.exists(), "{failure}");
        assert!(!fixture.active.exists(), "{failure}");
        assert!(!fixture.paths.socket.exists(), "{failure}");
        assert!(
            fixture
                .systemctl_log()
                .contains("--user is-active codex-session-control-test-Setup1.service"),
            "{failure}"
        );
        assert!(
            !fixture.systemctl_log().contains("disable --now"),
            "{failure}"
        );
    }
}

#[tokio::test]
async fn unproven_service_activity_retains_a_changed_descriptor_without_stop() {
    let fixture = Fixture::new();
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&launcher);
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);
    context.target = context.target.fail_service_unit_write();
    fs::write(
        &fixture.systemctl_fail,
        "--user is-active codex-session-control-test-Setup1.service",
    )
    .unwrap();

    let error = setup_with_context(context).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert!(descriptor.exists());
    assert!(!fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!fixture.systemctl_log().contains("disable --now"));
}

#[tokio::test]
async fn preexisting_running_authority_and_unchanged_descriptor_are_never_stopped() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&launcher);
    let mut initial = fixture.context(true);
    initial.desktop_launcher = Some(launcher.clone());
    setup_with_context(initial).await.unwrap();
    let expected = fs::read(&descriptor).unwrap();
    fixture.clear_logs();

    let mut retry = fixture.context(true);
    retry.desktop_launcher = Some(launcher);
    retry.target = retry.target.fail_service_unit_write();

    let error = setup_with_context(retry).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupServiceConfigurationRetry
        )
    ));
    assert_eq!(fs::read(&descriptor).unwrap(), expected);
    assert!(fixture.enabled.exists());
    assert!(fixture.active.exists());
    assert!(fixture.paths.socket.exists());
    assert!(!fixture.systemctl_log().contains("disable --now"));
}

#[tokio::test]
async fn newly_active_authority_after_failed_start_retains_changed_descriptor_without_stop() {
    let fixture = Fixture::new();
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&launcher);
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);

    let error = setup_with_context(context).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::RollbackIncomplete(_)
    ));
    assert!(descriptor.exists());
    assert!(fixture.enabled.exists());
    assert!(fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!fixture.systemctl_log().contains("disable --now"));
}

#[tokio::test]
async fn preexisting_exact_descriptor_is_retained_when_this_invocation_does_not_change_intent() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    let descriptor = fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json");
    write_compatible_launcher(&launcher);
    let mut initial = fixture.context(true);
    initial.desktop_launcher = Some(launcher.clone());
    setup_with_context(initial).await.unwrap();
    let expected = fs::read(&descriptor).unwrap();
    drop(authority);
    fs::remove_file(&fixture.enabled).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();
    fs::write(
        &fixture.systemctl_fail,
        "--user enable --now codex-session-control-test-Setup1.service",
    )
    .unwrap();
    fixture.clear_logs();
    let mut retry = fixture.context(true);
    retry.desktop_launcher = Some(launcher);

    let error = setup_with_context(retry).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupServiceStartRetry
        )
    ));
    assert_eq!(fs::read(&descriptor).unwrap(), expected);
    assert!(!fixture.enabled.exists());
    assert!(!fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!fixture.systemctl_log().contains("disable --now"));
}

#[test]
fn classifies_only_ordinary_desktop_and_cli_cmdlines() {
    assert_eq!(
        detect_running_unattached_clients_from_snapshot(
            1000,
            [(1000, b"/opt/Codex\0--foo\0".as_slice())]
        ),
        RunningClientFacts {
            cli: false,
            desktop: true,
        }
    );
    assert_eq!(
        detect_running_unattached_clients_from_snapshot(
            1000,
            [(1000, b"/usr/bin/codex\0resume\0".as_slice())]
        ),
        RunningClientFacts {
            cli: true,
            desktop: false,
        }
    );
    assert_eq!(
        detect_running_unattached_clients_from_snapshot(
            1000,
            [
                (
                    1000,
                    b"/usr/bin/codex\0app-server\0--listen\0unix:///socket\0".as_slice(),
                ),
                (
                    1000,
                    b"/usr/bin/codex\0--remote\0unix:///socket\0resume\0".as_slice(),
                ),
                (1000, b"/usr/bin/codex-session-control\0".as_slice()),
                (1000, b"/usr/bin/sh\0".as_slice()),
            ]
        ),
        RunningClientFacts::default()
    );
}

#[test]
fn process_snapshot_filters_uid_and_never_reads_the_operator_proc_tree() {
    let euid = rustix::process::geteuid().as_raw();
    let clients = detect_running_unattached_clients_from_snapshot(
        euid,
        [
            (euid, b"/usr/bin/codex\0resume\0".as_slice()),
            (euid, b"/usr/bin/codex\0app-server\0".as_slice()),
            (
                euid,
                b"/usr/bin/codex\0--remote=unix:///socket\0".as_slice(),
            ),
            (euid.saturating_add(1), b"/opt/Codex\0".as_slice()),
        ],
    );

    assert_eq!(
        clients,
        RunningClientFacts {
            cli: true,
            desktop: false,
        }
    );
}

#[test]
fn running_client_facts_are_independent_of_desktop_availability() {
    let euid = rustix::process::geteuid().as_raw();
    let facts = detect_running_unattached_clients_from_snapshot(
        euid,
        [(euid, b"/usr/bin/codex\0resume\0".as_slice())],
    );

    assert_eq!(
        facts,
        RunningClientFacts {
            cli: true,
            desktop: false,
        }
    );
}

#[tokio::test]
async fn lifecycle_uses_the_injected_process_snapshot_not_the_live_proc_tree() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    write_compatible_launcher(&launcher);
    let euid = fixture.paths.euid;
    let mut context = fixture.context(true);
    context.desktop_launcher = Some(launcher);
    context.target = context.target.with_client_process_snapshot(vec![
        (euid, b"/opt/Codex\0".to_vec()),
        (euid, b"/usr/bin/codex\0resume\0".to_vec()),
        (euid, b"/usr/bin/codex\0app-server\0".to_vec()),
        (euid, b"/usr/bin/codex\0--remote\0unix:///socket\0".to_vec()),
        (euid.saturating_add(1), b"/opt/Codex\0".to_vec()),
    ]);

    let report = setup_with_context(context).await.unwrap().render();

    assert!(
        report
            .stdout
            .contains("Codex CLI is already running without Codex Session Control.")
    );
    assert!(report.stdout.contains("  codex-session-control codex"));
    assert!(report.stdout.contains(
        "Codex Desktop is already running without Codex Session Control.\n\
Restart Codex Desktop to use Codex Session Control there."
    ));
}
