use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

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

fn descriptor(fixture: &Fixture) -> PathBuf {
    fixture
        .paths
        .home
        .join(".config/codex-desktop/app-server-attachment.json")
}

async fn setup_attached(fixture: &Fixture) -> FakeAuthority {
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    let launcher = fixture._root.path().join("desktop-launcher");
    super::write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher);
    setup_with_context(setup).await.unwrap();
    assert!(descriptor(fixture).is_file());
    authority
}

fn assert_later_product_state_is_retained(fixture: &Fixture) {
    for retained in [
        &fixture.paths.unit,
        &fixture.paths.marketplace,
        &fixture.paths.config,
        &fixture.paths.manifest,
        &fixture.paths.binary,
    ] {
        assert!(retained.exists(), "{}", retained.display());
    }
    assert!(fixture.plugin_state.exists());
    assert!(fixture.marketplace_state.exists());
    assert!(fixture.codex_log().is_empty());
}

#[tokio::test]
async fn disable_removes_only_the_exact_descriptor_after_service_proof() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fixture.clear_logs();

    let report = disable_with_context(context(&fixture)).await.unwrap();

    assert!(!fixture.enabled.exists());
    assert!(!fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!descriptor(&fixture).exists());
    assert_eq!(
        report.stderr,
        "completed: service-disable\n\
completed: service-verify\n\
completed: descriptor-remove\n"
    );
    assert!(report.stdout.contains("Desktop restart required: yes\n"));
    assert!(report.stdout.contains(DESKTOP_DETACH_GUIDANCE));
}

#[tokio::test]
async fn unproven_service_stop_retains_descriptor_and_every_later_product_stage() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(
        &fixture.systemctl_fail,
        "--user is-active codex-session-control-test-Setup1.service",
    )
    .unwrap();
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(error.to_string().contains("failed at service-stop-verify:"));
    assert!(descriptor(&fixture).exists());
    assert_later_product_state_is_retained(&fixture);
}

#[tokio::test]
async fn present_socket_after_inactive_service_proof_blocks_descriptor_and_later_removal() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(&fixture.preserve_service_state, b"preserve").unwrap();
    fs::remove_file(&fixture.enabled).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed at service-stop-verify: invalid socket: still exists")
    );
    assert!(fixture.paths.socket.exists());
    assert!(descriptor(&fixture).exists());
    assert_later_product_state_is_retained(&fixture);
}

#[tokio::test]
async fn descriptor_drift_blocks_uninstall_before_every_later_removal() {
    for state in ["foreign", "unsafe", "unproven"] {
        let fixture = Fixture::new();
        let _authority = setup_attached(&fixture).await;
        let descriptor = descriptor(&fixture);
        match state {
            "foreign" => {
                fs::write(
                    &descriptor,
                    b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
                )
                .unwrap();
            }
            "unsafe" => {
                fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "unproven" => {
                fs::write(&fixture.paths.manifest, b"{}").unwrap();
            }
            _ => unreachable!(),
        }
        fixture.clear_logs();

        let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

        assert!(
            error.to_string().contains("failed at descriptor-remove:"),
            "{state}: {error}"
        );
        assert!(descriptor.exists(), "{state}");
        assert_later_product_state_is_retained(&fixture);
        assert!(!error.to_string().contains(DESKTOP_DETACH_GUIDANCE));
    }
}

#[tokio::test]
async fn configuration_only_uninstall_stops_at_descriptor_remove_without_guessing() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
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
    assert!(descriptor(&fixture).exists());
    for retained in [
        &fixture.paths.unit,
        &fixture.paths.marketplace,
        &fixture.paths.config,
        &fixture.paths.binary,
    ] {
        assert!(retained.exists(), "{}", retained.display());
    }
    assert!(fixture.codex_log().is_empty());
}

#[tokio::test]
async fn successful_uninstall_removes_product_state_and_preserves_normal_home_state() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    let preserved = [
        ("auth.json", b"auth".as_slice()),
        ("tasks/tasks.db", b"tasks".as_slice()),
        ("rollouts/rollout.jsonl", b"rollout".as_slice()),
        ("config.toml", b"unrelated config".as_slice()),
        (
            "plugins/unrelated/plugin.json",
            b"unrelated plugin".as_slice(),
        ),
    ];
    for (relative, bytes) in preserved {
        let path = fixture.paths.codex_home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fixture.clear_logs();

    let report = uninstall_with_context(context(&fixture)).await.unwrap();

    assert_eq!(
        report.stdout,
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
            fixture.paths.codex_home.display()
        )
    );
    assert_eq!(
        report.stderr,
        "completed: service-stop\n\
completed: service-stop-verify\n\
completed: descriptor-remove\n\
completed: service-unit-remove\n\
completed: plugin-remove\n\
completed: marketplace-remove\n\
completed: projection-remove\n\
completed: configuration-remove\n\
completed: manifest-remove\n\
completed: binary-remove\n\
Desktop: fully exit and restart Desktop to return to ordinary mode.\n"
    );
    assert!(!descriptor(&fixture).exists());
    for removed in [
        &fixture.paths.unit,
        &fixture.paths.marketplace,
        &fixture.paths.config,
        &fixture.paths.manifest,
        &fixture.paths.binary,
    ] {
        assert!(!removed.exists(), "{}", removed.display());
    }
    for (relative, bytes) in preserved {
        assert_eq!(
            fs::read(fixture.paths.codex_home.join(relative)).unwrap(),
            bytes
        );
    }
    assert!(fixture.paths.codex_home.is_dir());
    assert!(!report.stdout.contains("manual:"));
    assert!(!report.stdout.contains("rm -"));
}

#[tokio::test]
async fn failure_after_descriptor_removal_reports_desktop_restart_guidance() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(
        &fixture.codex_fail,
        "plugin marketplace remove codex-session-control-local --json",
    )
    .unwrap();
    fixture.clear_logs();

    let error = uninstall_with_context(context(&fixture)).await.unwrap_err();

    assert!(error.to_string().contains("failed at marketplace-remove:"));
    assert!(error.to_string().contains(DESKTOP_DETACH_GUIDANCE));
    assert!(!descriptor(&fixture).exists());
    assert!(fixture.paths.config.exists());
    assert!(fixture.paths.manifest.exists());
    assert!(fixture.paths.binary.exists());
}
