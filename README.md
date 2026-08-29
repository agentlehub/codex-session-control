# Codex Session Control

Codex Session Control lets MCP clients create, inspect, message, and manage your Codex sessions. It runs locally under your Linux account and uses your Codex installation and configuration. MCP clients, Codex CLI, and compatible Desktop builds can connect to the same local service and work with the same sessions.

## Why this exists

Codex can delegate work to subagents, but its native tools for coordinating independent sessions, especially across Desktop and CLI, are limited. Desktop’s built-in tools cannot manage session goals, regular Desktop and CLI instances do not share live task control, and `codex exec` is better suited to individual non-interactive runs than ongoing coordination.

Codex Session Control provides one MCP interface for managing live Codex sessions. Clients can create workers, inspect progress, wait for updates, send follow-ups, manage goals, and interrupt active responses. Attached CLI sessions, supported Desktop builds, and other MCP clients all work with the same sessions.

This enables two powerful workflows:

- **Orchestrator:** delegate work across multiple sessions, coordinate their progress, and combine their results.
- **Supervisor:** monitor long-running sessions, periodically check their work, and intervene only when necessary.

Workers can be ordinary Codex sessions rather than native subagents. This keeps them visible and easy to inspect, and supports model combinations that native spawning cannot currently use. For example, a Sol or Terra controller can manage a Luna worker.

All of this is possible through the underlying Codex app-server, but integrating with that protocol directly is cumbersome. Codex Session Control handles those details and exposes a simpler interface designed for agents.

## Install

Requirements:

- Linux on x86-64 or ARM64 with a working systemd user session
- Native app-server protocol validated against Codex `0.150.0-alpha.12.2`. <!-- generated: supported-codex-version -->
- Codex CLI `0.149.1` on `PATH` is the plugin-host target.
- `curl` and `sha256sum`

Download and run the release installer:

```bash
curl --fail --location --output install.sh \
  https://github.com/agentlehub/codex-session-control/releases/latest/download/install.sh
sh ./install.sh
```

The installer downloads the correct release for your system and configures Codex Session Control.

If you use a custom `CODEX_HOME`, pass its absolute path to the installer:

```bash
CODEX_HOME=/absolute/path/to/codex-home sh ./install.sh
```

Codex Session Control saves this choice for future commands.

## How to use

Launch a Codex CLI that shares the same sessions:

```bash
codex-session-control codex
```

If Codex is signed out, complete its normal sign-in flow. Codex Session Control does not require or perform sign-in during installation.

You can still run `codex` normally when you do not want it connected to Codex Session Control. Already-running CLI and Desktop clients do not switch automatically; close and relaunch them to connect.

Available commands:

| Command | What it does |
| --- | --- |
| `setup` | Install Codex Session Control and start its service. |
| `update` | Install the latest release. |
| `status` | Check whether Codex Session Control is ready. |
| `enable` | Start the service and turn on automatic startup. |
| `disable` | Stop the service and turn off automatic startup. |
| `uninstall` | Remove the service while keeping your Codex data. |
| `codex` | Launch Codex CLI connected to Codex Session Control. |

### MCP tools

| Tool | Purpose |
| --- | --- |
| `thread_create` | Create a session and send its first message. |
| `thread_fork` | Create a new session from an existing one. |
| `threads_list` | List sessions. |
| `thread_read` | Read a session and its conversation history. |
| `threads_wait` | Wait for changes across multiple sessions. |
| `thread_message_send` | Send a message to a session. |
| `thread_title_set` | Rename a session. |
| `thread_goal_get` | Read a session's goal. |
| `thread_goal_set` | Set or replace a session's goal. |
| `thread_goal_pause` | Pause a session's goal. |
| `thread_goal_resume` | Resume a session's goal. |
| `thread_goal_clear` | Clear a session's goal. |
| `thread_interrupt` | Interrupt a session's active response, optionally including active subagents. |

Do not work on the same session through Codex Session Control and regular Codex at the same time. Concurrent changes are not coordinated and can conflict.

If a connection drops during an action, an MCP tool may return `outcome_unknown`. The action might already have happened, so inspect the session before trying again.

## Updates and removal

Update to the latest release with:

```bash
codex-session-control update
```

Most updates do not interrupt running work. If a service restart is required, Codex Session Control lists the active sessions and asks before continuing. The default answer is no.

Running responses interrupted by a restart remain marked as interrupted. Active goals are not paused automatically and may continue when their sessions resume. Pause any goal you do not want to continue before approving the restart. If an update stops partway through, follow the retry command shown in the error message.

Remove Codex Session Control with:

```bash
codex-session-control uninstall
```

Uninstalling does not delete your Codex configuration, login, sessions, or unrelated plugins.

## Desktop compatibility

Desktop integration is optional. CLI and MCP work without it. Only builds from the [`agentlehub/codex-desktop-linux` fork](https://github.com/agentlehub/codex-desktop-linux) are supported. Follow [Desktop support](docs/desktop.md) to build and connect Desktop.

## Support

If something is not working, check [Troubleshooting](docs/troubleshooting.md) first. If that does not solve the problem, open a [bug report](https://github.com/agentlehub/codex-session-control/issues/new?template=bug.yml).

## Security

Codex Session Control runs locally under your Linux account and does not open a network port. Only processes running as the same user can access it. See [Architecture](docs/architecture.md) and [Security](docs/security.md) for details.

Report vulnerabilities privately by following [SECURITY.md](SECURITY.md). Do not use public issues for security reports.
