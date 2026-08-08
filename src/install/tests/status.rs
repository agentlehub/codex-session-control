use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

use super::support::{FakeAuthority, Fixture};
use super::*;

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
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    }
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

        let report = status_with_context(context(&fixture)).await.unwrap();

        let service = format!(
            "{}, {}",
            if enabled { "enabled" } else { "disabled" },
            if active { "active" } else { "inactive" }
        );
        let healthy = enabled == active && active == socket;
        let expected = if healthy && !enabled {
            format!(
                "Status: healthy\n\
Installed release: {version}\n\
Codex app-server service: {service}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Loaded task state: not_verified\n\
Availability: codex-session-control enable\n",
                version = env!("CARGO_PKG_VERSION")
            )
        } else if healthy {
            format!(
                "Status: healthy\n\
Installed release: {version}\n\
Codex app-server service: {service}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Loaded task state: not_verified\n",
                version = env!("CARGO_PKG_VERSION")
            )
        } else {
            let detail = if enabled {
                "enabled service is not active"
            } else {
                "disabled service is active"
            };
            let mut expected = format!(
                "Status: drifted\n\
Installed release: {version}\n\
Codex app-server service: {service}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Loaded task state: not_verified\n\
Failed checks:\n\
- service-state: {detail}\n\
{indent}action: journalctl --user -u codex-session-control-test-Setup1.service\n",
                indent = "  ",
                version = env!("CARGO_PKG_VERSION")
            );
            if enabled && !socket {
                expected.push_str(concat!(
                    "- socket: enabled service socket is missing\n",
                    "  action: journalctl --user -u codex-session-control-test-Setup1.service\n",
                ));
            }
            expected
        };
        assert_eq!(report.stdout, expected, "{name}");
        assert_eq!(report.healthy, healthy, "{name}");
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

        let report = status_with_context(context(&fixture)).await.unwrap();

        assert!(!report.healthy, "{operation}");
        assert_eq!(
            report.stdout,
            format!(
                "Status: drifted\n\
Installed release: {version}\n\
Codex app-server service: unknown, unknown\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Loaded task state: not_verified\n\
Failed checks:\n\
- service-state: systemctl {operation} could not provide trustworthy service state\n\
{indent}action: journalctl --user -u codex-session-control-test-Setup1.service\n",
                indent = "  ",
                version = env!("CARGO_PKG_VERSION")
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
async fn status_reports_compatible_desktop_as_available_after_setup_without_mutation() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let launcher = fixture._root.path().join("desktop-launcher");
    let desktop_entry = fixture
        .paths
        .home
        .join(".local/share/applications/codex-desktop.desktop");
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

    let report = status_with_context(context(&fixture)).await.unwrap();

    assert!(report.healthy, "{}", report.stdout);
    assert!(
        report
            .stdout
            .contains("Desktop attachment: available after setup\n")
    );
    assert!(!fixture.paths.home.join(".config/codex-desktop").exists());
    assert_read_only_logs(&fixture);
}

#[tokio::test]
async fn status_detects_foreign_discovered_descriptor_after_null_setup_without_mutation() {
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

    let report = status_with_context(context(&fixture)).await.unwrap();

    assert!(!report.healthy);
    assert!(report.stdout.contains("- desktop-descriptor:"));
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

    let available = status_with_context(context(&fixture)).await.unwrap();
    assert!(available.healthy, "{}", available.stdout);
    assert!(available.stdout.contains("Desktop attachment: available\n"));
    assert_eq!(fs::read(&descriptor).unwrap(), expected);

    fs::remove_file(&descriptor).unwrap();
    let absent_active = status_with_context(context(&fixture)).await.unwrap();
    assert!(!absent_active.healthy, "{}", absent_active.stdout);
    assert!(
        absent_active
            .stdout
            .contains("Desktop attachment: unverified\n")
    );
    assert!(absent_active.stdout.contains("- desktop-descriptor:"));
    assert!(
        absent_active
            .stdout
            .contains("action: codex-session-control update")
    );

    drop(authority);
    fs::remove_file(&fixture.enabled).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();
    let absent_inactive = status_with_context(context(&fixture)).await.unwrap();
    assert!(absent_inactive.healthy, "{}", absent_inactive.stdout);
    assert!(
        absent_inactive
            .stdout
            .contains("Desktop attachment: unverified\n")
    );

    fs::write(
        &descriptor,
        b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
    )
    .unwrap();
    let foreign = status_with_context(context(&fixture)).await.unwrap();
    assert!(!foreign.healthy);
    assert!(foreign.stdout.contains("- desktop-descriptor:"));
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

    let report = status_with_context(context(&fixture)).await.unwrap();

    assert!(report.healthy, "{}", report.stdout);
    assert!(report.stdout.contains("Desktop attachment: unverified\n"));
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

    let report = status_with_context(context(&fixture)).await.unwrap();

    assert!(!report.healthy);
    assert!(report.stdout.contains("Desktop attachment: unverified\n"));
    assert!(report.stdout.contains("- desktop-descriptor:"));
    assert_eq!(fs::read(&descriptor).unwrap(), descriptor_before);
    assert_eq!(
        fs::metadata(&descriptor).unwrap().permissions().mode() & 0o7777,
        0o1600
    );
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

    let report = status_with_context(context(&fixture)).await.unwrap();

    assert_eq!(
        report.stdout,
        format!(
            "Compatibility warning: Codex app-server {untested_version} has not been tested with codex-session-control {version}; native results remain authoritative.\n\
Status: drifted\n\
Installed release: {version}\n\
Codex app-server service: enabled, inactive\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Loaded task state: not_verified\n\
Failed checks:\n\
- executable: digest does not match installed manifest\n\
{indent}action: codex-session-control setup\n\
- configuration: invalid installed configuration\n\
{indent}action: codex-session-control setup\n\
- projection: digest does not match installed manifest\n\
{indent}action: codex-session-control setup\n\
- plugin: native registration does not match installed manifest\n\
{indent}action: codex-session-control setup\n\
- service-unit: digest does not match installed manifest\n\
{indent}action: codex-session-control setup\n\
- service-state: enabled service is not active\n\
{indent}action: journalctl --user -u codex-session-control-test-Setup1.service\n\
- socket: enabled service socket is missing\n\
{indent}action: journalctl --user -u codex-session-control-test-Setup1.service\n\
",
            version = env!("CARGO_PKG_VERSION"),
            indent = "  ",
        )
    );
    assert!(!report.healthy);
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

    let report = status_with_context(context(&fixture)).await.unwrap();

    assert_eq!(
        report.stdout,
        format!(
            "Status: drifted\n\
Installed release: {version}\n\
Codex app-server service: enabled, active\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Loaded task state: not_verified\n\
Failed checks:\n\
- configuration: {}: unsafe owner, type, or mode\n\
{indent}action: inspect the path and restore its approved ownership, type, and mode\n",
            fixture.paths.config.display(),
            indent = "  ",
            version = env!("CARGO_PKG_VERSION"),
        )
    );
    assert!(!report.healthy);
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
        let report = status_with_context(context(&fixture)).await.unwrap();
        assert!(report.healthy, "mode {mode:04o}: {}", report.stdout);
    }

    for mode in [0o000, 0o200, 0o400, 0o500, 0o601, 0o660, 0o711, 0o777] {
        fs::set_permissions(&fixture.paths.socket, fs::Permissions::from_mode(mode)).unwrap();
        let report = status_with_context(context(&fixture)).await.unwrap();
        assert!(!report.healthy, "mode {mode:04o}");
        assert!(
                    report.stdout.contains(&format!(
                        "- socket: {}: must be an owner-owned Unix socket with owner read/write permissions and no group/other permissions\n",
                        fixture.paths.socket.display()
                    )),
                    "mode {mode:04o}: {}",
                    report.stdout
                );
    }
}

#[tokio::test]
async fn tested_and_unparseable_versions_have_exact_advisory_behavior() {
    for (version_output, warning) in [
        (TESTED_CODEX_CLI_VERSION_OUTPUT, None),
        ("codex-cli not-semver\n", Some("not-semver")),
    ] {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        drop(authority);
        fs::remove_file(&fixture.paths.socket).unwrap();
        fs::remove_file(&fixture.enabled).unwrap();
        fs::remove_file(&fixture.active).unwrap();
        fs::write(&fixture.codex_version, version_output).unwrap();
        fixture.clear_logs();

        let report = status_with_context(context(&fixture)).await.unwrap();

        let expected_prefix = warning.map(|version| {
                    format!(
                        "Compatibility warning: Codex app-server {version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
                        env!("CARGO_PKG_VERSION")
                    )
                });
        assert_eq!(
            report.stdout.starts_with("Compatibility warning:"),
            warning.is_some()
        );
        if let Some(prefix) = expected_prefix {
            assert!(report.stdout.starts_with(&prefix));
        }
        assert!(report.healthy);
        assert!(report.stdout.ends_with(
            "Loaded task state: not_verified\n\
Availability: codex-session-control enable\n"
        ));
        assert_read_only_logs(&fixture);
    }
}
