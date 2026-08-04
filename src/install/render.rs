use std::{os::unix::ffi::OsStrExt, path::Path};

use sha2::{Digest, Sha256};

use crate::error::ControllerError;

use super::{
    evidence::ResolvedUserPaths,
    paths::{create_product_dir, reconcile_file},
};

pub(super) struct RenderedProjection {
    pub(super) marketplace: Vec<u8>,
    pub(super) plugin: Vec<u8>,
    pub(super) mcp: Vec<u8>,
    pub(super) sha256: String,
}

pub(super) fn render_unit(
    paths: &ResolvedUserPaths,
    codex: &Path,
) -> Result<Vec<u8>, ControllerError> {
    for path in [
        paths.home.as_path(),
        paths.codex_home.as_path(),
        paths.socket.as_path(),
        codex,
    ] {
        if !path.is_absolute() {
            return Err(ControllerError::InvalidData {
                field: "service_unit",
                reason: "all rendered paths must be absolute",
            });
        }
    }
    let template = include_str!("../../assets/systemd/codex-session-control.service.in");
    let rendered = template
        .replace("__CODEX_HOME__", &escape_systemd_path(&paths.codex_home))
        .replace("__USER_HOME__", &escape_systemd_path(&paths.home))
        .replace("__CODEX_EXECUTABLE__", &escape_systemd_path(codex))
        .replace("__SOCKET_PATH__", &escape_systemd_path(&paths.socket));
    reject_remaining_sentinel(&rendered, "service_unit")?;
    Ok(rendered.into_bytes())
}

pub(super) fn escape_systemd_path(path: &Path) -> String {
    let mut escaped = String::new();
    for byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-' => {
                escaped.push(char::from(*byte))
            }
            b'%' => escaped.push_str("%%"),
            b'$' => escaped.push_str("$$"),
            byte => {
                use std::fmt::Write as _;
                write!(&mut escaped, "\\x{byte:02x}").expect("writing to String cannot fail");
            }
        }
    }
    escaped
}

pub(super) fn render_projection(
    installed_binary: &Path,
    product_version: &str,
) -> Result<RenderedProjection, ControllerError> {
    if !installed_binary.is_absolute() {
        return Err(ControllerError::InvalidData {
            field: "projection",
            reason: "installed executable must be absolute",
        });
    }
    let installed_binary = installed_binary
        .to_str()
        .ok_or(ControllerError::InvalidData {
            field: "projection",
            reason: "installed executable must be UTF-8",
        })?;
    let marketplace =
        include_bytes!("../../assets/marketplace/.agents/plugins/marketplace.json").to_vec();
    let plugin = replace_json_string_sentinel(
        include_str!(
            "../../assets/marketplace/plugins/codex-session-control/.codex-plugin/plugin.json"
        ),
        "__PRODUCT_VERSION__",
        product_version,
    )?;
    let mcp = replace_json_string_sentinel(
        include_str!("../../assets/marketplace/plugins/codex-session-control/.mcp.json"),
        "__INSTALLED_EXECUTABLE__",
        installed_binary,
    )?;
    for bytes in [&marketplace, &plugin, &mcp] {
        let text = std::str::from_utf8(bytes).map_err(|_| ControllerError::InvalidData {
            field: "projection",
            reason: "rendered JSON must be UTF-8",
        })?;
        reject_remaining_sentinel(text, "projection")?;
        serde_json::from_slice::<serde_json::Value>(bytes).map_err(|_| {
            ControllerError::InvalidData {
                field: "projection",
                reason: "rendered JSON is invalid",
            }
        })?;
    }
    let mut digest = Sha256::new();
    for (relative, bytes) in [
        (".agents/plugins/marketplace.json", marketplace.as_slice()),
        (
            "plugins/codex-session-control/.codex-plugin/plugin.json",
            plugin.as_slice(),
        ),
        ("plugins/codex-session-control/.mcp.json", mcp.as_slice()),
    ] {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(bytes);
        digest.update([0]);
    }
    Ok(RenderedProjection {
        marketplace,
        plugin,
        mcp,
        sha256: hex::encode(digest.finalize()),
    })
}

pub(super) fn replace_json_string_sentinel(
    template: &str,
    sentinel: &str,
    replacement: &str,
) -> Result<Vec<u8>, ControllerError> {
    let quoted_sentinel =
        serde_json::to_string(sentinel).expect("static sentinel serialization cannot fail");
    let quoted_replacement =
        serde_json::to_string(replacement).map_err(|_| ControllerError::InvalidData {
            field: "projection",
            reason: "replacement cannot be serialized",
        })?;
    if template.matches(&quoted_sentinel).count() != 1 {
        return Err(ControllerError::InvalidData {
            field: "projection",
            reason: "approved sentinel must occur exactly once",
        });
    }
    Ok(template
        .replacen(&quoted_sentinel, &quoted_replacement, 1)
        .into_bytes())
}

pub(super) fn reject_remaining_sentinel(
    text: &str,
    field: &'static str,
) -> Result<(), ControllerError> {
    let Some(start) = text.find("__") else {
        return Ok(());
    };
    if text[start + 2..].contains("__") {
        return Err(ControllerError::InvalidData {
            field,
            reason: "rendered output contains an unresolved sentinel",
        });
    }
    Ok(())
}

pub(super) fn reconcile_projection(
    paths: &ResolvedUserPaths,
    projection: &RenderedProjection,
) -> Result<bool, ControllerError> {
    for directory in [
        paths.marketplace.clone(),
        paths.marketplace.join(".agents"),
        paths.marketplace.join(".agents/plugins"),
        paths.marketplace.join("plugins"),
        paths.marketplace.join("plugins/codex-session-control"),
        paths
            .marketplace
            .join("plugins/codex-session-control/.codex-plugin"),
    ] {
        create_product_dir(&directory, paths.euid)?;
    }
    let changes = [
        reconcile_file(
            &paths.marketplace.join(".agents/plugins/marketplace.json"),
            &projection.marketplace,
            0o644,
            paths.euid,
        )?,
        reconcile_file(
            &paths
                .marketplace
                .join("plugins/codex-session-control/.codex-plugin/plugin.json"),
            &projection.plugin,
            0o644,
            paths.euid,
        )?,
        reconcile_file(
            &paths
                .marketplace
                .join("plugins/codex-session-control/.mcp.json"),
            &projection.mcp,
            0o644,
            paths.euid,
        )?,
    ];
    Ok(changes.into_iter().any(std::convert::identity))
}
