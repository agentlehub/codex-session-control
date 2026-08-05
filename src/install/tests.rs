use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use uzers::os::unix::UserExt;

use crate::{
    desktop::render_descriptor,
    error::ControllerError,
    model::{DesktopAttachmentIdentity, InstalledRelease, ProductConfig},
};

use super::{
    CandidateRelease, DESKTOP_DETACH_GUIDANCE, LifecycleContext,
    enable_disable::{disable_with_context, enable_with_context},
    evidence::{
        InstalledEvidenceCase, InvalidEvidence, NativeProductResidue, ResolvedUserPaths,
        StoredEvidence, classify_selected_home_evidence,
        classify_selected_home_evidence_with_native_product_artifact, load_config_from_paths,
        read_configuration_evidence, read_manifest_evidence, require_selected_home_evidence,
        select_first_install_codex_home, selected_codex_home,
    },
    native::marketplace_roots,
    paths::{
        FileKind, SOCKET_SECURITY_REQUIREMENT, StatusFileError, atomic_write,
        create_missing_selected_codex_home, create_product_dir, create_shared_dir,
        remove_owned_empty_dir, resolve_codex_executable, shell_quote_path, validate_config_file,
        validate_control_socket, validate_existing,
    },
    persisted_codex_version, product_target,
    release::{
        RELEASE_CONNECT_TIMEOUT, RELEASE_METADATA_TIMEOUT, RELEASE_TRANSFER_IDLE_TIMEOUT,
        ReleaseAsset, ReleaseEndpoints, ReleaseStage, build_release_client,
        discover_latest_release, download_verified_release, production_release_endpoints,
        release_target_for_arch, stream_release_asset, validate_checksum_entry,
        with_release_stage_timeout,
    },
    render::{render_projection, render_unit},
    service::{
        CONTROL_SOCKET_READINESS_TIMEOUT, LifecycleTarget, append_unattached_client_guidance,
        classify_unattached_client, detect_running_unattached_clients_from_snapshot,
        wait_for_control_socket,
    },
    setup::{SetupContext, setup_preflight, setup_with_context},
    status::{StatusContext, status_with_context},
    test_target,
    uninstall::uninstall_with_context,
    update::{
        TerminalState, UpdateContext, baseline_active_turn_gate, list_active_threads,
        outer_update_with_endpoints, run_candidate_apply, staged_update_with_context,
    },
    wrapper::{exec_codex_wrapper_command, prepare_codex_wrapper},
};

fn write_executable_fixture(path: &std::path::Path, contents: impl AsRef<[u8]>) {
    use std::os::unix::fs::PermissionsExt;

    let stage = path.with_extension("stage");
    std::fs::write(&stage, contents).unwrap();
    std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::File::open(&stage).unwrap().sync_all().unwrap();
    std::fs::rename(stage, path).unwrap();
}

fn assert_disposable_systemd_fixture_path(path: &std::path::Path, expected: FileKind, euid: u32) {
    validate_existing(path, expected, euid).unwrap_or_else(|error| {
        panic!(
            "disposable systemd fixture preflight rejected {}: {error}",
            path.display()
        )
    });
}

fn systemd_helper_initialize_response(
    id: &serde_json::Value,
    codex_home: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "result": {
            "codexHome": codex_home,
            "userAgent": "codex-cli 0.146.0"
        }
    })
}

#[test]
fn systemd_helper_reports_codex_home_as_a_protocol_string() {
    let response =
        systemd_helper_initialize_response(&serde_json::json!(7), "/home/disposable/.codex");

    assert_eq!(
        response,
        serde_json::json!({
            "id": 7,
            "result": {
                "codexHome": "/home/disposable/.codex",
                "userAgent": "codex-cli 0.146.0"
            }
        })
    );
    assert!(response["result"]["codexHome"].is_string());
}

mod systemd;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly authorized disposable systemd user and dbus session"]
async fn disposable_systemd_user() {
    systemd::run_disposable_systemd_user().await;
}

mod active_turn_gate;
mod codex_wrapper;
mod config_loader;
mod desktop_start_lifecycle;
mod desktop_stop_lifecycle;
mod enable_disable;
mod failure_retry;
mod normal_home_setup;
mod paths;
mod release;
mod render;
mod selected_home_evidence;
mod setup;
mod status;
mod support;
mod uninstall;
mod update_matrix;
