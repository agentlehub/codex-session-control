use std::fs;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::*;

const EXPECTED_UNIT_TEMPLATE: &[u8] = b"[Unit]\n\
Description=Codex app-server for Codex session control\n\
\n\
[Service]\n\
Type=simple\n\
UMask=0077\n\
Environment=CODEX_HOME=__CODEX_HOME__\n\
RuntimeDirectory=codex-session-control\n\
RuntimeDirectoryMode=0700\n\
WorkingDirectory=__USER_HOME__\n\
ExecStart=__CODEX_EXECUTABLE__ app-server --listen unix://__SOCKET_PATH__\n\
Restart=on-failure\n\
RestartSec=2s\n\
\n\
[Install]\n\
WantedBy=default.target\n";
const EXPECTED_MARKETPLACE: &[u8] = br#"{
  "name": "codex-session-control-local",
  "interface": {
    "displayName": "Codex session control"
  },
  "plugins": [
    {
      "name": "codex-session-control",
      "source": {
        "source": "local",
        "path": "./plugins/codex-session-control"
      },
      "policy": {
        "installation": "AVAILABLE"
      },
      "category": "Coding"
    }
  ]
}
"#;
const EXPECTED_PLUGIN: &[u8] = br#"{
  "name": "codex-session-control",
  "version": "__PRODUCT_VERSION__",
  "description": "Control Codex sessions via MCP",
  "author": {
    "name": "Agentlehub"
  },
  "license": "MIT",
  "mcpServers": "./.mcp.json",
  "interface": {
    "displayName": "Codex session control",
    "shortDescription": "Control Codex sessions via MCP",
    "category": "Coding",
    "capabilities": ["Read", "Write"]
  }
}
"#;
const EXPECTED_MCP: &[u8] = br#"{
  "mcpServers": {
    "codex-session-control": {
      "command": "__INSTALLED_EXECUTABLE__",
      "args": ["mcp-server"],
      "env_vars": ["XDG_RUNTIME_DIR"],
      "tool_timeout_sec": 86460
    }
  }
}
"#;

fn fixture() -> (tempfile::TempDir, ResolvedUserPaths) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    let euid = rustix::process::geteuid().as_raw();
    (root, ResolvedUserPaths::for_test(euid, home, runtime))
}

#[test]
fn embedded_assets_are_the_exact_approved_source_bytes() {
    assert_eq!(
        include_bytes!("../../../assets/systemd/codex-session-control.service.in"),
        EXPECTED_UNIT_TEMPLATE
    );
    assert_eq!(
        include_bytes!("../../../assets/marketplace/.agents/plugins/marketplace.json"),
        EXPECTED_MARKETPLACE
    );
    assert_eq!(
        include_bytes!(
            "../../../assets/marketplace/plugins/codex-session-control/.codex-plugin/plugin.json"
        ),
        EXPECTED_PLUGIN
    );
    assert_eq!(
        include_bytes!("../../../assets/marketplace/plugins/codex-session-control/.mcp.json"),
        EXPECTED_MCP
    );
}

#[test]
fn service_unit_renders_exact_paths_and_systemd_escapes() {
    let (_root, paths) = fixture();
    let codex = paths.home.join("bin/codex");
    let rendered = String::from_utf8(render_unit(&paths, &codex).unwrap()).unwrap();
    let expected = format!(
        "[Unit]\n\
Description=Codex app-server for Codex session control\n\
\n\
[Service]\n\
Type=simple\n\
UMask=0077\n\
Environment=CODEX_HOME={}\n\
RuntimeDirectory=codex-session-control\n\
RuntimeDirectoryMode=0700\n\
WorkingDirectory={}\n\
ExecStart={} app-server --listen unix://{}\n\
Restart=on-failure\n\
RestartSec=2s\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        paths.codex_home.display(),
        paths.home.display(),
        codex.display(),
        paths.socket.display(),
    );
    assert_eq!(rendered, expected);
    assert!(rendered.contains("\nUMask=0077\n"));
    assert!(!rendered.contains("\nUMask=0177\n"));
    assert!(!rendered.contains("__"));
}

#[test]
fn service_unit_escapes_unsafe_systemd_word_bytes_without_shell() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home with \"quote\" % $ slash\\");
    let runtime = root.path().join("runtime with space");
    let paths = ResolvedUserPaths::for_test(rustix::process::geteuid().as_raw(), home, runtime);
    let codex = paths.home.join("bin/codex with space");

    let rendered = String::from_utf8(render_unit(&paths, &codex).unwrap()).unwrap();

    assert!(rendered.contains("home\\x20with\\x20\\x22quote\\x22\\x20%%\\x20$$\\x20slash\\x5c"));
    assert!(rendered.contains("codex\\x20with\\x20space app-server"));
    assert!(rendered.contains("runtime\\x20with\\x20space"));
    assert!(!rendered.contains("__"));
}

#[test]
fn projection_renders_exact_json_command_timeout_and_digest() {
    let (_root, paths) = fixture();
    let rendered = render_projection(&paths.binary, env!("CARGO_PKG_VERSION")).unwrap();
    let marketplace: Value = serde_json::from_slice(&rendered.marketplace).unwrap();
    let plugin: Value = serde_json::from_slice(&rendered.plugin).unwrap();
    let mcp: Value = serde_json::from_slice(&rendered.mcp).unwrap();

    assert_eq!(
        marketplace["name"],
        Value::String("codex-session-control-local".to_owned())
    );
    assert_eq!(
        plugin["version"],
        Value::String(env!("CARGO_PKG_VERSION").to_owned())
    );
    assert_eq!(
        mcp["mcpServers"]["codex-session-control"]["command"],
        Value::String(paths.binary.display().to_string())
    );
    assert_eq!(
        mcp["mcpServers"]["codex-session-control"]["args"],
        serde_json::json!(["mcp-server"])
    );
    assert_eq!(
        mcp["mcpServers"]["codex-session-control"]["env_vars"],
        serde_json::json!(["XDG_RUNTIME_DIR"])
    );
    assert_eq!(
        mcp["mcpServers"]["codex-session-control"]["tool_timeout_sec"],
        86460
    );
    for bytes in [&rendered.marketplace, &rendered.plugin, &rendered.mcp] {
        assert!(!String::from_utf8_lossy(bytes).contains("__"));
    }

    let mut digest = Sha256::new();
    for (relative, bytes) in [
        (
            ".agents/plugins/marketplace.json",
            rendered.marketplace.as_slice(),
        ),
        (
            "plugins/codex-session-control/.codex-plugin/plugin.json",
            rendered.plugin.as_slice(),
        ),
        (
            "plugins/codex-session-control/.mcp.json",
            rendered.mcp.as_slice(),
        ),
    ] {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(bytes);
        digest.update([0]);
    }
    assert_eq!(rendered.sha256, hex::encode(digest.finalize()));
    assert_eq!(rendered.sha256.len(), 64);
}

#[test]
fn renderers_reject_relative_paths_and_remaining_sentinels() {
    let (_root, paths) = fixture();
    assert!(render_unit(&paths, Path::new("codex")).is_err());
    assert!(render_projection(Path::new("codex-session-control"), "1.0.0").is_err());
    assert!(render_projection(&paths.binary, "__UNRESOLVED__").is_err());
}
