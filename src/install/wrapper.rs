use std::{ffi::OsString, path::Path, process::Command};

use crate::{app_server::AppServerClient, error::ControllerError, model::ProductConfig};

use super::{
    evidence::{InstalledEvidenceCase, ResolvedUserPaths, require_selected_home_evidence},
    native::{read_codex_version, valid_executable},
    paths::{
        FileKind, lifecycle_file_error, read_product_evidence_file, validate_control_socket,
        validate_existing,
    },
};

pub(crate) async fn codex_wrapper(args: Vec<OsString>) -> Result<(), ControllerError> {
    let paths = ResolvedUserPaths::from_effective_user()?;
    let command = prepare_codex_wrapper(&paths, args).await?;
    exec_codex_wrapper_command(command)
}

pub(super) async fn prepare_codex_wrapper(
    paths: &ResolvedUserPaths,
    args: Vec<OsString>,
) -> Result<Command, ControllerError> {
    let unavailable = |error: ControllerError| wrapper_authority_unavailable(error.to_string());
    let evidence =
        require_selected_home_evidence(paths, &[InstalledEvidenceCase::CoherentV2], "codex")
            .map_err(unavailable)?;
    let expected_config = evidence.configuration.ok_or_else(|| {
        unavailable(ControllerError::InvalidData {
            field: "configuration",
            reason: "valid schema-2 configuration is required",
        })
    })?;
    let manifest = evidence.manifest.ok_or_else(|| {
        unavailable(ControllerError::InvalidData {
            field: "manifest",
            reason: "valid schema-2 installed manifest is required",
        })
    })?;
    if expected_config.codex_executable != manifest.codex_executable
        || expected_config.codex_home != manifest.codex_home
        || expected_config.socket_path != manifest.socket_path
    {
        return Err(wrapper_authority_unavailable(
            "stored configuration contradicts the installed manifest",
        ));
    }
    let config_bytes = read_product_evidence_file(&paths.home, paths.euid, &paths.config, 0o600)
        .map_err(|error| {
            wrapper_authority_unavailable(lifecycle_file_error(&paths.config, error))
        })?;
    let config = std::str::from_utf8(&config_bytes)
        .ok()
        .and_then(|text| toml::from_str::<ProductConfig>(text).ok())
        .filter(|config| config.validate(&paths.codex_home, &paths.socket).is_ok())
        .ok_or_else(|| wrapper_authority_unavailable("invalid installed configuration"))?;
    if config != expected_config {
        return Err(wrapper_authority_unavailable(
            "installed configuration changed during validation",
        ));
    }
    if !valid_executable(&config.codex_executable) {
        return Err(wrapper_authority_unavailable(
            "configured Codex executable is unavailable",
        ));
    }
    let (_, expected_running_version) =
        read_codex_version(&config.codex_executable, &paths.codex_home).map_err(unavailable)?;
    validate_existing(&paths.runtime_dir, FileKind::Directory, paths.euid).map_err(unavailable)?;
    validate_control_socket(&paths.socket, paths.euid).map_err(unavailable)?;
    let client = AppServerClient::new(
        paths.socket.clone(),
        paths.codex_home.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
        expected_running_version,
    );
    let connection = client
        .connect_initialized()
        .await
        .map_err(|_| wrapper_authority_unavailable("app-server initialize failed"))?;
    if connection.compatibility_warning().is_some() {
        return Err(wrapper_authority_unavailable(
            "running Codex version differs from configured executable",
        ));
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

fn wrapper_authority_unavailable(reason: impl std::fmt::Display) -> ControllerError {
    ControllerError::Operational(format!(
        "Codex authority is unavailable: {reason}\nRun codex-session-control status. If the service is disabled, run codex-session-control enable.\n"
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

pub(super) fn exec_codex_wrapper_command(mut command: Command) -> Result<(), ControllerError> {
    use std::os::unix::process::CommandExt;

    let error = command.exec();
    Err(ControllerError::Operational(format!(
        "cannot exec configured Codex: {error}"
    )))
}
