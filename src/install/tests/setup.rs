use std::{fs, os::unix::fs::PermissionsExt};

use serde_json::{Value, json};

use super::support::{FakeAuthority, Fixture, assert_installed_modes};
use super::*;
use crate::model::InstalledRelease;

type MutationFileSnapshot = (PathBuf, Option<(Vec<u8>, u32)>);

#[test]
fn setup_guidance_precedence_is_exact() {
    use crate::cli_output::{
        DesktopAvailability as OutputDesktopAvailability, RunningClientFacts, SetupSuccess,
        UserSuccess,
    };

    let cases = [
        (
            RunningClientFacts {
                cli: true,
                desktop: false,
            },
            OutputDesktopAvailability::Unavailable,
            false,
            concat!(
                "Codex Session Control 1.2.3 is ready.\n\n",
                "Codex CLI is already running without Codex Session Control.\n",
                "Exit it, then start it with:\n",
                "  codex-session-control codex\n",
            ),
        ),
        (
            RunningClientFacts {
                cli: false,
                desktop: true,
            },
            OutputDesktopAvailability::Available,
            false,
            concat!(
                "Codex Session Control 1.2.3 is ready.\n\n",
                "To use Codex Session Control with Codex CLI, start the CLI with:\n",
                "  codex-session-control codex\n\n",
                "Codex Desktop is already running without Codex Session Control.\n",
                "Restart Codex Desktop to use Codex Session Control there.\n",
            ),
        ),
        (
            RunningClientFacts {
                cli: true,
                desktop: true,
            },
            OutputDesktopAvailability::Available,
            true,
            concat!(
                "Codex Session Control 1.2.3 is ready.\n\n",
                "Codex CLI is already running without Codex Session Control.\n",
                "Exit it, then start it with:\n",
                "  codex-session-control codex\n\n",
                "Codex Desktop is already running without Codex Session Control.\n",
                "Restart Codex Desktop to use Codex Session Control there.\n",
            ),
        ),
        (
            RunningClientFacts::default(),
            OutputDesktopAvailability::Available,
            true,
            concat!(
                "Codex Session Control 1.2.3 is ready.\n\n",
                "To use Codex Session Control with Codex CLI, start the CLI with:\n",
                "  codex-session-control codex\n\n",
                "If Codex Desktop is already running, restart it to make Codex Session Control available there.\n",
            ),
        ),
    ];

    for (running, desktop, changed, expected) in cases {
        let success = SetupSuccess::new(
            semver::Version::parse("1.2.3").unwrap(),
            running,
            desktop,
            changed,
            Vec::new(),
        )
        .unwrap();
        let rendered = UserSuccess::Setup(success).render();
        assert_eq!(rendered.stdout, expected);
        assert_eq!(
            rendered
                .stdout
                .matches("Codex CLI is already running")
                .count(),
            usize::from(running.cli)
        );
        assert_eq!(
            rendered
                .stdout
                .matches("Codex Desktop is already running without Codex Session Control")
                .count(),
            usize::from(running.desktop)
        );
    }

    for desktop in [
        OutputDesktopAvailability::Unavailable,
        OutputDesktopAvailability::CouldNotVerify,
    ] {
        assert!(
            SetupSuccess::new(
                semver::Version::parse("1.2.3").unwrap(),
                RunningClientFacts::default(),
                desktop,
                true,
                Vec::new(),
            )
            .is_none()
        );
    }
}

#[test]
fn setup_pure_failure_mappings_are_exact() {
    use crate::cli_output::{OrdinaryFailure, UserFailure};
    use crate::desktop::{DescriptorPublicationFailure, DescriptorPublicationResidue};

    assert_eq!(
        setup_invocation_failure(),
        UserFailure::Ordinary(OrdinaryFailure::SetupUnsafeTerminalRetry)
    );

    assert_eq!(
        setup_cli_reconciliation_failure(&ControllerError::Operational("sentinel".to_owned())),
        UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationRetry)
    );
    assert_eq!(
        setup_cli_reconciliation_failure(&ControllerError::InvalidData {
            field: "sentinel",
            reason: "sentinel",
        }),
        UserFailure::Ordinary(OrdinaryFailure::SetupCliIntegrationCheckStatus)
    );

    let clean = DescriptorPublicationFailure {
        source: ControllerError::Operational("sentinel".to_owned()),
        residue: None,
    };
    assert_eq!(
        setup_descriptor_publication_failure(clean),
        UserFailure::Ordinary(OrdinaryFailure::SetupDesktopIntegrationRetry)
    );
    for residue in [
        DescriptorPublicationResidue::Stage(PathBuf::from("/managed/stage")),
        DescriptorPublicationResidue::Final(PathBuf::from("/managed/final")),
    ] {
        assert!(matches!(
            setup_descriptor_publication_failure(DescriptorPublicationFailure {
                source: ControllerError::Operational("sentinel".to_owned()),
                residue: Some(residue),
            }),
            UserFailure::RollbackIncomplete(_)
        ));
    }
}

#[tokio::test]
async fn setup_default_and_verbose_are_behaviorally_identical() {
    use crate::diagnostics::{DiagnosticCommand, Diagnostics};

    let default_fixture = Fixture::new();
    let verbose_fixture = Fixture::new();
    let _default_authority =
        FakeAuthority::start(&default_fixture.paths, TESTED_CODEX_VERSION).await;
    let _verbose_authority =
        FakeAuthority::start(&verbose_fixture.paths, TESTED_CODEX_VERSION).await;
    let mut default_context = default_fixture.context(true);
    default_context.path_environment = std::env::join_paths([
        &default_fixture.fake_bin,
        &default_fixture.paths.home.join(".local/bin"),
    ])
    .unwrap();
    let mut verbose_context = verbose_fixture.context(true);
    verbose_context.path_environment = std::env::join_paths([
        &verbose_fixture.fake_bin,
        &verbose_fixture.paths.home.join(".local/bin"),
    ])
    .unwrap();
    let mut default_diagnostics = Diagnostics::new(false, DiagnosticCommand::Setup);
    let mut verbose_diagnostics = Diagnostics::record(DiagnosticCommand::Setup);

    let default = setup_with_context_and_diagnostics(default_context, &mut default_diagnostics)
        .await
        .map(|success| success.render())
        .unwrap();
    let verbose = setup_with_context_and_diagnostics(verbose_context, &mut verbose_diagnostics)
        .await
        .map(|success| success.render())
        .unwrap();

    let verbose = with_recorded_diagnostics(verbose, &verbose_diagnostics);

    assert_default_verbose_parity(&default, &verbose, &verbose_diagnostics, "[verbose] setup:");
    assert_eq!(
        default_fixture.systemctl_log(),
        verbose_fixture.systemctl_log()
    );
    assert!(default_fixture.paths.manifest.exists());
    assert!(verbose_fixture.paths.manifest.exists());
    assert_eq!(
        default_fixture.codex_log().lines().count(),
        verbose_fixture.codex_log().lines().count()
    );
}

#[tokio::test]
async fn setup_public_diagnostic_ownership_emits_controller_started_once() {
    use crate::diagnostics::{DiagnosticCommand, DiagnosticEvent, DiagnosticTarget, Diagnostics};

    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let mut diagnostics = Diagnostics::record(DiagnosticCommand::Setup);
    diagnostics.emit(DiagnosticEvent::ControllerStarted {
        version: semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
        target: DiagnosticTarget::current(),
    });

    setup_with_context_after_start(fixture.context(true), &mut diagnostics)
        .await
        .unwrap();

    assert_eq!(
        diagnostics
            .recorded_lines()
            .iter()
            .filter(|line| line.contains(": controller "))
            .count(),
        1
    );
}

fn snapshot_mutation_files(paths: impl IntoIterator<Item = PathBuf>) -> Vec<MutationFileSnapshot> {
    paths
        .into_iter()
        .map(|path| {
            let state = fs::symlink_metadata(&path).ok().map(|metadata| {
                (
                    fs::read(&path).unwrap(),
                    metadata.permissions().mode() & 0o7777,
                )
            });
            (path, state)
        })
        .collect()
}

#[tokio::test]
async fn first_install_writes_manifest_last_and_exact_receipt() {
    let fixture = Fixture::new();
    let _authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;

    let report = setup_with_context(fixture.context(false))
        .await
        .unwrap()
        .render();

    assert_eq!(
        report.stdout,
        format!(
            "Codex Session Control {} is ready.\n\n",
            env!("CARGO_PKG_VERSION")
        ) + "To use Codex Session Control with Codex CLI, start the CLI with:\n"
            + "  codex-session-control codex\n"
    );
    assert_eq!(
        report.stderr,
        format!(
            "Codex Desktop integration is unavailable because a compatible Desktop launcher was not found.\n\n\
Note: `{}` is not on your PATH.\n\
Add it to your PATH to use the short `codex-session-control` command.\n",
            fixture.paths.home.join(".local/bin").display()
        )
    );
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

    let report = setup_with_context(fixture.context(true))
        .await
        .unwrap()
        .render();
    let authority = starter.await.unwrap();

    assert!(report.stdout.starts_with("Codex Session Control "));
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

        assert_eq!(error.render().exit_code, 1, "{operation}");
        assert!(matches!(
            error,
            crate::cli_output::UserFailure::Ordinary(
                crate::cli_output::OrdinaryFailure::SetupServiceStateRetryUpdate
            )
        ));
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
        .unwrap_or_else(|failure| panic!("{failure:?}"));

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

    let second = setup_with_context(fixture.context(true))
        .await
        .unwrap()
        .render();
    assert!(second.stdout.starts_with("Codex Session Control "));
    assert!(!second.stderr.contains("Note:"));
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
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupInstalledStateCheckStatus
        )
    ));
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

    let report = setup_with_context(fixture.context(true))
        .await
        .unwrap()
        .render();

    assert!(report.stdout.starts_with("Codex Session Control "));
    assert!(fixture.paths.manifest.is_file());
}

#[tokio::test]
async fn manifestless_older_release_routes_to_its_exact_recovery() {
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

    assert_eq!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupInstalledStateRepair {
                binary: fixture.paths.binary.clone(),
            }
        )
    );
    assert!(!fixture.paths.manifest.exists());
    assert!(fixture.systemctl_log().is_empty());

    let plugin_fixture = Fixture::new();
    for directory in [
        plugin_fixture.paths.marketplace.clone(),
        plugin_fixture.paths.marketplace.join("plugins"),
        plugin_fixture
            .paths
            .marketplace
            .join("plugins/codex-session-control/.codex-plugin"),
    ] {
        create_shared_dir(&directory, plugin_fixture.paths.euid).unwrap();
    }
    let plugin = plugin_fixture
        .paths
        .marketplace
        .join("plugins/codex-session-control/.codex-plugin/plugin.json");
    fs::write(&plugin, br#"{"version":"0.0.9"}"#).unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o644)).unwrap();

    let error = setup_with_context(plugin_fixture.context(true))
        .await
        .unwrap_err();

    assert!(matches!(
        &error,
        crate::cli_output::UserFailure::VerifiedRelease(_)
    ));
    let rendered = error.render();
    assert!(rendered.stderr.contains("/releases/download/v0.0.9/"));
    assert!(rendered.stderr.contains("/v0.0.9/SHA256SUMS"));
    assert!(!plugin_fixture.paths.manifest.exists());
    assert!(plugin_fixture.systemctl_log().is_empty());
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

    assert!(
        matches!(
            error,
            crate::cli_output::UserFailure::Ordinary(
                crate::cli_output::OrdinaryFailure::SetupInstalledStateCheckStatus
            )
        ),
        "{error:?}"
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
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupInstallationFilesRetryUpdate
        )
    ));
    assert!(fixture.codex_log().lines().count() <= 3);
    assert!(fixture.systemctl_log().is_empty());

    fs::remove_file(&fixture.paths.manifest).unwrap();
    fs::write(&fixture.paths.unit, b"ambiguous unit").unwrap();
    fixture.clear_logs();
    let error = setup_with_context(fixture.context(true)).await.unwrap_err();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupInstalledStateCheckStatus
        )
    ));
    assert!(fixture.systemctl_log().is_empty());
}

#[tokio::test]
async fn invalid_persisted_desktop_identity_rejects_setup_before_every_mutation() {
    let fixture = Fixture::new();
    let authority = FakeAuthority::start(&fixture.paths, TESTED_CODEX_VERSION).await;
    let launcher = fixture._root.path().join("desktop-launcher");
    write_executable_fixture(
        &launcher,
        "#!/bin/sh\nif [ \"$1\" = \"--print-build-info\" ]; then printf '%s\\n' '{\"appIdentity\":{\"id\":\"codex-desktop\"},\"linuxCapabilities\":[\"external-app-server-attachment-descriptor-v1\"]}'; exit 0; fi\nexit 64\n",
    );
    let mut initial_setup = fixture.context(true);
    initial_setup.desktop_launcher = Some(launcher);
    setup_with_context(initial_setup).await.unwrap();
    let valid_manifest_bytes = fs::read(&fixture.paths.manifest).unwrap();
    let valid_manifest: Value = serde_json::from_slice(&valid_manifest_bytes).unwrap();
    let valid_attachment = valid_manifest["desktopAttachment"].clone();
    let valid_descriptor = PathBuf::from(valid_attachment["descriptorPath"].as_str().unwrap());

    for (case, invalid_attachment) in invalid_desktop_attachment_shapes(&valid_attachment) {
        let invalid_descriptor =
            PathBuf::from(invalid_attachment["descriptorPath"].as_str().unwrap());
        let mut invalid_manifest = valid_manifest.clone();
        invalid_manifest["desktopAttachment"] = invalid_attachment;
        let mut invalid_manifest_bytes = serde_json::to_vec_pretty(&invalid_manifest).unwrap();
        invalid_manifest_bytes.push(b'\n');
        fs::write(&fixture.paths.manifest, invalid_manifest_bytes).unwrap();
        let mutation_paths = [
            fixture.paths.binary.clone(),
            fixture.paths.config.clone(),
            fixture
                .paths
                .marketplace
                .join("plugins/codex-session-control/.mcp.json"),
            fixture
                .paths
                .marketplace
                .join(".agents/plugins/marketplace.json"),
            fixture
                .paths
                .marketplace
                .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
            fixture.paths.unit.clone(),
            fixture.paths.manifest.clone(),
            valid_descriptor.clone(),
            invalid_descriptor,
            fixture.marketplace_state.clone(),
            fixture.plugin_state.clone(),
            fixture.plugin_version_state.clone(),
            fixture.enabled.clone(),
            fixture.active.clone(),
            fixture.restart_requested.clone(),
        ];
        let before = snapshot_mutation_files(mutation_paths.clone());
        fixture.clear_logs();

        let error = setup_with_context(fixture.context(true)).await.unwrap_err();

        assert!(
            matches!(
                error,
                crate::cli_output::UserFailure::Ordinary(
                    crate::cli_output::OrdinaryFailure::SetupInstalledStateCheckStatus
                )
            ),
            "{case}: {error:?}"
        );
        assert_eq!(snapshot_mutation_files(mutation_paths), before, "{case}");
        assert!(fixture.systemctl_log().is_empty(), "{case}");
        assert!(fixture.codex_log().is_empty(), "{case}");
        assert!(fixture.paths.socket.exists(), "{case}");
    }

    fs::write(&fixture.paths.manifest, valid_manifest_bytes).unwrap();
    drop(authority);
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
async fn preflight_mismatch_and_stage_failure_never_write_manifest() {
    let candidate = Fixture::new();
    let mut candidate_context = candidate.context(true);
    candidate_context.candidate.target = "wrong-target".to_owned();
    let error = setup_with_context(candidate_context).await.unwrap_err();
    assert_eq!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupInstallationFilesRetry
        )
    );
    assert!(!candidate.paths.manifest.exists());
    assert!(candidate.systemctl_log().is_empty());

    let mismatch = Fixture::new();
    let untested_version = crate::test_support::different_stable_version(TESTED_CODEX_VERSION);
    let _authority = FakeAuthority::start(&mismatch.paths, &untested_version).await;
    let error = setup_with_context(mismatch.context(true))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupServiceStateRetryUpdate
        )
    ));
    assert!(!mismatch.paths.manifest.exists());
    assert!(!mismatch.systemctl_log().contains("restart"));

    let failed = Fixture::new();
    let _authority = FakeAuthority::start(&failed.paths, TESTED_CODEX_VERSION).await;
    fs::write(&failed.systemctl_fail, "--user daemon-reload").unwrap();
    let error = setup_with_context(failed.context(true)).await.unwrap_err();
    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupServiceConfigurationRetry
        )
    ));
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

    assert!(matches!(
        error,
        crate::cli_output::UserFailure::Ordinary(
            crate::cli_output::OrdinaryFailure::SetupServiceStateRetryUpdate
        )
    ));
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

        let report = setup_with_context(fixture.context(true))
            .await
            .unwrap()
            .render();

        assert_eq!(
            report.stdout,
            format!(
                "Codex Session Control {} is ready.\n\n",
                env!("CARGO_PKG_VERSION")
            ) + "To use Codex Session Control with Codex CLI, start the CLI with:\n"
                + "  codex-session-control codex\n"
        );
        let expected_stderr = if warning {
            let displayed_version = semver::Version::parse(
                version_output
                    .trim()
                    .strip_prefix("codex-cli ")
                    .unwrap_or(version_output.trim()),
            )
            .unwrap_or_else(|_| semver::Version::parse("0.0.0+unknown").unwrap());
            format!(
                "Warning: Codex {displayed_version} has not been tested with Codex Session Control {}.\n\
Some features may not work as expected.\n\n\
Codex Desktop integration is unavailable because a compatible Desktop launcher was not found.\n",
                env!("CARGO_PKG_VERSION")
            )
        } else {
            "Codex Desktop integration is unavailable because a compatible Desktop launcher was not found.\n"
                .to_owned()
        };
        assert_eq!(report.stderr, expected_stderr, "{version_output}");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.paths.manifest).unwrap()).unwrap();
        assert_eq!(manifest["schemaVersion"], 3);
        assert!(manifest.get("codexVersion").is_none());
    }
}
