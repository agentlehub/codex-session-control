# Architecture

Codex Session Control is a local, stateless stdio MCP plugin. Each host task
starts one plugin process, which connects its operations to the app-server
already owned by the supported community Desktop build. Codex/Desktop owns
authentication, configuration, and task data; Codex Session Control creates no
second login or task database.

```text
Codex CLI task ────────┐
Codex Desktop task ────┼── stdio MCP ── private Unix socket ── supported community Desktop app-server
Generic stdio host ────┘                                    (not OpenAI's official ChatGPT Desktop app)
```

## Endpoint and connection model

Every independent tool operation freshly resolves and validates the Desktop
endpoint, opens a fresh connection to the validated private Unix socket, and
initializes the connection. An explicit bridge socket takes precedence;
otherwise the endpoint is derived from the runtime directory and Desktop
application ID. The process does not scan for endpoints, use TCP, or read legacy
installation state.

The endpoint directory and socket must be private and owned by the current
Linux user. Unsafe, missing, or changed metadata prevents the operation from
connecting. After a Desktop or socket restart, an existing plugin process
reconnects on its next operation; it does not require a new task or CLI session.

Tool-catalog changes take effect in new tasks and CLI sessions. Open tasks and
sessions retain the tools and plugin process they already loaded.

## Operation semantics

The MCP server retains the Codex protocol mapping, request correlation, waits,
and cross-thread safeguards. A mutation is dispatched at most once. If the
connection breaks after it may have been sent, the result is `outcome_unknown`;
the server does not replay the mutation.

A persisted thread can be returned as `notLoaded` after a restart. Before
`thread_message_send` starts a turn, it resumes that exact thread on the same
connection and verifies that it loaded. A failed, mismatched, still-unloaded,
or active result sends no prompt.

`thread_interrupt` interrupts the selected thread by default. With
`includeDescendants: true`, it also interrupts discovered active subagents,
including nested subagents. Interrupting does not pause or clear goals.

See [Security](security.md) for the transport and caller trust boundary.
