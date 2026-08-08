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

        let report = enable_with_context(context(&fixture)).await.unwrap();
        let started_authority = match starter {
            Some(starter) => Some(starter.await.unwrap()),
            None => None,
        };

        assert_eq!(
            report.stdout,
            "Codex app-server service: enabled, active\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Desktop restart required: no\n\
Run codex-session-control setup to attach Desktop.\n",
            "{name}"
        );
        assert_eq!(
            report.stderr, "completed: service-enable\ncompleted: service-verify\n",
            "{name}"
        );
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

        assert_eq!(error.exit_code(), 1, "{operation}");
        assert_eq!(
            error.to_string(),
            format!(
                "completed: service-enable\n\
failed at service-verify: systemctl {operation} could not provide trustworthy service state\n\
retry: codex-session-control enable\n"
            ),
            "{operation}"
        );
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

    let report = enable_with_context(context(&fixture)).await.unwrap();

    assert!(report.stdout.contains("Desktop attachment: unavailable\n"));
    assert!(report.stdout.contains("Desktop restart required: no\n"));
    assert!(
        report
            .stdout
            .contains("Run codex-session-control setup to attach Desktop.\n")
    );
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

    let report = enable_with_context(context(&fixture)).await.unwrap();

    assert_eq!(
        fs::read(&descriptor).unwrap(),
        render_descriptor(&fixture.paths.socket).unwrap(),
    );
    assert!(report.stdout.contains("Desktop attachment: available\n"));
    assert!(report.stdout.contains("Desktop restart required: yes\n"));
    let stages = report.stderr;
    assert!(
        stages.find("completed: descriptor\n").unwrap()
            < stages.find("completed: service-enable\n").unwrap()
    );
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

    assert!(error.to_string().contains("failed at service-enable:"));
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

        assert!(error.to_string().contains("Desktop descriptor"));
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

        assert!(error.to_string().contains(&format!("failed at {drift}:")));
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

    let report = enable_with_context(context(&fixture)).await.unwrap();

    assert_eq!(
        report.stderr,
        format!(
            "completed: service-enable\n\
completed: service-verify\n\
Compatibility warning: Codex app-server {untested_version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
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
            error.to_string().contains(
                "completed: service-disable\n\
completed: service-verify\n\
failed at descriptor-remove: Desktop descriptor cleanup is incomplete:"
            ),
            "{name}: {error}"
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

    assert_eq!(error.exit_code(), 1);
    assert_eq!(
        error.to_string(),
        "failed at service-disable: systemctl command failed\n\
retry: codex-session-control disable\n"
    );
    assert!(fixture.enabled.is_file());
    assert!(fixture.active.is_file());
    assert!(fixture.paths.socket.exists());
}
