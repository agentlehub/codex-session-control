# Codex Session Control

Codex Session Control is a local, stateless stdio MCP plugin for coordinating
Codex tasks. It connects each operation to the shared app-server authority
already owned by Codex Desktop; Desktop must be open and its private shared
socket must be available.

The plugin exposes one thirteen-tool catalog for Codex CLI, Codex Desktop, and
other stdio MCP hosts. It has no management CLI, does not own app-server or
socket lifecycle, and does not add a network listener.

## Install

Keep a stable checkout of this repository. From its root, install the local
plugin:

```bash
./scripts/install-local-plugin.sh
```

The installer builds and stages a native binary for the current Linux host. It
supports x86-64 and AArch64. It then registers the checkout-local legacy
plugin with Codex.

Requirements:

- Linux on x86-64 or AArch64
- Codex Desktop open with `shared-app-server-socket` enabled
- A private Desktop shared socket available to the current Linux user
- Native app-server protocol validated against Codex `0.150.0-alpha.12.2`. <!-- generated: supported-codex-version -->
- Codex CLI `0.149.1` on `PATH` is the plugin-host target.

## Use

Start a new normal Codex CLI session or a new Desktop task after installation.
The host loads plugin tools when it creates the session or task; an already open
one does not acquire them retroactively. In Desktop, use the Plugins UI to
enable or disable the plugin, then create a new task to observe the change.

Other stdio MCP clients can use the same checkout-local plugin binary. They
receive the same catalog and require the same open Desktop authority.

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

Do not make conflicting changes to the same thread through multiple clients at
once. A mutation is dispatched at most once. If a tool returns
`outcome_unknown`, the action may already have reached Codex; inspect the thread
before deciding what to do next rather than retrying blindly.

## Update and remove

To update, keep using the same stable checkout, pull the desired revision, and
run the installer again:

```bash
git pull
./scripts/install-local-plugin.sh
```

Start a new CLI session or Desktop task after restaging. Existing sessions and
tasks keep their already loaded plugin process.

To remove the native plugin registration:

```bash
codex plugin remove codex-session-control@codex-session-control-local
```

Native removal removes the plugin from newly created sessions and tasks. It
does not delete the checkout or its staged binary, and it does not terminate
processes already owned by open sessions or tasks. See [upgrading](docs/upgrading.md)
when replacing a historical 0.3.x installation.

## Support and security

See [Troubleshooting](docs/troubleshooting.md) before opening a
[bug report](https://github.com/agentlehub/codex-session-control/issues/new?template=bug.yml).
For the transport and trust model, see [Architecture](docs/architecture.md) and
[Security](docs/security.md). Report vulnerabilities privately as described in
[SECURITY.md](SECURITY.md).
