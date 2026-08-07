use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

use super::support::{FakeAuthority, Fixture};
use super::*;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const SETUP_STAGES: [&str; 13] = [
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
];
const STAGED_UPDATE_STAGES: [&str; 15] = [
    "candidate-preflight",
    "service-snapshot",
    "restart-inspection",
    "binary",
    "configuration",
    "projection",
    "plugin-marketplace",
    "plugin-install",
    "desktop-discovery",
    "descriptor",
    "service-unit",
    "daemon-reload",
    "service-apply",
    "service-verify",
    "manifest",
];
const UNINSTALL_STAGES: [&str; 10] = [
    "service-stop",
    "service-stop-verify",
    "descriptor-remove",
    "service-unit-remove",
    "plugin-remove",
    "marketplace-remove",
    "projection-remove",
    "configuration-remove",
    "manifest-remove",
    "binary-remove",
];

fn candidate(fixture: &Fixture, name: &str) -> PathBuf {
    let path = fixture.paths.home.join(name);
    super::write_executable_fixture(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nexit 64\n",
            higher_test_release_version(),
            product_target()
        ),
    );
    path
}

fn update_context(
    fixture: &Fixture,
    candidate: PathBuf,
    stage: Option<&'static str>,
    path_environment: Option<OsString>,
) -> UpdateContext {
    let setup = fixture.context(true);
    let target = match stage {
        Some(stage) => setup.target.fail_after_completed_stage(stage),
        None => setup.target,
    };
    UpdateContext {
        lifecycle: LifecycleContext {
            target,
            path_environment: path_environment.unwrap_or(setup.path_environment),
            desktop_environment: setup.desktop_environment,
            cwd: setup.cwd,
        },
        candidate,
        terminal: TerminalState::noninteractive(),
    }
}

fn lifecycle_context_with_stage(
    fixture: &Fixture,
    stage: Option<&'static str>,
) -> LifecycleContext {
    let setup = fixture.context(true);
    let target = match stage {
        Some(stage) => setup.target.fail_after_completed_stage(stage),
        None => setup.target,
    };
    LifecycleContext {
        target,
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    }
}

fn installed(paths: &ResolvedUserPaths) -> InstalledRelease {
    serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap()
}

fn assert_no_backups(path: &Path) {
    if !path.exists() {
        return;
    }
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(!name.contains("backup"), "unexpected backup: {name}");
        if entry.file_type().unwrap().is_dir() {
            assert_no_backups(&entry.path());
        }
    }
}

fn assert_injected(error: &ControllerError, stage: &str, command: &str) {
    let error = error.to_string();
    assert!(
        error.contains(&format!("completed: {stage}\nfailed at {stage}:")),
        "{error}"
    );
    assert!(error.contains(command), "{error}");
}

fn assert_injected_before_stage(error: &ControllerError, stage: &str, command: &str) {
    let error = error.to_string();
    assert!(error.contains(&format!("failed at {stage}:")), "{error}");
    assert!(!error.contains(&format!("completed: {stage}\n")), "{error}");
    assert!(error.contains(command), "{error}");
}

fn seed_preserved_normal_home_state(fixture: &Fixture) -> Vec<(PathBuf, Vec<u8>, u32)> {
    let home = &fixture.paths.codex_home;
    let preserved = vec![
        (
            home.join("credentials.json"),
            b"credentials".to_vec(),
            0o600,
        ),
        (home.join("tasks.json"), b"tasks".to_vec(), 0o600),
        (home.join("rollouts.json"), b"rollouts".to_vec(), 0o600),
        (
            home.join("config.toml"),
            b"unrelated-config".to_vec(),
            0o600,
        ),
        (
            home.join("plugins/unrelated.json"),
            b"unrelated-plugin".to_vec(),
            0o600,
        ),
    ];
    for (path, bytes, mode) in &preserved {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(*mode)).unwrap();
    }
    preserved
}

fn assert_preserved_normal_home_state(preserved: &[(PathBuf, Vec<u8>, u32)]) {
    for (path, bytes, mode) in preserved {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.file_type().is_file(), "{}", path.display());
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            *mode,
            "{}",
            path.display()
        );
        assert_eq!(fs::read(path).unwrap(), *bytes, "{}", path.display());
    }
}

async fn immutable_release_server(
    fixture: &Fixture,
) -> (ReleaseEndpoints, tokio::task::JoinHandle<()>, PathBuf) {
    let candidate_log = fixture.paths.home.join("outer-candidate.log");
    let version = higher_test_release_version();
    let binary = Arc::<[u8]>::from(
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control {} ({})\\n'; exit 0; fi\nif [ \"$1\" = update ]; then exit 0; fi\nexit 64\n",
                    candidate_log.display(),
                    version,
                    product_target()
                )
                .into_bytes(),
            );
    let digest = hex::encode(Sha256::digest(binary.as_ref()));
    let binary_name = format!("codex-session-control-{}", product_target());
    let checksums = Arc::<[u8]>::from(format!("{digest}  {binary_name}\n").into_bytes());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let metadata = Arc::<[u8]>::from(
        json!({
            "tag_name": format!("v{version}"),
            "assets": [{
                "name": binary_name,
                "browser_download_url": format!("{base}/releases/download/v{version}/{binary_name}"),
                "size": binary.len()
            }, {
                "name": "SHA256SUMS",
                "browser_download_url": format!("{base}/releases/download/v{version}/SHA256SUMS"),
                "size": checksums.len()
            }]
        })
        .to_string()
        .into_bytes(),
    );
    let served_binary = Arc::clone(&binary);
    let served_checksums = Arc::clone(&checksums);
    let served_metadata = Arc::clone(&metadata);
    let served_binary_name = binary_name.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0);
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8(request).unwrap();
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let body = if path.ends_with("/releases/latest") {
                Arc::clone(&served_metadata)
            } else if path.ends_with(&format!("/{served_binary_name}")) {
                Arc::clone(&served_binary)
            } else if path.ends_with("/SHA256SUMS") {
                Arc::clone(&served_checksums)
            } else {
                panic!("unexpected release path: {path}");
            };
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    (
        ReleaseEndpoints {
            api: base.clone(),
            downloads: base,
        },
        task,
        candidate_log,
    )
}

#[test]
fn production_target_construction_has_no_selected_test_hook() {
    let fixture = Fixture::new();
    let target = LifecycleTarget::production(fixture.paths.clone());
    let suffixed = LifecycleTarget::suffixed(fixture.paths.clone(), "Retry1");

    assert_eq!(target.unit_name, "codex-session-control.service");
    assert_eq!(target.test_hooks.fail_after_completed_stage, None);
    assert_eq!(
        suffixed.paths.unit.file_name().unwrap(),
        "codex-session-control-test-Retry1.service"
    );
}

#[tokio::test]
async fn setup_retries_after_every_completed_stage_without_rollback() {
    for stage in SETUP_STAGES {
        let fixture = Fixture::new();
        let preserved = seed_preserved_normal_home_state(&fixture);
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        let mut context = fixture.context(true);
        context.target = context.target.fail_after_completed_stage(stage);

        let error = setup_with_context(context).await.unwrap_err();

        assert_injected(&error, stage, "retry: codex-session-control setup");
        assert_no_backups(&fixture.paths.home);
        assert_preserved_normal_home_state(&preserved);
        let report = setup_with_context(fixture.context(true)).await.unwrap();
        assert!(report.stdout.starts_with(&format!(
            "Installed release: {}\n",
            env!("CARGO_PKG_VERSION")
        )));
        assert_preserved_normal_home_state(&preserved);
        let manifest = installed(&fixture.paths);
        manifest
            .validate(&fixture.paths.codex_home, &fixture.paths.socket)
            .unwrap();
        assert_eq!(manifest.product_version, env!("CARGO_PKG_VERSION"));
    }
}

#[tokio::test]
async fn outer_update_retries_every_release_and_candidate_apply_stage() {
    for stage in [
        "release-discovery",
        "release-download",
        "checksum",
        "candidate-preflight",
        "candidate-apply",
    ] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let (endpoints, server, candidate_log) = immutable_release_server(&fixture).await;
        let setup = fixture.context(true);
        let failing = LifecycleContext {
            target: setup.target.fail_after_completed_stage(stage),
            path_environment: setup.path_environment.clone(),
            desktop_environment: setup.desktop_environment.clone(),
            cwd: setup.cwd.clone(),
        };

        let error = outer_update_with_endpoints(failing, endpoints.clone())
            .await
            .unwrap_err();

        assert_injected(&error, stage, "retry: codex-session-control update");
        assert_no_backups(&fixture.paths.home);
        let report = outer_update_with_endpoints(
            LifecycleContext {
                target: fixture.context(true).target,
                path_environment: setup.path_environment,
                desktop_environment: setup.desktop_environment,
                cwd: setup.cwd,
            },
            endpoints,
        )
        .await
        .unwrap();
        assert_eq!(report.stderr, "completed: candidate-apply\n");
        assert_eq!(
            installed(&fixture.paths).product_version,
            env!("CARGO_PKG_VERSION")
        );
        assert!(
            status_with_context(StatusContext {
                target: fixture.context(true).target,
                path_environment: fixture.context(true).path_environment,
                desktop_environment: fixture.context(true).desktop_environment,
                cwd: fixture.context(true).cwd,
            })
            .await
            .unwrap()
            .healthy
        );
        let candidate_runs = fs::read_to_string(candidate_log).unwrap();
        assert_eq!(
            candidate_runs
                .lines()
                .filter(|line| *line == "update")
                .count(),
            if stage == "candidate-apply" { 2 } else { 1 }
        );
        server.abort();
    }
}

#[tokio::test]
async fn staged_update_retries_every_stage_for_running_and_stopped_services() {
    for running in [true, false] {
        for stage in STAGED_UPDATE_STAGES {
            let fixture = Fixture::new();
            let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
            setup_with_context(fixture.context(true)).await.unwrap();
            let authority = if running {
                Some(authority)
            } else {
                drop(authority);
                fs::remove_file(&fixture.enabled).unwrap();
                fs::remove_file(&fixture.active).unwrap();
                fs::remove_file(&fixture.paths.socket).unwrap();
                None
            };
            let candidate = candidate(
                &fixture,
                &format!(
                    "candidate-{}-{stage}",
                    if running { "running" } else { "stopped" }
                ),
            );

            let error = staged_update_with_context(update_context(
                &fixture,
                candidate.clone(),
                Some(stage),
                None,
            ))
            .await
            .unwrap_err();

            assert_injected(&error, stage, "retry: codex-session-control update");
            let higher_version = higher_test_release_version();
            assert_eq!(
                installed(&fixture.paths).product_version,
                if stage == "manifest" {
                    higher_version.as_str()
                } else {
                    env!("CARGO_PKG_VERSION")
                },
                "{stage}"
            );
            assert_no_backups(&fixture.paths.home);
            let report =
                staged_update_with_context(update_context(&fixture, candidate, None, None))
                    .await
                    .unwrap();
            assert!(
                report
                    .stdout
                    .starts_with(&format!("Installed release: {higher_version}\n"))
                    || report
                        .stdout
                        .starts_with(&format!("Already current: {higher_version}\n")),
                "{stage}: {}",
                report.stdout
            );
            assert_eq!(installed(&fixture.paths).product_version, higher_version);
            assert_eq!(fixture.enabled.exists(), running);
            assert_eq!(fixture.active.exists(), running);
            assert_eq!(fixture.paths.socket.exists(), running);
            drop(authority);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_turn_gate_failure_retries_without_a_process_handoff() {
    let fixture = Fixture::new();
    let old_authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let candidate = candidate(&fixture, "candidate-active-turn-retry");
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
    let before_binary = fs::read(&fixture.paths.binary).unwrap();

    let error = staged_update_with_context(update_context(
        &fixture,
        candidate.clone(),
        Some("active-turn-gate"),
        Some(path.clone()),
    ))
    .await
    .unwrap_err();

    assert_injected(
        &error,
        "active-turn-gate",
        "retry: codex-session-control update",
    );
    assert_eq!(fs::read(&fixture.paths.binary).unwrap(), before_binary);
    fs::write(&fixture.wait_for_socket, b"wait").unwrap();
    let paths = fixture.paths.clone();
    let restart_requested = fixture.restart_requested.clone();
    let starter = tokio::spawn(async move {
        while !restart_requested.exists() {
            tokio::task::yield_now().await;
        }
        FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
    });

    let report = staged_update_with_context(update_context(&fixture, candidate, None, Some(path)))
        .await
        .unwrap();
    let restarted_authority = starter.await.unwrap();

    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        higher_test_release_version()
    )));
    assert_eq!(installed(&fixture.paths).codex_executable, new_codex);
    assert_eq!(fixture.systemctl_log().matches(" restart ").count(), 1);
    drop(restarted_authority);
    drop(old_authority);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_and_disable_retry_after_each_completed_service_stage() {
    for (operation, stages) in [
        ("enable", ["service-enable", "service-verify"]),
        ("disable", ["service-disable", "service-verify"]),
    ] {
        for stage in stages {
            let fixture = Fixture::new();
            let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
            setup_with_context(fixture.context(true)).await.unwrap();
            if operation == "enable" {
                disable_with_context(lifecycle_context_with_stage(&fixture, None))
                    .await
                    .unwrap();
                fs::write(&fixture.wait_for_socket, b"wait").unwrap();
                let paths = fixture.paths.clone();
                let active = fixture.active.clone();
                let starter = tokio::spawn(async move {
                    while !active.exists() {
                        tokio::task::yield_now().await;
                    }
                    FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
                });
                let error =
                    enable_with_context(lifecycle_context_with_stage(&fixture, Some(stage)))
                        .await
                        .unwrap_err();
                assert_injected(&error, stage, "retry: codex-session-control enable");
                enable_with_context(lifecycle_context_with_stage(&fixture, None))
                    .await
                    .unwrap();
                drop(starter.await.unwrap());
                assert!(fixture.enabled.exists());
                assert!(fixture.active.exists());
                assert!(fixture.paths.socket.exists());
            } else {
                let error =
                    disable_with_context(lifecycle_context_with_stage(&fixture, Some(stage)))
                        .await
                        .unwrap_err();
                assert_injected(&error, stage, "retry: codex-session-control disable");
                disable_with_context(lifecycle_context_with_stage(&fixture, None))
                    .await
                    .unwrap();
                assert!(!fixture.enabled.exists());
                assert!(!fixture.active.exists());
                assert!(!fixture.paths.socket.exists());
            }
            assert_no_backups(&fixture.paths.home);
        }
    }
}

#[tokio::test]
async fn uninstall_retries_while_a_valid_identity_survives() {
    for stage in UNINSTALL_STAGES {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();

        let error = uninstall_with_context(lifecycle_context_with_stage(&fixture, Some(stage)))
            .await
            .unwrap_err();

        if matches!(stage, "manifest-remove" | "binary-remove") {
            assert_injected_before_stage(&error, stage, "retry: codex-session-control uninstall");
        } else {
            assert_injected(&error, stage, "retry: codex-session-control uninstall");
        }
        assert!(fixture.paths.manifest.exists(), "{stage}");
        assert!(fixture.paths.binary.exists(), "{stage}");
        assert_no_backups(&fixture.paths.home);
        let report = uninstall_with_context(lifecycle_context_with_stage(&fixture, None))
            .await
            .unwrap();
        assert_eq!(
            report.stdout.lines().next(),
            Some("Codex app-server service: removed")
        );
        for removed in [
            &fixture.paths.unit,
            &fixture.paths.marketplace,
            &fixture.paths.config,
            &fixture.paths.manifest,
            &fixture.paths.binary,
        ] {
            assert!(!removed.exists(), "{stage}: {}", removed.display());
        }
        assert!(fixture.paths.codex_home.is_dir());
    }
}

#[tokio::test]
async fn uninstall_retry_crosses_the_exact_missing_managed_unit_boundary() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    let error = uninstall_with_context(lifecycle_context_with_stage(
        &fixture,
        Some("service-unit-remove"),
    ))
    .await
    .unwrap_err();

    assert_injected(
        &error,
        "service-unit-remove",
        "retry: codex-session-control uninstall",
    );
    assert!(!fixture.paths.unit.exists());
    assert!(fixture.paths.config.exists());
    assert!(fixture.paths.manifest.exists());
    assert!(fixture.paths.binary.exists());
    drop(authority);

    let report = uninstall_with_context(lifecycle_context_with_stage(&fixture, None))
        .await
        .unwrap();

    assert_eq!(
        report.stdout.lines().next(),
        Some("Codex app-server service: removed")
    );
    assert!(!fixture.paths.config.exists());
    assert!(!fixture.paths.manifest.exists());
    assert!(!fixture.paths.binary.exists());
    assert_eq!(
        fixture
            .systemctl_log()
            .matches("--user disable --now codex-session-control-test-Setup1.service")
            .count(),
        2
    );
}

#[tokio::test]
async fn missing_unit_retry_rejects_every_unproven_service_boundary() {
    for state in [
        "present",
        "unsafe",
        "enabled-unproven",
        "active",
        "activity-unproven",
        "socket",
    ] {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let unit = fs::read(&fixture.paths.unit).unwrap();
        uninstall_with_context(lifecycle_context_with_stage(
            &fixture,
            Some("service-unit-remove"),
        ))
        .await
        .unwrap_err();
        drop(authority);

        let mut socket_authority = None;
        match state {
            "present" | "unsafe" => {
                fs::write(&fixture.paths.unit, &unit).unwrap();
                fs::set_permissions(
                    &fixture.paths.unit,
                    fs::Permissions::from_mode(if state == "present" { 0o644 } else { 0o666 }),
                )
                .unwrap();
                fs::write(
                    &fixture.systemctl_fail,
                    "--user disable --now codex-session-control-test-Setup1.service",
                )
                .unwrap();
            }
            "enabled-unproven" => {
                fs::write(
                    &fixture.systemctl_fail,
                    "--user is-enabled codex-session-control-test-Setup1.service",
                )
                .unwrap();
            }
            "active" => {
                fs::write(&fixture.active, b"active").unwrap();
            }
            "activity-unproven" => {
                fs::write(
                    &fixture.systemctl_fail,
                    "--user is-active codex-session-control-test-Setup1.service",
                )
                .unwrap();
            }
            "socket" => {
                socket_authority =
                    Some(FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await);
            }
            _ => unreachable!(),
        }

        let error = uninstall_with_context(lifecycle_context_with_stage(&fixture, None))
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("failed at service-stop:"),
            "{state}: {error}"
        );
        assert!(fixture.paths.config.exists(), "{state}");
        assert!(fixture.paths.manifest.exists(), "{state}");
        assert!(fixture.paths.binary.exists(), "{state}");
        drop(socket_authority);
    }
}

#[tokio::test]
async fn binary_remove_failure_reports_terminal_partial_without_a_retry_or_full_receipt() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let binary_parent = fixture.paths.binary.parent().unwrap();
    fs::set_permissions(binary_parent, fs::Permissions::from_mode(0o500)).unwrap();

    let error = uninstall_with_context(lifecycle_context_with_stage(&fixture, None))
        .await
        .unwrap_err();

    fs::set_permissions(binary_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let error = error.to_string();
    assert!(error.contains("failed at binary-remove: terminal partial uninstall:"));
    assert!(error.contains(&format!(
        "remaining product executable: {}",
        fixture.paths.binary.display()
    )));
    assert!(error.contains("installed identity was removed; no fresh-process retry is available"));
    assert!(!error.contains("retry:"));
    assert!(!error.contains("Codex app-server service: removed"));
    assert!(!error.contains("Codex home preserved:"));
    assert!(!fixture.paths.config.exists());
    assert!(!fixture.paths.manifest.exists());
    assert!(!fixture.paths.data_root.exists());
    assert!(fixture.paths.binary.exists());
    assert_no_backups(&fixture.paths.home);
}
