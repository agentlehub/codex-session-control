use std::{fs, os::unix::fs::PermissionsExt};

use serde_json::{Value, json};

use super::support::{FakeAuthority, Fixture, assert_installed_modes};
use super::*;
use crate::model::InstalledRelease;

#[tokio::test]
async fn first_install_writes_manifest_last_and_exact_receipt() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;

    let report = setup_with_context(fixture.context(false)).await.unwrap();

    assert_eq!(
        report.stdout,
        format!(
            "Installed release: {version}\n\
Codex app-server service: enabled, active\n\
Codex home: {home}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Desktop restart required: no\n\
Plugin: codex-session-control {version} at {plugin}\n\
Durable plugin state: current\n\
Loaded task state: may_be_stale\n\
New task required for guaranteed plugin convergence: yes\n\
\n\
Note: {local_bin} is not on PATH. Add it to PATH to use the short codex-session-control command.\n",
            version = env!("CARGO_PKG_VERSION"),
            plugin = fixture
                .paths
                .marketplace
                .join("plugins/codex-session-control")
                .display(),
            home = fixture.paths.codex_home.display(),
            local_bin = fixture.paths.home.join(".local/bin").display(),
        )
    );
    assert!(report.stderr.contains("completed: manifest\n"));
    assert_installed_modes(&fixture.paths);
    let manifest: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    manifest
        .validate(&fixture.paths.codex_home, &fixture.paths.socket)
        .unwrap();
    assert_eq!(manifest.product_version, env!("CARGO_PKG_VERSION"));
    assert!(fixture.codex_log().contains("plugin marketplace add "));
    assert!(
        fixture
            .codex_log()
            .contains("plugin add codex-session-control@codex-session-control-local --json")
    );
    let systemctl = fixture.systemctl_log();
    assert!(systemctl.contains("--user daemon-reload"));
    assert!(systemctl.contains("--user enable --now codex-session-control-test-Setup1.service"));
    assert!(!systemctl.contains("codex-session-control.service\n"));
}

#[tokio::test(start_paused = true)]
async fn first_install_waits_for_safe_socket_after_service_start_returns() {
    let fixture = Fixture::new();
    let paths = fixture.paths.clone();
    let active = fixture.active.clone();
    let starter = tokio::spawn(async move {
        while !active.exists() {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        FakeAuthority::start(&paths, TESTED_CODEX_VERSION).await
    });

    let report = setup_with_context(fixture.context(true)).await.unwrap();
    let authority = starter.await.unwrap();

    assert!(report.stderr.contains("completed: service-verify\n"));
    assert!(fixture.paths.socket.exists());
    drop(authority);
}

#[tokio::test]
async fn setup_verification_reports_untrustworthy_systemctl_state_as_operational_failure() {
    for operation in ["is-enabled", "is-active"] {
        let fixture = Fixture::new();
        let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
        fs::write(
            &fixture.systemctl_fail,
            format!("--user {operation} codex-session-control-test-Setup1.service"),
        )
        .unwrap();

        let error = setup_with_context(fixture.context(true)).await.unwrap_err();

        assert_eq!(error.exit_code(), 1, "{operation}");
        assert!(
            error.to_string().contains(&format!(
                "failed at service-verify: systemctl {operation} could not provide trustworthy service state\n"
            )),
            "{operation}: {error}"
        );
        assert!(
            error
                .to_string()
                .contains("retry: codex-session-control update\n"),
            "{operation}: {error}"
        );
        assert!(!fixture.paths.manifest.exists(), "{operation}");
        let systemctl = fixture.systemctl_log();
        assert!(
            systemctl.contains(&format!(
                "--user {operation} codex-session-control-test-Setup1.service\n"
            )),
            "{operation}: {systemctl}"
        );
        assert!(!systemctl.contains("--quiet"), "{operation}: {systemctl}");
    }
}

#[tokio::test]
async fn preflight_treats_absent_selected_home_as_empty_native_state() {
    let fixture = Fixture::new();
    fs::remove_dir_all(&fixture.paths.codex_home).unwrap();
    let mut context = fixture.context(true);

    setup_preflight(&mut context)
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.cause));

    assert_eq!(
        fixture.codex_log(),
        format!(
            "--version|CODEX_HOME={}\n",
            fixture.paths.codex_home.display()
        )
    );
    assert!(!fixture.paths.codex_home.exists());
    for path in [
        &fixture.paths.binary,
        &fixture.paths.config,
        &fixture.paths.marketplace,
        &fixture.paths.unit,
        &fixture.paths.manifest,
    ] {
        assert!(!path.exists(), "unexpected artifact: {}", path.display());
    }
}

#[tokio::test]
async fn setup_is_idempotent_blocks_invalid_identity_and_accepts_manifestless_matching_partial() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fixture.clear_logs();

    let second = setup_with_context(fixture.context(true)).await.unwrap();
    assert!(second.stdout.contains("Loaded task state: not_verified"));
    assert!(!second.stdout.contains("Note:"));
    assert!(!fixture.codex_log().contains(" marketplace add "));
    assert!(
        !fixture
            .codex_log()
            .contains("plugin add codex-session-control@codex-session-control-local --json")
    );

    let valid_config = fs::read(&fixture.paths.config).unwrap();
    fs::write(&fixture.paths.config, b"drift").unwrap();
    fs::set_permissions(&fixture.paths.config, fs::Permissions::from_mode(0o600)).unwrap();
    let error = setup_with_context(fixture.context(true)).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("selected-home identity is unavailable")
    );
    assert_eq!(fs::read(&fixture.paths.config).unwrap(), b"drift");
    fs::write(&fixture.paths.config, valid_config).unwrap();
    assert!(
        toml::from_str::<ProductConfig>(&fs::read_to_string(&fixture.paths.config).unwrap())
            .is_ok()
    );

    fs::remove_file(&fixture.paths.manifest).unwrap();
    setup_with_context(fixture.context(true)).await.unwrap();
    assert!(fixture.paths.manifest.is_file());

    for path in [
        fixture.paths.binary.clone(),
        fixture.paths.unit.clone(),
        fixture
            .paths
            .marketplace
            .join(".agents/plugins/marketplace.json"),
        fixture
            .paths
            .marketplace
            .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
        fixture
            .paths
            .marketplace
            .join("plugins/codex-session-control/.mcp.json"),
    ] {
        fs::write(&path, b"owned drift").unwrap();
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(if path == fixture.paths.binary {
                0o755
            } else {
                0o644
            }),
        )
        .unwrap();
        setup_with_context(fixture.context(true)).await.unwrap();
        assert_ne!(fs::read(&path).unwrap(), b"owned drift");
    }

    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    manifest["binarySha256"] =
        json!("0000000000000000000000000000000000000000000000000000000000000000");
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    manifest_bytes.push(b'\n');
    fs::write(&fixture.paths.manifest, manifest_bytes).unwrap();
    setup_with_context(fixture.context(true)).await.unwrap();
    let repaired: InstalledRelease =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    assert_ne!(
        repaired.binary_sha256,
        "0000000000000000000000000000000000000000000000000000000000000000"
    );

    for path in [
        fixture.paths.binary.clone(),
        fixture.paths.config.clone(),
        fixture.paths.unit.clone(),
        fixture.paths.manifest.clone(),
    ] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir_all(&fixture.paths.marketplace).unwrap();
    fs::remove_file(&fixture.marketplace_state).unwrap();
    fs::remove_file(&fixture.plugin_state).unwrap();
    setup_with_context(fixture.context(true)).await.unwrap();
    assert_installed_modes(&fixture.paths);
}

#[tokio::test]
async fn setup_service_verify_accepts_mode_0700_owner_only_socket() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    fs::set_permissions(&fixture.paths.socket, fs::Permissions::from_mode(0o700)).unwrap();

    let report = setup_with_context(fixture.context(true)).await.unwrap();

    assert!(
        report
            .stdout
            .contains("Codex app-server service: enabled, active")
    );
    assert!(fixture.paths.manifest.is_file());
}

#[tokio::test]
async fn manifestless_older_release_routes_to_its_exact_executable() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    create_shared_dir(fixture.paths.binary.parent().unwrap(), fixture.paths.euid).unwrap();
    super::write_executable_fixture(
        &fixture.paths.binary,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'codex-session-control 0.0.9 ({})\\n'; exit 0; fi\nexit 64\n",
            test_target()
        ),
    );

    let error = setup_with_context(fixture.context(true)).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("release 0.0.9 is partially installed without a manifest")
    );
    assert!(
        error
            .to_string()
            .contains(&format!("retry: {} setup", fixture.paths.binary.display()))
    );
    assert!(!fixture.paths.manifest.exists());
    assert!(fixture.systemctl_log().is_empty());
}

#[tokio::test]
async fn manifestless_unsafe_binary_is_never_executed_for_release_discovery() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    create_shared_dir(fixture.paths.binary.parent().unwrap(), fixture.paths.euid).unwrap();
    let marker = fixture._root.path().join("unsafe-binary-executed");
    let target = fixture._root.path().join("unsafe-binary-target");
    super::write_executable_fixture(
        &target,
        format!(
            "#!/bin/sh\nprintf executed > '{}'\nprintf 'codex-session-control 0.0.9 ({})\\n'\n",
            marker.display(),
            test_target()
        ),
    );
    std::os::unix::fs::symlink(&target, &fixture.paths.binary).unwrap();

    let error = setup_with_context(fixture.context(true)).await.unwrap_err();

    assert!(error.to_string().contains("ambiguous"));
    assert!(
        error
            .to_string()
            .contains(&fixture.paths.binary.display().to_string())
    );
    assert!(!marker.exists());
    assert!(!fixture.paths.manifest.exists());
    assert!(fixture.systemctl_log().is_empty());
}

#[test]
fn malformed_product_marketplace_entry_is_not_treated_as_absent() {
    let error = marketplace_roots(&json!({
        "marketplaces": [{
            "name": "codex-session-control-local"
        }]
    }))
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid marketplace: native list shape is invalid"
    );
}

#[tokio::test]
async fn different_manifest_and_ambiguous_partial_fail_before_mutation() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();

    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
    manifest["productVersion"] = json!("9.0.0");
    fs::write(
        &fixture.paths.manifest,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fixture.clear_logs();
    let error = setup_with_context(fixture.context(true)).await.unwrap_err();
    assert!(error.to_string().contains("failed at preflight:"));
    assert!(error.to_string().contains(" update"));
    assert!(fixture.codex_log().lines().count() <= 3);
    assert!(fixture.systemctl_log().is_empty());

    fs::remove_file(&fixture.paths.manifest).unwrap();
    fs::write(&fixture.paths.unit, b"ambiguous unit").unwrap();
    fixture.clear_logs();
    let error = setup_with_context(fixture.context(true)).await.unwrap_err();
    assert!(error.to_string().contains("ambiguous"));
    assert!(
        error
            .to_string()
            .contains(&fixture.paths.unit.display().to_string())
    );
    assert!(fixture.systemctl_log().is_empty());
}

#[tokio::test]
async fn exact_product_native_plugin_drift_is_repaired_without_marketplace_churn() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    setup_with_context(fixture.context(true)).await.unwrap();
    fs::write(&fixture.plugin_version_state, b"0.0.1").unwrap();
    fixture.clear_logs();

    setup_with_context(fixture.context(true)).await.unwrap();

    let log = fixture.codex_log();
    assert!(!log.contains("plugin marketplace remove "), "{log}");
    assert!(!log.contains("plugin marketplace add "), "{log}");
    assert!(log.contains("plugin remove codex-session-control@codex-session-control-local --json"));
    assert!(log.contains("plugin add codex-session-control@codex-session-control-local --json"));
}

#[tokio::test]
async fn running_version_mismatch_and_stage_failure_never_write_manifest() {
    let mismatch = Fixture::new();
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let _authority = FakeAuthority::start(&mismatch.paths, &untested_version).await;
    let error = setup_with_context(mismatch.context(true))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("failed at service-verify:"));
    assert!(error.to_string().contains(" update"));
    assert!(!mismatch.paths.manifest.exists());
    assert!(!mismatch.systemctl_log().contains("restart"));

    let failed = Fixture::new();
    let _authority = FakeAuthority::start(&failed.paths, TESTED_CODEX_VERSION).await;
    fs::write(&failed.systemctl_fail, "--user daemon-reload").unwrap();
    let error = setup_with_context(failed.context(true)).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "completed: preflight\n\
completed: binary\n\
completed: configuration\n\
completed: projection\n\
completed: plugin-marketplace\n\
completed: plugin-install\n\
completed: desktop-discovery\n\
completed: descriptor\n\
completed: service-unit\n\
failed at daemon-reload: systemctl command failed\n\
retry: codex-session-control setup\n"
    );
    assert!(!failed.paths.manifest.exists());
}

#[tokio::test]
async fn initialize_home_mismatch_fails_service_verify_without_manifest() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start_reporting(
        &fixture.paths,
        TESTED_CODEX_VERSION,
        fixture.paths.home.join("wrong-codex-home"),
    )
    .await;

    let error = setup_with_context(fixture.context(true)).await.unwrap_err();

    assert!(error.to_string().contains("failed at service-verify:"));
    assert!(!fixture.paths.manifest.exists());
    assert!(!fixture.systemctl_log().contains("restart"));
}

#[tokio::test]
async fn tested_untested_and_unparseable_versions_only_change_advisory() {
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let cases = [
        (
            format!("codex-cli {untested_version}\n"),
            untested_version,
            true,
        ),
        (
            TESTED_CODEX_CLI_VERSION_OUTPUT.to_owned(),
            TESTED_CODEX_VERSION.to_owned(),
            false,
        ),
        (
            "codex-cli not-semver\n".to_owned(),
            "not-semver".to_owned(),
            true,
        ),
    ];
    for (version_output, authority_version, warning) in cases {
        let fixture = Fixture::new();
        fs::write(&fixture.codex_version, &version_output).unwrap();
        let _authority = FakeAuthority::start(&fixture.paths, &authority_version).await;

        let report = setup_with_context(fixture.context(true)).await.unwrap();

        assert_eq!(
            report.stdout,
            format!(
                "Installed release: {version}\n\
Codex app-server service: enabled, active\n\
Codex home: {home}\n\
CLI attachment: available through codex-session-control codex\n\
Desktop attachment: unavailable\n\
Desktop restart required: no\n\
Plugin: codex-session-control {version} at {plugin}\n\
Durable plugin state: current\n\
Loaded task state: may_be_stale\n\
New task required for guaranteed plugin convergence: yes\n",
                version = env!("CARGO_PKG_VERSION"),
                plugin = fixture
                    .paths
                    .marketplace
                    .join("plugins/codex-session-control")
                    .display(),
                home = fixture.paths.codex_home.display(),
            )
        );
        let mut expected_stderr = "completed: preflight\n\
completed: binary\n\
completed: configuration\n\
completed: projection\n\
completed: plugin-marketplace\n\
completed: plugin-install\n\
completed: desktop-discovery\n\
completed: descriptor\n\
completed: service-unit\n\
completed: daemon-reload\n\
completed: service-enable\n\
completed: service-verify\n\
completed: manifest\n"
            .to_owned();
        if warning {
            let displayed_version = version_output
                .trim()
                .strip_prefix("codex-cli ")
                .unwrap_or(version_output.trim());
            expected_stderr.push_str(&format!(
                        "Compatibility warning: Codex app-server {displayed_version} has not been tested with codex-session-control {}; native results remain authoritative.\n",
                        env!("CARGO_PKG_VERSION")
                    ));
        }
        expected_stderr
            .push_str("Desktop attachment unavailable: codex-desktop.desktop was not found\n");
        assert_eq!(report.stderr, expected_stderr, "{version_output}");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 3);
        assert!(manifest.get("codexVersion").is_none());
    }
}
