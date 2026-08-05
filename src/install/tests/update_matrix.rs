use std::{ffi::OsString, fs, os::unix::fs::MetadataExt};

use serde_json::Value;

use super::support::{FakeAuthority, Fixture};
use super::*;

const DESIRED_STATES: [(&str, bool, bool); 3] = [
    ("enabled_running", true, true),
    ("enabled_inactive", true, false),
    ("disabled_stopped", false, false),
];

fn candidate(fixture: &Fixture, name: &str, product: &str, version: &str, target: &str) -> PathBuf {
    let path = fixture.paths.home.join(name);
    super::write_executable_fixture(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '{product} {version} ({target})\\n'; exit 0; fi\nexit 64\n"
        ),
    );
    path
}

fn context(
    fixture: &Fixture,
    candidate: PathBuf,
    path_environment: Option<OsString>,
) -> UpdateContext {
    let setup = fixture.context(true);
    UpdateContext {
        lifecycle: LifecycleContext {
            target: setup.target,
            path_environment: path_environment.unwrap_or(setup.path_environment),
            desktop_environment: setup.desktop_environment,
            cwd: setup.cwd,
        },
        candidate,
        terminal: TerminalState {
            stdin: false,
            stderr: false,
            restart_prompt: None,
        },
    }
}

fn manifest(paths: &ResolvedUserPaths) -> InstalledRelease {
    serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap()
}

fn write_compatible_launcher(path: &Path) {
    super::write_executable_fixture(
        path,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
}

fn write_incompatible_launcher(path: &Path) {
    super::write_executable_fixture(
        path,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[]}'; exit 0; fi\nexit 64\n",
    );
}

fn drift_projection(fixture: &Fixture) {
    fs::write(
        fixture
            .paths
            .marketplace
            .join("plugins/codex-session-control/.mcp.json"),
        b"drift",
    )
    .unwrap();
}

fn equal_candidate(fixture: &Fixture, name: &str) -> PathBuf {
    let candidate = fixture.paths.home.join(name);
    super::write_executable_fixture(&candidate, fs::read(&fixture.paths.binary).unwrap());
    candidate
}

async fn setup_attached(fixture: &Fixture) -> (FakeAuthority, DesktopAttachmentIdentity) {
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    let launcher = fixture._root.path().join("desktop-launcher");
    write_compatible_launcher(&launcher);
    let mut setup = fixture.context(true);
    setup.desktop_launcher = Some(launcher);
    setup_with_context(setup).await.unwrap();
    let attachment = manifest(&fixture.paths).desktop_attachment.unwrap();
    (authority, attachment)
}

#[tokio::test]
async fn candidate_identity_and_semver_rejections_precede_all_mutation() {
    for (name, product, version, target, expected) in [
        (
            "wrong-product",
            "other-product",
            env!("CARGO_PKG_VERSION"),
            product_target(),
            "candidate-preflight",
        ),
        (
            "wrong-target",
            "codex-session-control",
            env!("CARGO_PKG_VERSION"),
            "other-unknown-linux-gnu",
            "candidate-preflight",
        ),
        (
            "lower",
            "codex-session-control",
            "0.0.9",
            product_target(),
            "candidate-preflight",
        ),
    ] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let before_binary = fs::read(&fixture.paths.binary).unwrap();
        let before_manifest = fs::read(&fixture.paths.manifest).unwrap();
        let candidate = candidate(&fixture, name, product, version, target);
        fixture.clear_logs();

        let error = staged_update_with_context(context(&fixture, candidate, None))
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains(&format!("failed at {expected}:"))
        );
        assert_eq!(fs::read(&fixture.paths.binary).unwrap(), before_binary);
        assert_eq!(fs::read(&fixture.paths.manifest).unwrap(), before_manifest);
        assert!(!fixture.systemctl_log().contains("daemon-reload"));
    }
}

#[tokio::test]
async fn coherent_equal_candidate_reports_current_only_after_state_proof() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let candidate = fixture.paths.home.join("candidate-equal");
    super::write_executable_fixture(&candidate, fs::read(&fixture.paths.binary).unwrap());
    fixture.clear_logs();

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert_eq!(
        report.stdout,
        format!(
            "Already current: {}\nDurable and running state: coherent\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert!(report.stderr.is_empty());
    assert!(fixture.systemctl_log().contains("--user is-enabled"));
    assert!(fixture.systemctl_log().contains("--user is-active"));
    assert!(!fixture.systemctl_log().contains("daemon-reload"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn higher_candidate_preserves_all_three_desired_service_states() {
    for (name, enabled, active) in DESIRED_STATES {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let authority = if active {
            Some(authority)
        } else {
            drop(authority);
            fs::remove_file(&fixture.active).unwrap();
            fs::remove_file(&fixture.paths.socket).unwrap();
            None
        };
        if !enabled {
            fs::remove_file(&fixture.enabled).unwrap();
        }
        let candidate = candidate(
            &fixture,
            "candidate-higher",
            "codex-session-control",
            "0.3.0",
            product_target(),
        );
        fixture.clear_logs();

        let starter = if enabled && !active {
            fs::write(&fixture.wait_for_socket, b"wait").unwrap();
            let paths = fixture.paths.clone();
            let active_path = fixture.active.clone();
            Some(tokio::spawn(async move {
                while !active_path.exists() {
                    tokio::task::yield_now().await;
                }
                FakeAuthority::start(&paths, "0.146.0").await
            }))
        } else {
            None
        };
        let report = staged_update_with_context(context(&fixture, candidate.clone(), None))
            .await
            .unwrap();
        let started = match starter {
            Some(starter) => Some(starter.await.unwrap()),
            None => None,
        };

        assert!(
            report.stdout.starts_with("Installed release: 0.3.0\n"),
            "{name}"
        );
        assert_eq!(manifest(&fixture.paths).product_version, "0.3.0");
        assert_eq!(
            fs::read(&fixture.paths.binary).unwrap(),
            fs::read(candidate).unwrap()
        );
        assert_eq!(fixture.enabled.exists(), enabled, "{name}");
        assert_eq!(fixture.active.exists(), enabled, "{name}");
        assert_eq!(fixture.paths.socket.exists(), enabled, "{name}");
        assert!(!fixture.systemctl_log().contains(" restart "), "{name}");
        if enabled && !active {
            assert!(fixture.systemctl_log().contains(" enable --now "), "{name}");
        }
        drop(started);
        drop(authority);
    }
}

#[tokio::test]
async fn equal_projection_drift_preserves_desktop_and_running_authority() {
    let fixture = Fixture::new();
    let (authority, attachment) = setup_attached(&fixture).await;
    let descriptor = fs::read(&attachment.descriptor_path).unwrap();
    let socket_inode = fs::symlink_metadata(&fixture.paths.socket).unwrap().ino();
    drift_projection(&fixture);
    let candidate = equal_candidate(&fixture, "candidate-equal-drift");
    fixture.clear_logs();

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        env!("CARGO_PKG_VERSION")
    )));
    assert_eq!(
        manifest(&fixture.paths).desktop_attachment,
        Some(attachment.clone())
    );
    assert_eq!(fs::read(&attachment.descriptor_path).unwrap(), descriptor);
    assert_eq!(
        fs::symlink_metadata(&fixture.paths.socket).unwrap().ino(),
        socket_inode
    );
    assert!(!fixture.systemctl_log().contains(" restart "));
    assert!(report.stdout.contains("Desktop attachment: available\n"));
    assert!(report.stdout.contains("Desktop restart required: no\n"));
    let stages = &report.stderr;
    assert!(
        stages.find("completed: plugin-install\n").unwrap()
            < stages.find("completed: desktop-discovery\n").unwrap()
    );
    assert!(
        stages.find("completed: desktop-discovery\n").unwrap()
            < stages.find("completed: descriptor\n").unwrap()
    );
    assert!(
        stages.find("completed: descriptor\n").unwrap()
            < stages.find("completed: service-unit\n").unwrap()
    );
    assert!(
        serde_json::from_slice::<Value>(
            &fs::read(
                fixture
                    .paths
                    .marketplace
                    .join("plugins/codex-session-control/.mcp.json")
            )
            .unwrap()
        )
        .is_ok()
    );
    drop(authority);
}

#[tokio::test]
async fn equal_healthy_attached_candidate_is_current_without_full_pipeline() {
    let fixture = Fixture::new();
    let (authority, attachment) = setup_attached(&fixture).await;
    let descriptor = fs::read(&attachment.descriptor_path).unwrap();
    let candidate = equal_candidate(&fixture, "candidate-equal-attached");
    fixture.clear_logs();

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert_eq!(
        report.stdout,
        format!(
            "Already current: {}\nDurable and running state: coherent\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert!(report.stderr.is_empty());
    assert_eq!(
        manifest(&fixture.paths).desktop_attachment,
        Some(attachment.clone())
    );
    assert_eq!(fs::read(&attachment.descriptor_path).unwrap(), descriptor);
    assert!(!fixture.systemctl_log().contains("daemon-reload"));
    assert!(!fixture.systemctl_log().contains(" restart "));
    drop(authority);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_update_retains_desktop_identity_without_publishing_then_enable_republishes() {
    let fixture = Fixture::new();
    let (authority, attachment) = setup_attached(&fixture).await;
    drop(authority);
    fs::remove_file(&fixture.enabled).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();
    fs::remove_file(&attachment.descriptor_path).unwrap();
    drift_projection(&fixture);
    let candidate = equal_candidate(&fixture, "candidate-disabled-desktop");
    fixture.clear_logs();

    let update = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert_eq!(
        manifest(&fixture.paths).desktop_attachment,
        Some(attachment.clone())
    );
    assert!(!attachment.descriptor_path.exists());
    assert!(!fixture.enabled.exists());
    assert!(!fixture.active.exists());
    assert!(update.stdout.contains("Desktop attachment: available\n"));
    assert!(update.stdout.contains("Desktop restart required: no\n"));

    fs::write(
        &fixture.required_descriptor,
        attachment.descriptor_path.display().to_string(),
    )
    .unwrap();
    fs::write(&fixture.wait_for_socket, b"wait").unwrap();
    fixture.clear_logs();
    let paths = fixture.paths.clone();
    let active = fixture.active.clone();
    let starter = tokio::spawn(async move {
        while !active.exists() {
            tokio::task::yield_now().await;
        }
        FakeAuthority::start(&paths, "0.146.0").await
    });
    let setup = fixture.context(true);
    let enabled = enable_with_context(LifecycleContext {
        target: setup.target,
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    })
    .await
    .unwrap();
    let authority = starter.await.unwrap();

    assert_eq!(
        fs::read(&attachment.descriptor_path).unwrap(),
        render_descriptor(&fixture.paths.socket).unwrap()
    );
    assert!(enabled.stdout.contains("Desktop restart required: yes\n"));
    drop(authority);
}

#[tokio::test]
async fn null_desktop_update_never_auto_selects_a_new_compatible_launcher() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
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
    write_compatible_launcher(&launcher);
    fs::create_dir_all(desktop_entry.parent().unwrap()).unwrap();
    fs::write(
        desktop_entry,
        format!(
            "[Desktop Entry]\nType=Application\nExec={}\n",
            launcher.display()
        ),
    )
    .unwrap();
    drift_projection(&fixture);
    let candidate = equal_candidate(&fixture, "candidate-null-desktop");
    fixture.clear_logs();

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert_eq!(manifest(&fixture.paths).desktop_attachment, None);
    assert!(!descriptor.exists());
    assert!(report.stdout.contains("Desktop attachment: unavailable\n"));
    assert!(report.stdout.contains("Desktop restart required: no\n"));
    assert!(!fixture.systemctl_log().contains(" restart "));
    drop(authority);
}

#[tokio::test]
async fn temporarily_unavailable_desktop_retains_exact_routing_and_warns_unverified() {
    let fixture = Fixture::new();
    let (authority, attachment) = setup_attached(&fixture).await;
    let descriptor = fs::read(&attachment.descriptor_path).unwrap();
    let socket_inode = fs::symlink_metadata(&fixture.paths.socket).unwrap().ino();
    fs::remove_file(&attachment.descriptor_path).unwrap();
    write_incompatible_launcher(&attachment.launcher_path);
    drift_projection(&fixture);
    let candidate = equal_candidate(&fixture, "candidate-unavailable-desktop");
    fixture.clear_logs();

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert_eq!(
        manifest(&fixture.paths).desktop_attachment,
        Some(attachment.clone())
    );
    assert_eq!(fs::read(&attachment.descriptor_path).unwrap(), descriptor);
    assert!(report.stdout.contains("Desktop attachment: unverified\n"));
    assert!(report.stdout.contains("Desktop restart required: yes\n"));
    assert!(report.stderr.contains("Desktop attachment unavailable:"));
    assert_eq!(
        fs::symlink_metadata(&fixture.paths.socket).unwrap().ino(),
        socket_inode
    );
    assert!(!fixture.systemctl_log().contains(" restart "));
    drop(authority);
}

#[tokio::test]
async fn running_update_republishes_a_missing_expected_descriptor_without_restarting() {
    let fixture = Fixture::new();
    let (authority, attachment) = setup_attached(&fixture).await;
    let socket_inode = fs::symlink_metadata(&fixture.paths.socket).unwrap().ino();
    fs::remove_file(&attachment.descriptor_path).unwrap();
    let candidate = equal_candidate(&fixture, "candidate-missing-descriptor");
    fixture.clear_logs();

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();

    assert_eq!(
        fs::read(&attachment.descriptor_path).unwrap(),
        render_descriptor(&fixture.paths.socket).unwrap()
    );
    assert_eq!(
        fs::symlink_metadata(&fixture.paths.socket).unwrap().ino(),
        socket_inode
    );
    assert!(!fixture.systemctl_log().contains(" restart "));
    assert!(report.stdout.contains("Desktop attachment: available\n"));
    assert!(report.stdout.contains("Desktop restart required: yes\n"));
    drop(authority);
}

#[tokio::test]
async fn unsafe_or_foreign_desktop_descriptor_stops_update_at_descriptor_stage() {
    for case in ["unsafe", "foreign"] {
        let fixture = Fixture::new();
        let (authority, attachment) = setup_attached(&fixture).await;
        if case == "unsafe" {
            fs::set_permissions(
                &attachment.descriptor_path,
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        } else {
            fs::write(
                &attachment.descriptor_path,
                b"{\"schemaVersion\":1,\"transport\":\"unix\",\"socketPath\":\"/foreign\"}",
            )
            .unwrap();
        }
        let descriptor = fs::read(&attachment.descriptor_path).unwrap();
        let manifest = fs::read(&fixture.paths.manifest).unwrap();
        let candidate = equal_candidate(&fixture, &format!("candidate-{case}-descriptor"));
        fixture.clear_logs();

        let error = staged_update_with_context(context(&fixture, candidate, None))
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("failed at descriptor:"),
            "{case}: {error}"
        );
        assert!(
            error.to_string().contains(if case == "unsafe" {
                "Desktop descriptor safety error"
            } else {
                "Desktop descriptor is foreign"
            }),
            "{case}: {error}"
        );
        assert_eq!(fs::read(&attachment.descriptor_path).unwrap(), descriptor);
        assert_eq!(fs::read(&fixture.paths.manifest).unwrap(), manifest);
        assert!(!fixture.systemctl_log().contains(" restart "));
        drop(authority);
    }
}

#[tokio::test]
async fn disabled_active_and_unknown_restart_evidence_fail_before_mutation() {
    for case in ["disabled-active", "unknown-unit"] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
        setup_with_context(fixture.context(true)).await.unwrap();
        if case == "disabled-active" {
            fs::remove_file(&fixture.enabled).unwrap();
        } else {
            fs::write(&fixture.paths.unit, b"contradictory unit").unwrap();
        }
        let before_binary = fs::read(&fixture.paths.binary).unwrap();
        let candidate = candidate(
            &fixture,
            "candidate-unknown",
            "codex-session-control",
            env!("CARGO_PKG_VERSION"),
            product_target(),
        );
        fixture.clear_logs();

        let error = staged_update_with_context(context(&fixture, candidate, None))
            .await
            .unwrap_err();

        let stage = if case == "disabled-active" {
            "service-snapshot"
        } else {
            "restart-inspection"
        };
        assert!(error.to_string().contains(&format!("failed at {stage}:")));
        if case == "unknown-unit" {
            assert!(
                error
                    .to_string()
                    .contains("installed service unit does not match last coherent manifest")
            );
            assert!(error.to_string().contains(
                        "codex-session-control disable\ncodex-session-control update\ncodex-session-control enable"
                    ));
        }
        assert_eq!(fs::read(&fixture.paths.binary).unwrap(), before_binary);
        assert!(!fixture.systemctl_log().contains("daemon-reload"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_changed_authority_inspects_then_restarts_exactly_once() {
    let fixture = Fixture::new();
    let old_authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let new_bin = fixture.paths.home.join("new-codex-bin");
    fs::create_dir(&new_bin).unwrap();
    let new_codex = new_bin.join("codex");
    fs::copy(fixture.fake_bin.join("codex"), &new_codex).unwrap();
    fs::set_permissions(
        &new_codex,
        fs::metadata(fixture.fake_bin.join("codex"))
            .unwrap()
            .permissions(),
    )
    .unwrap();
    let setup = fixture.context(true);
    let mut path = vec![new_bin];
    path.extend(std::env::split_paths(&setup.path_environment));
    let path = std::env::join_paths(path).unwrap();
    let candidate = candidate(
        &fixture,
        "candidate-changed",
        "codex-session-control",
        "0.3.0",
        product_target(),
    );
    fs::write(&fixture.wait_for_socket, b"wait").unwrap();
    fixture.clear_logs();
    let paths = fixture.paths.clone();
    let restart_requested = fixture.restart_requested.clone();
    let starter = tokio::spawn(async move {
        while !restart_requested.exists() {
            tokio::task::yield_now().await;
        }
        FakeAuthority::start(&paths, "0.146.0").await
    });

    let report = staged_update_with_context(context(&fixture, candidate, Some(path)))
        .await
        .unwrap();
    let new_authority = starter.await.unwrap();

    assert!(report.stdout.starts_with("Installed release: 0.3.0\n"));
    assert_eq!(manifest(&fixture.paths).codex_executable, new_codex);
    assert_eq!(fixture.systemctl_log().matches(" restart ").count(), 1);
    assert!(!fixture.codex_log().contains("thread/interrupt"));
    drop(new_authority);
    drop(old_authority);
}

#[tokio::test]
async fn retry_uses_last_manifest_after_partial_candidate_files() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, "0.146.0").await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let candidate = candidate(
        &fixture,
        "candidate-retry",
        "codex-session-control",
        "0.3.0",
        product_target(),
    );
    fs::write(&fixture.systemctl_fail, "--user daemon-reload").unwrap();

    let error = staged_update_with_context(context(&fixture, candidate.clone(), None))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("failed at daemon-reload:"));
    assert_eq!(
        manifest(&fixture.paths).product_version,
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        fs::read(&fixture.paths.binary).unwrap(),
        fs::read(&candidate).unwrap()
    );

    fs::remove_file(&fixture.systemctl_fail).unwrap();
    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();
    assert!(report.stdout.starts_with("Installed release: 0.3.0\n"));
    assert_eq!(manifest(&fixture.paths).product_version, "0.3.0");
}

#[tokio::test]
async fn tested_untested_and_unparseable_versions_only_change_update_advisory() {
    for (version_output, authority_version, warning) in [
        ("codex-cli 0.146.0\n", "0.146.0", None),
        ("codex-cli 0.147.0\n", "0.147.0", Some("0.147.0")),
        ("codex-cli not-semver\n", "not-semver", Some("not-semver")),
    ] {
        let fixture = Fixture::new();
        fs::write(&fixture.codex_version, version_output).unwrap();
        let _authority = FakeAuthority::start(&fixture.paths, authority_version).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let candidate = candidate(
            &fixture,
            "candidate-advisory",
            "codex-session-control",
            "0.3.0",
            product_target(),
        );
        fixture.clear_logs();

        let report = staged_update_with_context(context(&fixture, candidate, None))
            .await
            .unwrap();

        assert!(report.stdout.starts_with("Installed release: 0.3.0\n"));
        assert_eq!(
            report.stderr.contains("Compatibility warning:"),
            warning.is_some(),
            "{version_output}"
        );
        if let Some(version) = warning {
            assert!(report.stderr.ends_with(&format!(
                        "Compatibility warning: Codex app-server {version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
                        env!("CARGO_PKG_VERSION")
                    )));
            assert_eq!(report.stderr.matches("Compatibility warning:").count(), 1);
        }
        assert_eq!(
            manifest(&fixture.paths).codex_version,
            persisted_codex_version(
                version_output
                    .trim()
                    .strip_prefix("codex-cli ")
                    .unwrap_or(version_output.trim())
            )
        );
    }
}

#[test]
fn candidate_apply_sets_only_the_private_staged_marker() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("candidate.log");
    let executable = root.path().join("candidate");
    super::write_executable_fixture(
        &executable,
        format!(
            "#!/bin/sh\nprintf 'argv=%s|marker=%s\\n' \"$*\" \"$CODEX_SESSION_CONTROL_STAGED_UPDATE\" > '{}'\n",
            log.display()
        ),
    );
    let candidate = CandidateRelease {
        executable,
        product_version: env!("CARGO_PKG_VERSION").to_owned(),
        target: product_target().to_owned(),
    };

    run_candidate_apply(&candidate).unwrap();

    assert_eq!(fs::read_to_string(log).unwrap(), "argv=update|marker=1\n");
}
