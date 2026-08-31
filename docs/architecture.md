# Architecture

Codex Session Control is one stateless stdio MCP process for each host task.
The checkout-local plugin starts
`plugins/codex-session-control/bin/codex-session-control` with no arguments.
It does not own a Codex app-server, a socket, process lifecycle, configuration
state, or a network endpoint.

```text
Codex CLI task ────────┐
Codex Desktop task ────┼── stdio MCP ── private Unix socket ── Desktop app-server
Generic stdio host ────┘
```

## Endpoint and connection model

Every independent tool operation freshly resolves and validates the Desktop
endpoint, opens the Unix WebSocket at exact `/rpc`, and initializes the
connection. An explicit bridge socket takes precedence; otherwise the endpoint
is derived from the runtime directory and Desktop application ID. The process
does not scan for endpoints, use TCP, or read legacy installation state.

The endpoint directory and socket must be private and owned by the current
Linux user. Unsafe, missing, or changed metadata prevents the operation from
connecting. A Desktop restart is recovered by the next independent operation,
which resolves a fresh endpoint instead of reusing an old connection.

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
