# Codex Session Control

Codex Session Control lets MCP clients create, inspect, message, and manage
your Codex tasks. It runs locally under your Linux account and connects to the
app-server already owned by Codex Desktop.

Codex Session Control requires the unofficial community Codex Desktop build from
[ilysenko/codex-desktop-linux](https://github.com/ilysenko/codex-desktop-linux),
with `shared-app-server-socket` enabled. It does not work with OpenAI's official
ChatGPT Desktop app.

## Why this exists

Codex can delegate work to subagents, but its native tools for coordinating
independent tasks, especially across Desktop and CLI, are limited. Desktop's
built-in tools cannot manage task goals, regular Desktop and CLI instances do
not share live task control, and `codex exec` is better suited to individual
non-interactive runs than ongoing coordination.

Codex Session Control provides one MCP interface for managing live Codex tasks.
Clients can create workers, inspect progress, wait for updates, send follow-ups,
manage goals, and interrupt active responses. Supported Desktop tasks, CLI
tasks, and other MCP clients all work with the same tasks.

This enables two useful workflows:

- **Orchestrator:** delegate work across multiple tasks, coordinate their progress, and combine their results.
- **Supervisor:** monitor long-running tasks, periodically check their work, and intervene only when necessary.

Workers can be ordinary Codex tasks rather than native subagents. This keeps
them visible and easy to inspect, and supports model combinations that native
spawning cannot currently use. For example, a Sol or Terra controller can
manage a Luna worker.

All of this is possible through the underlying Codex app-server, but integrating
with that protocol directly is cumbersome. Codex Session Control handles those
details and exposes a simpler interface designed for agents.

## Install

Keep a stable checkout of this repository. From its root, install the local
plugin:

```bash
./scripts/install-local-plugin.sh
```

The installer builds and stages a native binary for the current Linux host. It
supports x86-64 and AArch64, then registers the local marketplace and plugin
with Codex.

Requirements:

- Linux on x86-64 or AArch64
- The supported community Desktop build open with `shared-app-server-socket` enabled
- A private Desktop shared socket available to the current Linux user
- Native app-server protocol validated against Codex `0.150.0-alpha.12.2`. <!-- generated: supported-codex-version -->
- Codex CLI `0.149.1` on `PATH` is the plugin-host target.

## Use

Create a new task in the community Desktop build, or launch its CLI with:

```bash
codex-desktop --cli
```

Other trusted local stdio MCP hosts can use the same local plugin. Desktop must
remain open because it owns the app-server and private shared socket.

Installing, enabling, disabling, or restaging the plugin takes effect only in a
new host task or CLI session. An already open task or session retains the tools
and plugin process it loaded.

### MCP tools

| Tool | Purpose |
| --- | --- |
| `thread_create` | Create a thread and start its initial turn. |
| `thread_fork` | Fork a thread. |
| `threads_list` | List threads. |
| `thread_read` | Read thread metadata and a page of turns. |
| `threads_wait` | Wait until a target is ready, a target read fails, or the timeout expires. |
| `thread_message_send` | Send a message to another thread, starting a turn if idle or steering its active turn. Overrides are rejected when steering. |
| `thread_title_set` | Set a thread title. |
| `thread_goal_get` | Read another thread's goal. |
| `thread_goal_set` | Set or replace another thread's goal and make it active. A running turn continues unchanged. |
| `thread_goal_pause` | Pause another thread's goal without interrupting its active turn. |
| `thread_goal_resume` | Resume another thread's goal. |
| `thread_goal_clear` | Clear another thread's goal without interrupting its active turn. |
| `thread_interrupt` | Interrupt another thread's active turn, optionally including active subagents. Active goals may start another turn. |

Do not make conflicting changes to the same task through multiple clients at
once. A mutation is dispatched at most once. If a tool returns
`outcome_unknown`, the action may already have reached Codex; inspect the thread
before deciding what to do next rather than retrying blindly.

## Update and remove

To update a checkout that tracks a remote branch, pull the intended revision and
run the installer again:

```bash
git pull --ff-only
./scripts/install-local-plugin.sh
```

Start a new CLI session or Desktop task after restaging. Existing sessions and
tasks keep their already loaded plugin process.

To remove the local plugin registration and marketplace:

```bash
codex plugin remove codex-session-control@codex-session-control-local
codex plugin marketplace remove codex-session-control-local
```

Removal affects newly created sessions and tasks. It does not delete the
checkout or its staged binary, and it does not terminate processes already owned
by open sessions or tasks.

## Desktop compatibility

Only the [community Codex Desktop build](https://github.com/ilysenko/codex-desktop-linux)
with `shared-app-server-socket` enabled is supported. See [Desktop support](docs/desktop.md).

## Support

If something is not working, check [Troubleshooting](docs/troubleshooting.md)
first. If that does not solve the problem, open a
[bug report](https://github.com/agentlehub/codex-session-control/issues/new?template=bug.yml).

## Security

Codex Session Control runs locally under your Linux account and does not open a
network port. Only processes running as the same user can access it. See
[Architecture](docs/architecture.md) and [Security](docs/security.md) for
details.

Report vulnerabilities privately by following [SECURITY.md](SECURITY.md). Do
not use public issues for security reports.
