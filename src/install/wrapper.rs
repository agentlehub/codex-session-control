use std::{ffi::OsString, path::Path, process::Command};

use crate::{
    app_server::AppServerClient,
    cli_output::UserFailure,
    diagnostics::{DiagnosticCause, Diagnostics},
    error::ControllerError,
    model::ProductConfig,
};

use super::{
    evidence::{ResolvedUserPaths, SelectedHomeOperation, require_selected_home_evidence},
    native::{read_codex_version, valid_executable},
    paths::{
        FileKind, lifecycle_file_error, read_product_evidence_file, validate_control_socket,
        validate_existing,
    },
};

const PREFLIGHT_STAGE: &str = "preflight";
const EXEC_STAGE: &str = "exec";

pub(crate) async fn codex_wrapper(
    args: Vec<OsString>,
    diagnostics: &mut Diagnostics,
) -> Result<(), UserFailure> {
    let paths = ResolvedUserPaths::from_effective_user()
        .map_err(|_| wrapper_failure(diagnostics, PREFLIGHT_STAGE, DiagnosticCause::Validation))?;
    let command = prepare_codex_wrapper(&paths, args, diagnostics).await?;
    exec_codex_wrapper_command(command, diagnostics)
}

pub(super) async fn prepare_codex_wrapper(
    paths: &ResolvedUserPaths,
    args: Vec<OsString>,
    diagnostics: &mut Diagnostics,
) -> Result<Command, UserFailure> {
    prepare_codex_wrapper_checked(paths, args)
        .await
        .map_err(|_| wrapper_failure(diagnostics, PREFLIGHT_STAGE, DiagnosticCause::Validation))
}

async fn prepare_codex_wrapper_checked(
    paths: &ResolvedUserPaths,
    args: Vec<OsString>,
) -> Result<Command, ControllerError> {
    let evidence = require_selected_home_evidence(paths, SelectedHomeOperation::Codex)?;
    let expected_config = evidence.configuration.ok_or(ControllerError::InvalidData {
        field: "configuration",
        reason: "valid schema-2 configuration is required",
    })?;
    let manifest = evidence.manifest.ok_or(ControllerError::InvalidData {
        field: "manifest",
        reason: "valid schema-2 or schema-3 installed manifest is required",
    })?;
    if expected_config.codex_executable != manifest.codex_executable
        || expected_config.codex_home != manifest.codex_home
        || expected_config.socket_path != manifest.socket_path
    {
        return Err(ControllerError::InvalidData {
            field: "configuration",
            reason: "contradicts installed manifest",
        });
    }
    let config_bytes = read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600)
        .map_err(|error| {
            ControllerError::Operational(lifecycle_file_error(&paths.config, error))
        })?;
    let config = std::str::from_utf8(&config_bytes)
        .ok()
        .and_then(|text| toml::from_str::<ProductConfig>(text).ok())
        .filter(|config| config.validate(&paths.codex_home, &paths.socket).is_ok())
        .ok_or(ControllerError::InvalidData {
            field: "configuration",
            reason: "is invalid",
        })?;
    if config != expected_config {
        return Err(ControllerError::InvalidData {
            field: "configuration",
            reason: "changed during validation",
        });
    }
    if !valid_executable(&config.codex_executable) {
        return Err(ControllerError::InvalidData {
            field: "Codex executable",
            reason: "is unavailable",
        });
    }
    let (_, expected_running_version) =
        read_codex_version(&config.codex_executable, &paths.codex_home)?;
    validate_existing(&paths.runtime_dir, FileKind::Directory, paths.euid)?;
    validate_control_socket(&paths.socket, paths.euid)?;
    let client = AppServerClient::new(
        paths.socket.clone(),
        paths.codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        expected_running_version,
    );
    let connection = client
        .connect_initialized()
        .await
        .map_err(|_| ControllerError::Operational("app-server initialize failed".to_owned()))?;
    if connection.compatibility_warning().is_some() {
        return Err(ControllerError::InvalidData {
            field: "running Codex version",
            reason: "differs from configured executable",
        });
    }
    let caller_cwd = std::env::current_dir().map_err(|_| ControllerError::InvalidData {
        field: "cwd",
        reason: "is unavailable",
    })?;
    Ok(build_codex_wrapper_command(
        &config.codex_executable,
        &paths.codex_home,
        &paths.socket,
        &caller_cwd,
        args,
    ))
}

fn build_codex_wrapper_command(
    codex: &Path,
    codex_home: &Path,
    socket: &Path,
    caller_cwd: &Path,
    args: Vec<OsString>,
) -> Command {
    let mut command = Command::new(codex);
    command
        .arg("--remote")
        .arg(format!("unix://{}", socket.display()))
        .arg("--cd")
        .arg(caller_cwd)
        .args(args)
        .env("CODEX_HOME", codex_home);
    command
}

pub(super) fn exec_codex_wrapper_command(
    mut command: Command,
    diagnostics: &mut Diagnostics,
) -> Result<(), UserFailure> {
    use std::os::unix::process::CommandExt;

    let _ = command.exec();
    Err(wrapper_failure(
        diagnostics,
        EXEC_STAGE,
        DiagnosticCause::Unexpected,
    ))
}

fn wrapper_failure(
    diagnostics: &mut Diagnostics,
    stage: &'static str,
    cause: DiagnosticCause,
) -> UserFailure {
    diagnostics.failed(stage, cause);
    UserFailure::WrapperUnavailable
}
