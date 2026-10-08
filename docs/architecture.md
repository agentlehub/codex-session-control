# Architecture

Codex Session Control is a local, stateless stdio MCP plugin. Each MCP client that loads it connects to the app-server already owned by the supported community [Codex Desktop Linux build](https://github.com/ilysenko/codex-desktop-linux). Codex Desktop owns authentication, configuration, and task data; Codex Session Control creates no second login or task database. It does not work with OpenAI's official ChatGPT Desktop app.

```text
Codex CLI task ────────┐
Codex Desktop task ────┼── stdio MCP ── private Unix socket ── Codex Desktop app-server
Generic stdio host ────┘
```

## Connection model

For each operation, the plugin validates Desktop's private same-user Unix socket and connects to it. It does not start Desktop, listen on TCP, or use an alternative endpoint.

After Desktop or its socket restarts, retry the operation. The existing plugin reconnects on its next operation.

## Operation semantics

Actions that change a task are sent at most once. If a connection breaks after an action may have been sent, the tool returns `outcome_unknown` instead of trying again automatically.

After a restart, Desktop can remember a task before it has loaded it. Before `thread_message_send` starts a turn for that task, Codex Session Control first asks Desktop to load that exact task. It sends no message unless the task loads successfully.

`thread_interrupt` interrupts the selected task by default. With `includeDescendants: true`, it also interrupts active subagents, including nested ones. Interrupting does not pause or clear goals.

See [Security](security.md) for the transport and caller trust boundary.
