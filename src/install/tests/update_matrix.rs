use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
};

use crate::install::sha256_bytes;
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

fn higher_candidate(fixture: &Fixture, name: &str) -> PathBuf {
    candidate(
        fixture,
        name,
        "codex-session-control",
        &higher_test_release_version(),
        product_target(),
    )
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

fn context_with_snapshot_systemctl(
    fixture: &Fixture,
    candidate: PathBuf,
    name: &str,
    enablement: &str,
    activity: &str,
) -> (UpdateContext, PathBuf) {
    let systemctl_bin = fixture._root.path().join(name);
    let systemctl_log = fixture._root.path().join(format!("{name}.log"));
    fs::create_dir(&systemctl_bin).unwrap();
    super::write_executable_fixture(
        &systemctl_bin.join("systemctl"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{systemctl_log}'\nif [ \"$1\" = \"--user\" ] && [ \"$2\" = \"is-enabled\" ]; then\n{enablement}\nfi\nif [ \"$1\" = \"--user\" ] && [ \"$2\" = \"is-active\" ]; then\n{activity}\nfi\nexit 64\n",
            systemctl_log = systemctl_log.display(),
        ),
    );
    let setup = fixture.context(true);
    let path = std::env::join_paths(
        std::iter::once(systemctl_bin).chain(std::env::split_paths(&setup.path_environment)),
    )
    .unwrap();
    (context(fixture, candidate, Some(path)), systemctl_log)
}

fn manifest(paths: &ResolvedUserPaths) -> InstalledRelease {
    serde_json::from_slice(&fs::read(&paths.manifest).unwrap()).unwrap()
}

type FileSnapshot = (PathBuf, Option<(Vec<u8>, u32)>);

#[derive(Debug, Eq, PartialEq)]
enum SocketFileType {
    Socket,
    Directory,
    RegularFile,
    Symlink,
    Other,
}

#[derive(Debug, Eq, PartialEq)]
struct SocketIdentity {
    file_type: SocketFileType,
    mode: u32,
    inode: u64,
    uid: u32,
}

#[derive(Debug, Eq, PartialEq)]
struct ProtectedStateSnapshot {
    codex_home_sentinels: Vec<FileSnapshot>,
    enabled: FileSnapshot,
    active: FileSnapshot,
    socket: Option<SocketIdentity>,
}

#[derive(Debug, Eq, PartialEq)]
struct GuardedStateSnapshot {
    product_files: Vec<FileSnapshot>,
    restart_requested: FileSnapshot,
    protected: ProtectedStateSnapshot,
}

fn snapshot_file(path: PathBuf) -> FileSnapshot {
    let state = fs::symlink_metadata(&path)
        .ok()
        .map(|metadata| (fs::read(&path).unwrap(), metadata.mode() & 0o7777));
    (path, state)
}

fn snapshot_socket(path: &Path) -> Option<SocketIdentity> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => panic!("cannot snapshot control socket {}: {error}", path.display()),
    };
    let file_type = metadata.file_type();
    let file_type = if file_type.is_socket() {
        SocketFileType::Socket
    } else if file_type.is_dir() {
        SocketFileType::Directory
    } else if file_type.is_file() {
        SocketFileType::RegularFile
    } else if file_type.is_symlink() {
        SocketFileType::Symlink
    } else {
        SocketFileType::Other
    };
    Some(SocketIdentity {
        file_type,
        mode: metadata.mode() & 0o7777,
        inode: metadata.ino(),
        uid: metadata.uid(),
    })
}

fn protected_codex_home_sentinel_paths(fixture: &Fixture) -> [PathBuf; 5] {
    [
        fixture.paths.codex_home.join("auth.json"),
        fixture.paths.codex_home.join("tasks/tasks.db"),
        fixture.paths.codex_home.join("rollouts/rollout.jsonl"),
        fixture.paths.codex_home.join("config.toml"),
        fixture
            .paths
            .codex_home
            .join("plugins/unrelated/plugin.json"),
    ]
}

fn seed_protected_codex_home_state(fixture: &Fixture) {
    for (path, bytes, mode) in [
        (
            fixture.paths.codex_home.join("auth.json"),
            b"{\"access_token\":\"protected\"}\n".as_slice(),
            0o600,
        ),
        (
            fixture.paths.codex_home.join("tasks/tasks.db"),
            b"protected-task-state\n".as_slice(),
            0o640,
        ),
        (
            fixture.paths.codex_home.join("rollouts/rollout.jsonl"),
            b"{\"rollout\":\"protected\"}\n".as_slice(),
            0o600,
        ),
        (
            fixture.paths.codex_home.join("config.toml"),
            b"model = \"protected\"\n".as_slice(),
            0o600,
        ),
        (
            fixture
                .paths
                .codex_home
                .join("plugins/unrelated/plugin.json"),
            b"{\"plugin\":\"protected\"}\n".as_slice(),
            0o644,
        ),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}

fn snapshot_protected_state(fixture: &Fixture) -> ProtectedStateSnapshot {
    ProtectedStateSnapshot {
        codex_home_sentinels: protected_codex_home_sentinel_paths(fixture)
            .into_iter()
            .map(snapshot_file)
            .collect(),
        enabled: snapshot_file(fixture.enabled.clone()),
        active: snapshot_file(fixture.active.clone()),
        socket: snapshot_socket(&fixture.paths.socket),
    }
}

fn snapshot_guarded_state(fixture: &Fixture) -> GuardedStateSnapshot {
    GuardedStateSnapshot {
        product_files: [
            fixture.paths.binary.clone(),
            fixture.paths.config.clone(),
            fixture
                .paths
                .marketplace
                .join("plugins/codex-session-control/.mcp.json"),
            fixture
                .paths
                .marketplace
                .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
            fixture
                .paths
                .home
                .join(".config/codex-desktop/app-server-attachment.json"),
            fixture.paths.unit.clone(),
            fixture.paths.manifest.clone(),
        ]
        .into_iter()
        .map(snapshot_file)
        .collect(),
        restart_requested: snapshot_file(fixture.restart_requested.clone()),
        protected: snapshot_protected_state(fixture),
    }
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
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
async fn invalid_persisted_desktop_identity_rejects_update_before_every_mutation() {
    let fixture = Fixture::new();
    let (authority, _) = setup_attached(&fixture).await;
    seed_protected_codex_home_state(&fixture);
    let valid_manifest_bytes = fs::read(&fixture.paths.manifest).unwrap();
    let valid_manifest: Value = serde_json::from_slice(&valid_manifest_bytes).unwrap();
    let valid_attachment = valid_manifest["desktopAttachment"].clone();
    let candidate = higher_candidate(&fixture, "candidate-invalid-desktop-identity");

    for (case, invalid_attachment) in invalid_desktop_attachment_shapes(&valid_attachment) {
        let invalid_descriptor =
            PathBuf::from(invalid_attachment["descriptorPath"].as_str().unwrap());
        let mut invalid_manifest = valid_manifest.clone();
        invalid_manifest["desktopAttachment"] = invalid_attachment;
        let mut invalid_manifest_bytes = serde_json::to_vec_pretty(&invalid_manifest).unwrap();
        invalid_manifest_bytes.push(b'\n');
        fs::write(&fixture.paths.manifest, invalid_manifest_bytes).unwrap();
        let before = snapshot_guarded_state(&fixture);
        let invalid_descriptor_before = snapshot_file(invalid_descriptor);
        fixture.clear_logs();

        let error = staged_update_with_context(context(&fixture, candidate.clone(), None))
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("failed at candidate-preflight:"),
            "{case}: {error}"
        );
        assert_eq!(snapshot_guarded_state(&fixture), before, "{case}");
        assert_eq!(
            snapshot_file(invalid_descriptor_before.0.clone()),
            invalid_descriptor_before
        );
        assert!(fixture.systemctl_log().is_empty(), "{case}");
        assert!(fixture.codex_log().is_empty(), "{case}");
    }

    fs::write(&fixture.paths.manifest, valid_manifest_bytes).unwrap();
    drop(authority);
}

#[tokio::test]
async fn coherent_equal_candidate_reports_current_only_after_state_proof() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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

#[tokio::test]
async fn self_hosted_restart_required_update_refuses_with_every_reason_before_mutation() {
    let fixture = Fixture::new();
    let running = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    seed_protected_codex_home_state(&fixture);
    fs::write(
        &fixture.whoami_unit,
        b"codex-session-control-test-Setup1.service\n",
    )
    .unwrap();
    let old_unit = b"coherent old unit\n";
    fs::write(&fixture.paths.unit, old_unit).unwrap();
    let mut receipt = manifest(&fixture.paths);
    receipt.service_unit_sha256 = sha256_bytes(old_unit);
    let mut receipt_bytes = serde_json::to_vec_pretty(&receipt).unwrap();
    receipt_bytes.push(b'\n');
    fs::write(&fixture.paths.manifest, receipt_bytes).unwrap();
    let changed_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    fs::write(
        &fixture.codex_version,
        format!("codex-cli {changed_version}\n"),
    )
    .unwrap();
    let new_bin = fixture.paths.home.join("new-codex-bin");
    fs::create_dir(&new_bin).unwrap();
    fs::copy(fixture.fake_bin.join("codex"), new_bin.join("codex")).unwrap();
    let setup = fixture.context(true);
    let path = std::env::join_paths(
        std::iter::once(new_bin).chain(std::env::split_paths(&setup.path_environment)),
    )
    .unwrap();
    let before = snapshot_guarded_state(&fixture);
    fixture.clear_logs();

    let error = staged_update_with_context(context(
        &fixture,
        higher_candidate(&fixture, "candidate-all-reasons"),
        Some(path),
    ))
    .await
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("running Codex version differs"));
    assert!(message.contains("resolved Codex executable path differs"));
    assert!(message.contains("rendered systemd unit differs"));
    assert!(
        message.find("running Codex version").unwrap()
            < message.find("resolved Codex executable path").unwrap()
    );
    assert!(
        message.find("resolved Codex executable path").unwrap()
            < message.find("rendered systemd unit").unwrap()
    );
    assert!(message.contains("run from an independent terminal"));
    assert!(message.contains("running inside the managed app-server"));
    assert_eq!(snapshot_guarded_state(&fixture), before);
    assert!(!fixture.systemctl_log().contains("daemon-reload"));
    assert!(!fixture.systemctl_log().contains(" restart "));
    drop(running);
}

#[tokio::test]
async fn unproven_self_hosted_restart_refuses_without_stop_recovery() {
    let fixture = Fixture::new();
    let running = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    seed_protected_codex_home_state(&fixture);
    let new_bin = fixture.paths.home.join("new-codex-bin");
    fs::create_dir(&new_bin).unwrap();
    fs::copy(fixture.fake_bin.join("codex"), new_bin.join("codex")).unwrap();
    let setup = fixture.context(true);
    let path = std::env::join_paths(
        std::iter::once(new_bin).chain(std::env::split_paths(&setup.path_environment)),
    )
    .unwrap();
    fs::write(&fixture.systemctl_fail, "--user whoami").unwrap();
    let before = snapshot_guarded_state(&fixture);
    fixture.clear_logs();

    let error = staged_update_with_context(context(
        &fixture,
        higher_candidate(&fixture, "candidate-unproven-caller"),
        Some(path),
    ))
    .await
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("failed at restart-inspection:"));
    assert!(message.contains("repair or upgrade the systemd user environment"));
    assert!(!message.contains("systemctl --user stop"));
    assert!(!message.contains("codex-session-control disable\ncodex-session-control update"));
    assert_eq!(snapshot_guarded_state(&fixture), before);
    assert!(!fixture.systemctl_log().contains("daemon-reload"));
    assert!(!fixture.systemctl_log().contains(" restart "));
    drop(running);
}

#[tokio::test]
async fn service_snapshot_rejects_unproven_and_contradictory_state_before_caller_inspection() {
    for (name, enablement, activity, expected) in [
        (
            "snapshot-command-failure",
            "exit 1",
            "printf 'inactive\\n'; exit 3",
            "service enablement cannot be proven",
        ),
        (
            "snapshot-activating",
            "printf 'enabled\\n'; exit 0",
            "printf 'activating\\n'; exit 0",
            "service activity cannot be proven",
        ),
    ] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        seed_protected_codex_home_state(&fixture);
        let before = snapshot_guarded_state(&fixture);
        fixture.clear_logs();

        let (context, systemctl_log) = context_with_snapshot_systemctl(
            &fixture,
            higher_candidate(&fixture, &format!("candidate-{name}")),
            name,
            enablement,
            activity,
        );
        let error = staged_update_with_context(context).await.unwrap_err();

        assert!(
            error.to_string().contains("failed at service-snapshot:"),
            "{name}"
        );
        assert!(error.to_string().contains(expected), "{name}: {error}");
        assert_eq!(snapshot_guarded_state(&fixture), before, "{name}");
        let log = fs::read_to_string(systemctl_log).unwrap();
        assert!(!log.contains("--user whoami"), "{name}");
        assert!(!log.contains("daemon-reload"), "{name}");
    }
}

#[tokio::test]
async fn absent_inactive_service_update_repairs_files_without_starting_the_service() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    drop(authority);
    fs::remove_file(&fixture.enabled).unwrap();
    fs::remove_file(&fixture.active).unwrap();
    fs::remove_file(&fixture.paths.socket).unwrap();
    fs::remove_file(&fixture.paths.unit).unwrap();
    fixture.clear_logs();

    let report = staged_update_with_context(context(
        &fixture,
        higher_candidate(&fixture, "candidate-absent-inactive"),
        None,
    ))
    .await
    .unwrap();

    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        higher_test_release_version()
    )));
    assert!(fixture.paths.unit.exists());
    assert!(!fixture.enabled.exists());
    assert!(!fixture.active.exists());
    assert!(!fixture.paths.socket.exists());
    assert!(!fixture.systemctl_log().contains("--user whoami"));
    assert!(!fixture.systemctl_log().contains(" restart "));
    assert!(!fixture.systemctl_log().contains(" enable --now "));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn higher_candidate_preserves_all_three_desired_service_states() {
    for (name, enabled, active) in DESIRED_STATES {
        let fixture = Fixture::new();
        let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
        let candidate = higher_candidate(&fixture, "candidate-higher");
        fixture.clear_logs();

        let starter = if enabled && !active {
            fs::write(&fixture.wait_for_socket, b"wait").unwrap();
            let paths = fixture.paths.clone();
            let active_path = fixture.active.clone();
            Some(tokio::spawn(async move {
                while !active_path.exists() {
                    tokio::task::yield_now().await;
                }
                FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
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
            report.stdout.starts_with(&format!(
                "Installed release: {}\n",
                higher_test_release_version()
            )),
            "{name}"
        );
        assert_eq!(
            manifest(&fixture.paths).product_version,
            higher_test_release_version()
        );
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
    seed_protected_codex_home_state(&fixture);
    fs::write(
        &fixture.whoami_unit,
        b"codex-session-control-test-Setup1.service\n",
    )
    .unwrap();
    let descriptor = fs::read(&attachment.descriptor_path).unwrap();
    let before = snapshot_protected_state(&fixture);
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
    assert_eq!(snapshot_protected_state(&fixture), before);
    assert_eq!(
        fs::symlink_metadata(&fixture.paths.socket).unwrap().ino(),
        socket_inode
    );
    assert!(!fixture.systemctl_log().contains("--user whoami"));
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
        stages.trim_end().ends_with("completed: manifest"),
        "{stages}"
    );
    let receipt: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    assert_eq!(receipt["schemaVersion"], 3);
    assert!(receipt.get("codexVersion").is_none());
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
        FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
    });
    let setup = fixture.context(true);
    let enabled = enable_with_context(LifecycleContext {
        target: setup.target,
        path_environment: setup.path_environment,
        desktop_environment: setup.desktop_environment,
        cwd: setup.cwd,
    })
    .await
    .unwrap()
    .render();
    let authority = starter.await.unwrap();

    assert_eq!(
        fs::read(&attachment.descriptor_path).unwrap(),
        render_descriptor(&fixture.paths.socket).unwrap()
    );
    assert!(enabled.stdout.contains(
        "If Codex Desktop is already running, restart it to make Codex Session Control available there."
    ));
    drop(authority);
}

#[tokio::test]
async fn null_desktop_update_never_auto_selects_a_new_compatible_launcher() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        seed_protected_codex_home_state(&fixture);
        if case == "disabled-active" {
            fs::remove_file(&fixture.enabled).unwrap();
        } else {
            fs::write(&fixture.paths.unit, b"contradictory unit").unwrap();
        }
        let before = snapshot_guarded_state(&fixture);
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
        assert_eq!(snapshot_guarded_state(&fixture), before, "{case}");
        if case == "disabled-active" {
            assert!(!fixture.systemctl_log().contains("--user whoami"));
        }
        assert!(!fixture.systemctl_log().contains("daemon-reload"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_changed_authority_inspects_then_restarts_exactly_once() {
    let fixture = Fixture::new();
    let old_authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
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
    let candidate = higher_candidate(&fixture, "candidate-changed");
    fs::write(&fixture.wait_for_socket, b"wait").unwrap();
    fixture.clear_logs();
    let paths = fixture.paths.clone();
    let restart_requested = fixture.restart_requested.clone();
    let starter = tokio::spawn(async move {
        while !restart_requested.exists() {
            tokio::task::yield_now().await;
        }
        FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
    });

    let report = staged_update_with_context(context(&fixture, candidate, Some(path)))
        .await
        .unwrap();
    let new_authority = starter.await.unwrap();

    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        higher_test_release_version()
    )));
    assert_eq!(manifest(&fixture.paths).codex_executable, new_codex);
    assert_eq!(fixture.systemctl_log().matches(" restart ").count(), 1);
    assert!(!fixture.codex_log().contains("thread/interrupt"));
    drop(new_authority);
    drop(old_authority);
}

#[tokio::test]
async fn independently_changed_codex_version_does_not_depend_on_installed_receipt() {
    let tested = semver::Version::parse(TESTED_CODEX_VERSION).unwrap();
    let lower = if tested.patch > 0 {
        semver::Version::new(tested.major, tested.minor, tested.patch - 1)
    } else if tested.minor > 0 {
        semver::Version::new(tested.major, tested.minor - 1, 0)
    } else if tested.major > 0 {
        semver::Version::new(tested.major - 1, 0, 0)
    } else {
        panic!("tested Codex version must have a lower stable version")
    };
    let versions = [
        lower.to_string(),
        crate::test_support::different_stable_version(TESTED_CODEX_VERSION),
    ];

    for version in versions {
        let fixture = Fixture::new();
        let original_authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        drop(original_authority);
        fs::remove_file(&fixture.paths.socket).unwrap();
        fs::write(&fixture.codex_version, format!("codex-cli {version}\n")).unwrap();
        let changed_authority = FakeAuthority::start(&fixture.paths, &version).await;
        let candidate = higher_candidate(&fixture, "candidate-independent-codex-change");
        fixture.clear_logs();

        let report = staged_update_with_context(context(&fixture, candidate, None))
            .await
            .unwrap();

        assert!(report.stdout.starts_with(&format!(
            "Installed release: {}\n",
            higher_test_release_version()
        )));
        assert_eq!(fixture.systemctl_log().matches(" restart ").count(), 0);
        let receipt: Value =
            serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
        assert_eq!(receipt["schemaVersion"], 3);
        assert!(receipt.get("codexVersion").is_none());
        drop(changed_authority);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_running_codex_is_restarted_to_match_the_current_executable() {
    let fixture = Fixture::new();
    let running_authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let current_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    fs::write(
        &fixture.codex_version,
        format!("codex-cli {current_version}\n"),
    )
    .unwrap();
    let candidate = higher_candidate(&fixture, "candidate-stale-running-codex");
    fs::write(&fixture.wait_for_socket, b"wait").unwrap();
    fixture.clear_logs();
    let paths = fixture.paths.clone();
    let restart_requested = fixture.restart_requested.clone();
    let version_after_restart = current_version.clone();
    let starter = tokio::spawn(async move {
        while !restart_requested.exists() {
            tokio::task::yield_now().await;
        }
        FakeAuthority::start(&paths, &version_after_restart).await
    });

    let report = staged_update_with_context(context(&fixture, candidate, None))
        .await
        .unwrap();
    let restarted_authority = starter.await.unwrap();

    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        higher_test_release_version()
    )));
    assert_eq!(fixture.systemctl_log().matches(" restart ").count(), 1);
    let receipt: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    assert_eq!(receipt["schemaVersion"], 3);
    assert!(receipt.get("codexVersion").is_none());
    drop(restarted_authority);
    drop(running_authority);
}

#[tokio::test]
async fn retry_uses_last_manifest_after_partial_candidate_files() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    let candidate = higher_candidate(&fixture, "candidate-retry");
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
    assert!(report.stdout.starts_with(&format!(
        "Installed release: {}\n",
        higher_test_release_version()
    )));
    assert_eq!(
        manifest(&fixture.paths).product_version,
        higher_test_release_version()
    );
}

#[tokio::test]
async fn tested_untested_and_unparseable_versions_only_change_update_advisory() {
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let cases = [
        (
            TESTED_CODEX_CLI_VERSION_OUTPUT.to_owned(),
            TESTED_CODEX_VERSION.to_owned(),
            None,
        ),
        (
            format!("codex-cli {untested_version}\n"),
            untested_version.clone(),
            Some(untested_version),
        ),
        (
            "codex-cli not-semver\n".to_owned(),
            "not-semver".to_owned(),
            Some("not-semver".to_owned()),
        ),
    ];
    for (version_output, authority_version, warning) in cases {
        let fixture = Fixture::new();
        fs::write(&fixture.codex_version, &version_output).unwrap();
        let _authority = FakeAuthority::start(&fixture.paths, &authority_version).await;
        setup_with_context(fixture.context(true)).await.unwrap();
        let candidate = higher_candidate(&fixture, "candidate-advisory");
        fixture.clear_logs();

        let report = staged_update_with_context(context(&fixture, candidate, None))
            .await
            .unwrap();

        assert!(report.stdout.starts_with(&format!(
            "Installed release: {}\n",
            higher_test_release_version()
        )));
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
        let receipt: Value =
            serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
        assert_eq!(receipt["schemaVersion"], 3);
        assert!(receipt.get("codexVersion").is_none());
    }
}

#[test]
fn candidate_apply_sets_only_the_private_staged_marker() {
    let root = crate::test_support::private_tempdir();
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
