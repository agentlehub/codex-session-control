use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::Value;

use crate::error::ControllerError;

use super::paths::{FileKind, shell_quote_path, validate_existing};

pub(super) fn read_installed_product_version(path: &Path, euid: u32) -> Option<String> {
    if !valid_owned_executable(path, euid) {
        return None;
    }
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let output = String::from_utf8(output.stdout).ok()?;
    let version = output
        .trim()
        .strip_prefix("codex-session-control ")?
        .split_whitespace()
        .next()?;
    semver::Version::parse(version)
        .ok()
        .map(|version| version.to_string())
}

pub(super) fn valid_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
}

pub(super) fn valid_owned_executable(path: &Path, euid: u32) -> bool {
    validate_existing(path, FileKind::RegularFile, euid).is_ok()
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

pub(super) fn resolve_named_executable(
    path_environment: &OsStr,
    cwd: &Path,
    name: &str,
) -> Result<PathBuf, ControllerError> {
    for directory in std::env::split_paths(path_environment) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            cwd.join(directory)
        };
        let candidate = directory.join(name);
        if valid_executable(&candidate) {
            return Ok(candidate);
        }
    }
    Err(ControllerError::InvalidData {
        field: "PATH",
        reason: "required executable is unavailable",
    })
}

pub(super) fn read_codex_version(
    codex: &Path,
    codex_home: &Path,
) -> Result<(String, String), ControllerError> {
    let output = Command::new(codex)
        .arg("--version")
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| ControllerError::InvalidData {
            field: "codex_version",
            reason: "cannot execute Codex",
        })?;
    if !output.status.success() {
        return Err(ControllerError::InvalidData {
            field: "codex_version",
            reason: "Codex version command failed",
        });
    }
    let raw = String::from_utf8(output.stdout).map_err(|_| ControllerError::InvalidData {
        field: "codex_version",
        reason: "Codex version output is not UTF-8",
    })?;
    let display = raw
        .trim()
        .strip_prefix("codex-cli ")
        .unwrap_or(raw.trim())
        .to_owned();
    if display.is_empty() {
        return Err(ControllerError::InvalidData {
            field: "codex_version",
            reason: "Codex version output is empty",
        });
    }
    let expected = semver::Version::parse(&display)
        .map(|version| version.to_string())
        .unwrap_or_else(|_| "unknown".to_owned());
    Ok((display, expected))
}

pub(super) fn run_codex_json(
    codex: &Path,
    codex_home: &Path,
    arguments: &[&OsStr],
) -> Result<Value, ControllerError> {
    let output = Command::new(codex)
        .args(arguments)
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| ControllerError::InvalidData {
            field: "codex_command",
            reason: "cannot execute Codex",
        })?;
    if !output.status.success() {
        return Err(ControllerError::InvalidData {
            field: "codex_command",
            reason: "Codex command failed",
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|_| ControllerError::InvalidData {
        field: "codex_command",
        reason: "Codex JSON output is invalid",
    })
}

pub(super) fn reconcile_marketplace(
    codex: &Path,
    codex_home: &Path,
    marketplace: &Path,
) -> Result<bool, ControllerError> {
    let list = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("marketplace"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let product_roots = marketplace_roots(&list)?;
    let expected = marketplace.to_str().ok_or(ControllerError::InvalidData {
        field: "marketplace",
        reason: "path must be UTF-8",
    })?;
    if product_roots.len() == 1 && product_roots[0] == expected {
        return Ok(false);
    }
    if !product_roots.is_empty() {
        return Err(ControllerError::InvalidData {
            field: "marketplace",
            reason: "foreign native product source",
        });
    }
    run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("marketplace"),
            OsStr::new("add"),
            marketplace.as_os_str(),
            OsStr::new("--json"),
        ],
    )?;
    let verified = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("marketplace"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    if marketplace_roots(&verified)? != [expected] {
        return Err(ControllerError::InvalidData {
            field: "marketplace",
            reason: "native marketplace verification failed",
        });
    }
    Ok(true)
}

pub(super) fn marketplace_roots(value: &Value) -> Result<Vec<&str>, ControllerError> {
    let marketplaces = value.get("marketplaces").and_then(Value::as_array).ok_or(
        ControllerError::InvalidData {
            field: "marketplace",
            reason: "native list shape is invalid",
        },
    )?;
    let mut roots = Vec::new();
    for marketplace in marketplaces {
        if marketplace.get("name").and_then(Value::as_str) != Some("codex-session-control-local") {
            continue;
        }
        roots.push(marketplace.get("root").and_then(Value::as_str).ok_or(
            ControllerError::InvalidData {
                field: "marketplace",
                reason: "native list shape is invalid",
            },
        )?);
    }
    Ok(roots)
}

pub(super) fn reconcile_plugin(
    codex: &Path,
    codex_home: &Path,
    marketplace: &Path,
    product_version: &str,
) -> Result<bool, ControllerError> {
    let expected_source = marketplace
        .join("plugins/codex-session-control")
        .to_str()
        .ok_or(ControllerError::InvalidData {
            field: "plugin",
            reason: "source path must be UTF-8",
        })?
        .to_owned();
    let list = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let product = product_plugins(&list)?;
    if product.len() == 1 && plugin_matches(product[0], product_version, &expected_source) {
        return Ok(false);
    }
    if !product.is_empty() {
        if product
            .iter()
            .any(|plugin| !plugin_uses_source(plugin, &expected_source))
        {
            return Err(ControllerError::InvalidData {
                field: "plugin",
                reason: "foreign native product source",
            });
        }
        run_codex_json(
            codex,
            codex_home,
            &[
                OsStr::new("plugin"),
                OsStr::new("remove"),
                OsStr::new("codex-session-control@codex-session-control-local"),
                OsStr::new("--json"),
            ],
        )?;
    }
    run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("add"),
            OsStr::new("codex-session-control@codex-session-control-local"),
            OsStr::new("--json"),
        ],
    )?;
    let verified = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let product = product_plugins(&verified)?;
    if product.len() != 1 || !plugin_matches(product[0], product_version, &expected_source) {
        return Err(ControllerError::InvalidData {
            field: "plugin",
            reason: "native plugin verification failed",
        });
    }
    Ok(true)
}

pub(super) fn product_plugins(value: &Value) -> Result<Vec<&Value>, ControllerError> {
    let installed =
        value
            .get("installed")
            .and_then(Value::as_array)
            .ok_or(ControllerError::InvalidData {
                field: "plugin",
                reason: "native list shape is invalid",
            })?;
    Ok(installed
        .iter()
        .filter(|plugin| {
            plugin.get("pluginId").and_then(Value::as_str)
                == Some("codex-session-control@codex-session-control-local")
        })
        .collect())
}

pub(super) fn plugin_matches(plugin: &Value, version: &str, expected_source: &str) -> bool {
    plugin.get("version").and_then(Value::as_str) == Some(version)
        && plugin.get("installed").and_then(Value::as_bool) == Some(true)
        && plugin.get("enabled").and_then(Value::as_bool) == Some(true)
        && plugin_uses_source(plugin, expected_source)
}

pub(super) fn plugin_uses_source(plugin: &Value, expected_source: &str) -> bool {
    plugin
        .get("source")
        .and_then(|source| source.get("path"))
        .and_then(Value::as_str)
        == Some(expected_source)
}

pub(super) fn cleanup_codex_executable(
    configured_codex: Option<PathBuf>,
    manifested_codex: Option<PathBuf>,
    path_environment: &OsStr,
    cwd: &Path,
) -> Option<PathBuf> {
    configured_codex
        .or(manifested_codex)
        .or_else(|| resolve_named_executable(path_environment, cwd, "codex").ok())
}

pub(super) fn remove_native_plugin_if_present(
    codex: &Path,
    codex_home: &Path,
) -> Result<(), ControllerError> {
    let listed = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let present = !product_plugins(&listed)?.is_empty();
    if present {
        run_codex_json(
            codex,
            codex_home,
            &[
                OsStr::new("plugin"),
                OsStr::new("remove"),
                OsStr::new("codex-session-control@codex-session-control-local"),
                OsStr::new("--json"),
            ],
        )?;
        let verified = run_codex_json(
            codex,
            codex_home,
            &[
                OsStr::new("plugin"),
                OsStr::new("list"),
                OsStr::new("--json"),
            ],
        )?;
        if !product_plugins(&verified)?.is_empty() {
            return Err(ControllerError::InvalidData {
                field: "plugin",
                reason: "native plugin verification failed",
            });
        }
    }
    Ok(())
}

pub(super) fn remove_native_marketplace_if_present(
    codex: &Path,
    codex_home: &Path,
) -> Result<(), ControllerError> {
    let listed = run_codex_json(
        codex,
        codex_home,
        &[
            OsStr::new("plugin"),
            OsStr::new("marketplace"),
            OsStr::new("list"),
            OsStr::new("--json"),
        ],
    )?;
    let present = !marketplace_roots(&listed)?.is_empty();
    if present {
        run_codex_json(
            codex,
            codex_home,
            &[
                OsStr::new("plugin"),
                OsStr::new("marketplace"),
                OsStr::new("remove"),
                OsStr::new("codex-session-control-local"),
                OsStr::new("--json"),
            ],
        )?;
        let verified = run_codex_json(
            codex,
            codex_home,
            &[
                OsStr::new("plugin"),
                OsStr::new("marketplace"),
                OsStr::new("list"),
                OsStr::new("--json"),
            ],
        )?;
        if !marketplace_roots(&verified)?.is_empty() {
            return Err(ControllerError::InvalidData {
                field: "marketplace",
                reason: "native marketplace verification failed",
            });
        }
    }
    Ok(())
}

pub(super) fn manual_native_removal(
    codex_home: &Path,
    codex: Option<&Path>,
    marketplace: bool,
) -> String {
    let codex = codex
        .and_then(|path| shell_quote_path(path).ok())
        .unwrap_or_else(|| "codex".to_owned());
    let arguments = if marketplace {
        "plugin marketplace remove codex-session-control-local --json"
    } else {
        "plugin remove codex-session-control@codex-session-control-local --json"
    };
    format!(
        "manual: CODEX_HOME={} {codex} {arguments}\n",
        shell_quote_path(codex_home).unwrap_or_else(|_| "'<invalid-codex-home>'".to_owned())
    )
}
