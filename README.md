# Codex Session Control

Codex Session Control lets MCP clients create, inspect, message, and manage your Codex tasks. It runs locally under your Linux account and connects to the app-server already owned by Codex Desktop.

Codex Session Control requires the unofficial community [Codex Desktop Linux build](https://github.com/ilysenko/codex-desktop-linux) with its [`shared-app-server-socket` feature](https://github.com/ilysenko/codex-desktop-linux/blob/main/linux-features/shared-app-server-socket/README.md) enabled. It does not work with OpenAI's official ChatGPT Desktop app.

## Why this exists

Codex can delegate work to subagents, but its native tools for coordinating independent tasks, especially across Desktop and CLI, are limited. Desktop's built-in tools cannot manage task goals, regular Desktop and CLI instances do not share live task control, and `codex exec` is better suited to individual non-interactive runs than ongoing coordination.

Codex Session Control provides one MCP interface for managing live Codex tasks. Clients can create workers, inspect progress, wait for updates, send follow-ups, manage goals, and interrupt active responses. Desktop tasks, CLI tasks, and other trusted MCP clients all work with the same tasks.

This enables two powerful workflows:

- **Orchestrator:** delegate work across multiple tasks, coordinate their progress, and combine their results.
- **Supervisor:** monitor long-running tasks, periodically check their work, and intervene only when necessary.

Workers can be ordinary Codex tasks rather than native subagents. This keeps them visible and easy to inspect, and supports model combinations that native spawning cannot currently use. For example, a Sol or Terra controller can manage a Luna worker.

All of this is possible through the underlying Codex app-server, but integrating with that protocol directly is cumbersome. Codex Session Control handles those details and exposes a simpler interface designed for agents.

## Install

Requirements:

- Linux on x86-64 or AArch64
- Rust 1.95 or newer with Cargo, plus `jq` and `readelf` from binutils
- Codex CLI `0.149.1` on `PATH`, used to register the plugin
- The supported community Desktop build, running with `shared-app-server-socket` enabled and bundled Codex `0.150.0-alpha.12.2` <!-- generated: supported-codex-version -->

Clone this repository and install the local plugin:

```bash
git clone https://github.com/agentlehub/codex-session-control.git
cd codex-session-control
./scripts/install-local-plugin.sh
```

In Desktop's Plugins UI, enable Codex Session Control, then create a new task. Changes to the plugin take effect only in new Desktop tasks and CLI sessions.

## How to use

Create a task in the community Desktop build, or launch its CLI with:

```bash
codex-desktop --cli
```

Desktop must remain open because it owns the app-server and shared socket. Other trusted local stdio MCP hosts can use the same plugin while Desktop is open.

### MCP tools

| Tool | Purpose |
| --- | --- |
| `thread_create` | Create a task and send its first message. |
| `thread_fork` | Create a new task from an existing task. |
| `threads_list` | List tasks. |
| `thread_read` | Read a task and its conversation history. |
| `threads_wait` | Wait for changes across multiple tasks. |
| `thread_message_send` | Send a message to a task. |
| `thread_title_set` | Rename a task. |
| `thread_goal_get` | Read a task's goal. |
| `thread_goal_set` | Set or replace a task's goal. |
| `thread_goal_pause` | Pause a task's goal. |
| `thread_goal_resume` | Resume a task's goal. |
| `thread_goal_clear` | Clear a task's goal. |
| `thread_interrupt` | Interrupt a task's active response, optionally including active subagents. |

Sending a message to an active task steers its current response. Goal changes do not interrupt an active response. `thread_interrupt` affects only the selected task unless `includeDescendants` is set to `true`.

Do not work on the same task through Codex Session Control and regular Codex at the same time. Concurrent changes are not coordinated and can conflict.

If a connection drops during an action, an MCP tool may return `outcome_unknown`. The action might already have happened, so inspect the task before trying again.

## Updates and unregistering

Update from the checkout used for installation:

```bash
git pull --ff-only
./scripts/install-local-plugin.sh
```

Start a new Desktop task or CLI session after updating.

To unregister the local plugin and marketplace:

```bash
codex plugin remove codex-session-control@codex-session-control-local
codex plugin marketplace remove codex-session-control-local
```

This leaves the checkout and staged binary in place. It does not delete Codex configuration, login, tasks, or unrelated plugins.

## Support

If something is not working, check [Troubleshooting](docs/troubleshooting.md) first. If that does not solve the problem, open a [bug report](https://github.com/agentlehub/codex-session-control/issues/new?template=bug.yml).

## Security

Codex Session Control runs locally under your Linux account and does not open a network port. Only processes running as the same user can access it. See [Architecture](docs/architecture.md) and [Security](docs/security.md) for details.

Report vulnerabilities privately by following [SECURITY.md](SECURITY.md). Do not use public issues for security reports.
