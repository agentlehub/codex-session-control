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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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

fn service_log_prefix(fixture: &Fixture) -> String {
    format!(
        "--user is-active {}\n",
        fixture.context(true).target.unit_name
    )
}

fn assert_active_stop_was_not_attempted(fixture: &Fixture) {
    let log = fixture.systemctl_log();
    assert!(!log.contains("disable --now"), "{log}");
    assert!(!log.contains("daemon-reload"), "{log}");
    assert!(fixture.enabled.exists());
    assert!(fixture.active.exists());
    assert!(fixture.paths.socket.exists());
}

#[tokio::test]
async fn active_self_hosted_disable_refuses_before_stop_and_descriptor_removal() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(
        &fixture.whoami_unit,
        b"codex-session-control-test-Setup1.service\n",
    )
    .unwrap();
    let auth = fixture.paths.codex_home.join("auth.json");
    fs::write(&auth, b"auth sentinel").unwrap();
    let descriptor = descriptor(&fixture);
    let before = [
        fs::read(&fixture.paths.manifest).unwrap(),
        fs::read(&descriptor).unwrap(),
        fs::read(&auth).unwrap(),
    ];
    fixture.clear_logs();

    let error = disable_with_context(context(&fixture)).await.unwrap_err();

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::IndependentTerminal(
            crate::cli_output::IndependentTerminal::Disable
        )
    ));
    assert_eq!(
        fixture.systemctl_log(),
        service_log_prefix(&fixture) + "--user whoami\n"
    );
    assert_active_stop_was_not_attempted(&fixture);
    assert_eq!(fs::read(&fixture.paths.manifest).unwrap(), before[0]);
    assert_eq!(fs::read(&descriptor).unwrap(), before[1]);
    assert_eq!(fs::read(auth).unwrap(), before[2]);
}

#[tokio::test]
async fn disable_stop_preflight_is_fail_closed_and_preserves_independent_or_inactive_success() {
    for case in [
        "self-hosted-control-group",
        "unknown-caller",
        "unproven-activity",
        "proven-inactive",
    ] {
        let fixture = Fixture::new();
        let authority = setup_attached(&fixture).await;
        let mut lifecycle = context(&fixture);
        let expected_log = match case {
            "self-hosted-control-group" => {
                fs::write(&fixture.systemctl_fail, b"--user whoami").unwrap();
                fs::write(
                    &fixture.control_group,
                    b"/user.slice/app.slice/codex-session-control-test-Setup1.service\n",
                )
                .unwrap();
                lifecycle.target = lifecycle.target.with_caller_cgroup_snapshot(
                    b"0::/user.slice/app.slice/codex-session-control-test-Setup1.service/child.scope\n"
                        .to_vec(),
                );
                None
            }
            "unknown-caller" => {
                fs::write(&fixture.systemctl_fail, b"--user whoami").unwrap();
                None
            }
            "unproven-activity" => {
                fs::write(
                    &fixture.systemctl_fail,
                    b"--user is-active codex-session-control-test-Setup1.service",
                )
                .unwrap();
                None
            }
            "proven-inactive" => {
                drop(authority);
                fs::remove_file(&fixture.active).unwrap();
                fs::remove_file(&fixture.paths.socket).unwrap();
                Some(
                    service_log_prefix(&fixture)
                        + "--user disable --now codex-session-control-test-Setup1.service\n"
                        + "--user is-enabled codex-session-control-test-Setup1.service\n"
                        + "--user is-active codex-session-control-test-Setup1.service\n",
                )
            }
            _ => unreachable!(),
        };
        fixture.clear_logs();

        let result = disable_with_context(lifecycle).await;

        match case {
            "self-hosted-control-group" | "unknown-caller" => {
                let error = result.unwrap_err();
                assert!(
                    matches!(
                        error,
                        crate::cli_output::UserFailure::StopThenRetry(
                            crate::cli_output::StopThenRetry::DisableUnsafeStopThenDisable
                        )
                    ),
                    "{case}: {error:?}"
                );
                assert_active_stop_was_not_attempted(&fixture);
            }
            "unproven-activity" => {
                let error = result.unwrap_err();
                assert!(
                    matches!(
                        error,
                        crate::cli_output::UserFailure::StopThenRetry(
                            crate::cli_output::StopThenRetry::DisableUnsafeStopThenDisable
                        )
                    ),
                    "{error:?}"
                );
                assert!(!fixture.systemctl_log().contains("--user whoami"));
                assert!(!fixture.systemctl_log().contains("disable --now"));
            }
            "proven-inactive" => {
                result.unwrap();
                assert_eq!(fixture.systemctl_log(), expected_log.unwrap());
                assert!(!fixture.systemctl_log().contains("--user whoami"));
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn disable_removes_only_the_exact_descriptor_after_service_proof() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fixture.clear_logs();

    let report = disable_with_context(context(&fixture))
        .await
        .unwrap()
        .render();

    assert!(!fixture.enabled.exists());
    assert!(!fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!descriptor(&fixture).exists());
    assert_eq!(
        fixture.systemctl_log(),
        service_log_prefix(&fixture)
            + "--user whoami\n"
            + "--user disable --now codex-session-control-test-Setup1.service\n"
            + "--user is-enabled codex-session-control-test-Setup1.service\n"
            + "--user is-active codex-session-control-test-Setup1.service\n"
    );
    assert!(report.stderr.is_empty());
    assert!(report.stdout.contains(
        "If Codex Desktop is already running, restart it to continue without Codex Session Control."
    ));
}

#[tokio::test]
async fn unproven_service_stop_retains_descriptor_and_every_later_product_stage() {
    let fixture = Fixture::new();
    let _authority = setup_attached(&fixture).await;
    fs::write(&fixture.fail_service_verify_after_stop, b"fail").unwrap();
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
