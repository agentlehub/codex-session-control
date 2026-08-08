# Architecture

Codex Session Control consists of one Rust executable, one systemd user service, and one local MCP server. Codex continues to own authentication and session data in the selected `CODEX_HOME`; Codex Session Control owns only the files required to install, connect, and run the service. It does not create another login or session database.

```text
attached CLI ───────────────┐
supported Desktop ──────────┼── Unix socket ── native Codex app-server
MCP caller ─ stdio endpoint ┘                       │
                                                    └── selected CODEX_HOME
                                                            ▲
                                                 direct systemd user unit
```

## Installed files and paths

| Purpose | Path or source |
| --- | --- |
| Executable | `$HOME/.local/bin/codex-session-control` |
| Product configuration | `$HOME/.config/codex-session-control/config.toml` |
| User unit | `$HOME/.config/systemd/user/codex-session-control.service` |
| Marketplace and manifest | `$HOME/.local/share/codex-session-control` |
| Runtime directory | `$XDG_RUNTIME_DIR/codex-session-control` |
| App-server socket | `$XDG_RUNTIME_DIR/codex-session-control/app-server.sock` |
| Optional Desktop connection file | `${XDG_CONFIG_HOME:-$HOME/.config}/<Desktop app ID>/app-server-attachment.json` |

## Saved Codex settings

During first setup, Codex Session Control uses `CODEX_HOME` when it is set; otherwise it uses `$HOME/.codex`. It saves the selected Codex home, Codex executable, and socket in its configuration and release manifest. Later changes to `CODEX_HOME` do not affect the installed service.

After installation, Codex Session Control stops without making changes if those saved settings are missing, invalid, or inconsistent.

## How the service runs

The systemd user service runs:

```text
<configured-codex> app-server --listen unix://<configured-socket>
```

It uses the saved `CODEX_HOME` and `UMask=0077`. systemd starts Codex directly without a shell, intermediary daemon, second endpoint, or separate session store.

## CLI and clients

`codex-session-control codex` validates the installed configuration, socket, and app-server connection. It then launches Codex with the saved `CODEX_HOME`, configured socket, and original arguments. Regular `codex` still works, but it does not use the shared app-server. To work with sessions through Codex Session Control, launch it with `codex-session-control codex`.

Do not use regular `codex` and Codex Session Control to change the same session at the same time. Their changes are not coordinated and can conflict.

CLI and MCP work without Desktop integration. Desktop support requires a verified build from the [`agentlehub/codex-desktop-linux` fork](https://github.com/agentlehub/codex-desktop-linux) and its connection file. See [Desktop support](desktop.md).

## Lifecycle

`setup` and `enable` create the Desktop connection file before starting the service. `disable` and `uninstall` stop the service and wait for its socket to disappear before removing that file. Uninstall removes only files owned by Codex Session Control.

Before an operation can stop or restart an active managed service, Codex Session Control inspects the caller. A successful `systemctl --user whoami` exactly identifies the caller's unit: the managed unit proves self-hosting, while another valid unit proves independence. If that inspection is unavailable, the fallback compares the caller's cgroup with the managed service's `ControlGroup`; it can prove self-hosting but never independence. Any other result is unknown and fails closed before a disruptive operation.

`update` verifies the downloaded release and current installation, snapshots the service, and determines whether the running Codex version, configured executable path, or rendered systemd unit requires a restart. A no-restart update applies its durable files without interrupting the service, verifies the resulting service state, and writes the schema-3 installation manifest last. If an active service needs a restart, caller independence is required before the active-task gate, file changes, descriptor changes, or service action. An independent caller then reviews the active-task interruption prompt before the update applies files, restarts the service, verifies it, and persists the manifest last.

For an active service, `disable` and `uninstall` inspect the caller before `systemctl --user disable --now`. Only after the service stop is verified can either command remove a Desktop connection file; uninstall then removes only files owned by Codex Session Control. If any lifecycle operation fails, Codex Session Control reports the completed stages and shows a recovery command. It does not undo completed changes automatically.

MCP tool catalog changes apply to new sessions. Already-open sessions may continue using cached tools.

## MCP connections

Each MCP client using Codex Session Control starts `codex-session-control mcp-server` as a local stdio process. The MCP server sends tool requests to the shared Codex app-server through the configured Unix socket, so all connected MCP clients work with the same sessions.

MCP operations have time limits, and responses are matched to request IDs. Actions that change a session are sent at most once. If the connection is lost after an action may have been sent, the tool reports `outcome_unknown` instead of retrying automatically.

See [Security](security.md) for MCP access and trust boundaries.
