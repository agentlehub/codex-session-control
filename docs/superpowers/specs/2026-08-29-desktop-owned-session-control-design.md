# Desktop-Owned Session Control Design

**Status:** approved
**Source:** `docs/superpowers/brainstorming/2026-08-28-desktop-owned-session-control.md`
**Review:** `docs/superpowers/reviews/2026-08-29-desktop-owned-session-control-design-review.md`

## Objective

Refactor Codex Session Control (CSC) into one stateless stdio MCP server that uses the app-server authority owned by upstream Codex Desktop.

Preserve the exact existing thirteen-tool public contract in Desktop, normal Codex CLI sessions, and conforming stdio MCP hosts. Delete CSC's separate authority, service, updater, attachment, and lifecycle-management surfaces.

```text
MCP host -> SessionControlMcp -> deterministic Desktop endpoint
         -> AppServerClient -> Desktop-owned shared Unix socket
```

## Prerequisites

- Work only in `/home/korty/dev/agentlehub/codex-session-control` on `feat/desktop-owned-session-control`.
- Before production edits, verify that the branch merge-base remains `origin/main` commit `88f2ac5b124abaa2a355ca88304e9439c692eb0a`, that approved design commit `da06acf2c229b5e0aa68ff45ab3aa87037aa4d52` is present, and that the worktree contains no unrelated dirt. Drift requires a target-inventory and specification re-review before deletion work.
- Preserve the public contract anchored by `src/mcp/contract.rs` and `tests/mcp_contract.rs`.
- Treat the approved brainstorming artifact named in `Source` as product policy.
- Use Codex CLI 0.149.1 as the initial plugin-host compatibility target and the installed Desktop authority's bundled Codex version as the native protocol target.
- Use Rust 1.95 and the committed `Cargo.lock` for builds.
- Have upstream Codex Desktop with `shared-app-server-socket` enabled for live validation.
- Have native Linux x86-64 and native Linux AArch64 CI runners.
- Restrict live validation to clearly named disposable tasks created by the test run.
- Do not require a release, registry, download host, personal Desktop fork, Hermes configuration, or attached-CLI command.
- Write and review an implementation plan against this specification before production edits.

## Execution and Authority

The operator authorized uninterrupted autonomous execution through specification, plan, implementation, tests, and reviews within this approved scope. Separate specification approval, plan approval, and pre-code reconfirmation pauses are waived.

One primary orchestrator owns architecture, integration, repository state, evidence, and delivery boundaries. Isolated subagents own broad discovery, bounded non-overlapping implementation slices, and each ordered review. Agents must not duplicate work or revert changes outside their assigned ownership.

Escalate only for a failed prerequisite that invalidates both the preferred design and approved fallback, a contradiction that changes product behavior, new external authority, unavailable credentials or human-only interaction, or scope outside this specification.

## Mission Phase Boundaries

### Phase 1: Codex Session Control

This specification governs the CSC refactor through the internal CSC pull request. No attached-CLI code belongs in the CSC repository.

### Phase 2: Desktop attached CLI

After the internal CSC pull request is open, continue autonomously in `/home/korty/dev/codex-desktop-linux` under a separate Desktop specification and plan. Implement the generic attached-CLI enhancement on a dedicated branch and open its first pull request only against the operator's internal `fork` remote. Do not open an issue, pull request, or proposal against official upstream without later explicit authorization.

## Scope

- Refactor the current Rust implementation; do not rewrite the protocol core.
- Make the existing binary run stdio MCP directly.
- Resolve and validate the Desktop shared socket for every new operation.
- Preserve protocol projection, validation, correlation, timeouts, waits, dispatch evidence, reconciliation, and no-blind-replay behavior.
- Port persisted `notLoaded` message recovery before `turn/start`.
- Package one current-host executable inside a local legacy Codex plugin.
- Provide one repository installer that builds, stages, and registers it.
- Keep a stable checkout-relative executable path for generic stdio clients.
- Support native Linux x86-64 and AArch64.
- Rewrite automated, live, packaging, security, and reader-facing coverage.
- Require every added module, helper, dependency, installer branch, compatibility path, and test layer to cover a unique retained behavior or plausible regression that existing code cannot cover more simply.
- Deliver an internal CSC pull request only.

## Non-goals

- Owning, spawning, stopping, restarting, reclaiming, or probing an app-server process.
- Creating, unlinking, replacing, or locking the Desktop socket or its parent.
- Retaining CSC service, wrapper, updater, status, setup, enable, disable, uninstall, or diagnostics commands.
- Reading or migrating legacy CSC configuration, descriptors, installation evidence, release state, or service state.
- Supporting the personal Desktop fork or external-attachment descriptor.
- Working while Desktop is closed.
- Scanning processes, runtime directories, state directories, or home directories for an authority.
- Using `CODEX_HOME` to select or authenticate the Desktop target.
- Exposing the authority over TCP.
- Adding an attached-CLI launcher to CSC.
- Adding Hermes-specific code, configuration, documentation, installation, or tests.
- Publishing binaries, checksums, installers, tags, releases, packages, or registry entries.
- Installing or maintaining a standalone CSC executable outside the checkout-local plugin bundle.
- Opening an upstream Desktop issue or pull request.
- Mutating any pre-existing user task during live validation.

## Gap Resolutions

1. Codex Agent Plugins v1 in 0.149.1 cannot forward the required host environment. Use legacy `.codex-plugin/plugin.json` plus `.mcp.json` initially.
2. An arbitrary clone cannot install command-free. Expose exactly one install entrypoint: `scripts/install-local-plugin.sh`.
3. The installer runs `cargo build --release --locked`; it never selects or downloads a release artifact.
4. Atomically stage exactly one current-host binary in the repo plugin bundle. Ship no downloader, checksum catalog, dispatcher, or second architecture.
5. Prove x86-64 and AArch64 on native CI runners, not through cross-compilation alone.
6. The binary starts stdio MCP directly; plugin and generic-client invocations pass no `mcp-server` subcommand.
7. Generic clients use the stable path inside the clone. Moving the clone intentionally changes it; CSC creates no global wrapper or symlink.
8. If neither an explicit bridge socket nor `XDG_RUNTIME_DIR` exists, fail. Do not copy Desktop's state-directory fallback.
9. Migration is documentation-only. The installer does not inspect, disable, remove, or rewrite a legacy installation.
10. Generic-client coverage is host-neutral; there is no Hermes-specific integration.
11. The prerequisite contained-plugin spike passed against Codex CLI 0.149.1 in isolated state: relative execution, mode/hash-preserving cache copy, exact three-variable forwarding, MCP initialize/list/call, same-version refresh, version-bump refresh, removal, and marketplace removal all passed. The approved standalone fallback is not triggered.
12. The spike's v1 negative control initialized successfully but received none of the three host variables. Legacy manifests remain required until a later supported Codex version proves equivalent forwarding.
13. Plugin-host compatibility and authority-protocol compatibility are separate. Legacy manifest behavior is verified against Codex CLI 0.149.1. The current Desktop authority reports `0.150.0-alpha.12.2`; extend the tested-version pipeline to canonical full SemVer, pin that exact version, and recapture its protocol fixture before runtime protocol edits.

## Architecture

### Ownership

Desktop is the sole app-server authority owner. CSC is a same-user client. Desktop owns process lifetime, socket creation, locks, stale-owner handling, reaping, replacement, and cleanup.

CSC must not duplicate Desktop lock/reaper logic or start a fallback authority. The socket speaks HTTP WebSocket upgrade on `/rpc`, followed by the stock app-server WebSocket protocol. Keep direct WebSocket-over-Unix transport without translation.

Production and retained native/live harness clients must connect with the exact URL `ws://localhost/rpc`. A fake server must inspect the HTTP upgrade request and reject `/`, a query string, or any path other than exactly `/rpc` so the regression cannot hide behind a permissive test transport.

### Runtime process

`src/main.rs` constructs `SessionControlMcp`, serves it over stdio, and exits cleanly at EOF. It accepts no lifecycle or management subcommands.

Stdout is MCP-only. Diagnostics use stderr. The process must not spawn Desktop, Codex, a proxy, a service manager, or another CSC process.

### Retained core

- `src/mcp/contract.rs`: sole public tool contract authority.
- `src/mcp/operations.rs`: native mapping, cross-thread policy, and reconciliation.
- `src/mcp/wait.rs`: readiness, timeout, and wait error behavior.
- `src/app_server/protocol.rs`: typed native projection and native-error classification.
- `src/error.rs`: public error categories, shapes, stages, dispatch evidence, and messages.
- `src/model.rs`: serialized `Thread`, `Turn`, `ThreadGoal`, and `ThreadSnapshot` shapes.

Delete installation and descriptor models instead of generalizing them. The only new production seam is Desktop endpoint resolution.

### Connection lifecycle

Each tool call resolves the endpoint, validates it, opens a fresh initialized connection, and performs the operation. This makes future-operation restart recovery structural.

Outcome reconciliation may open a separate read-only connection. It observes state but never replays a mutation or converts an unproven outcome into certainty.

`threads_wait` may hold one connection for one wait. Desktop restart during that wait returns the existing transport failure; CSC does not silently restart the in-flight wait.

## Endpoint, Security, Errors, and Reconnect

### Resolution

For every new connection:

1. Read `CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET` as an `OsString`. If present with nonzero byte length, use it exactly and do not read or validate XDG/app-ID inputs. Do not trim it.
2. Otherwise read `XDG_RUNTIME_DIR` as an `OsString`. Missing or zero-length is `target_unavailable`. Do not trim it.
3. Read `CODEX_LINUX_APP_ID`. Missing or zero-length defaults to `codex-desktop`. A nonempty value must be valid UTF-8, match `^[A-Za-z0-9._-]+$`, and not equal `.` or `..`.
4. Derive `${XDG_RUNTIME_DIR}/${CODEX_LINUX_APP_ID}/app-server-bridge/app-server.sock`.
5. Never consult persistent CSC state, `CODEX_HOME`, Desktop state directories, processes, locks, or scans.

An accepted explicit socket path and XDG runtime path must be absolute and byte-for-byte lexically normalized. Reconstruct the path from root plus only `Component::Normal` components and require exact equality with the original `OsString`. Reject `.`, `..`, repeated separators, trailing separators, non-root prefixes, `/` as the runtime directory, and an explicit socket path without a normal final basename. Endpoint selection is environment-only and cannot be overridden by MCP input.

### Security validation

Before every connection:

- `lstat` the socket's immediate parent. Require a real effective-user-owned directory, not a symlink, with `(mode & 0o7777) == 0o0700`.
- Canonicalize the immediate parent and require byte-for-byte equality with the selected normalized parent. This rejects a symlink in any parent component.
- Reject a symlink as the endpoint's final component.
- For a derived endpoint, separately `lstat` `XDG_RUNTIME_DIR`, the app-ID directory, and `app-server-bridge`; each must be a real effective-user-owned non-symlink directory with `(mode & 0o7777) == 0o0700`. Do not apply ownership requirements to ancestors outside the selected XDG tree such as `/run` or `/`.
- Require the final entry to be a non-symlink effective-user-owned Unix socket. Upstream emits `0600`; CSC accepts exactly `(mode & 0o7777) ∈ {0o0600, 0o0700}` to preserve the existing owner-only contract.
- Read fresh metadata immediately before connecting; never reuse earlier validation.
- Connect only through the Unix socket and WebSocket `/rpc`.

CSC may read filesystem metadata needed for validation. It must not parse, rewrite, signal from, or clean up the adjacent lock. Metadata or canonicalization races fail closed as `authority_transport_failure`.

### Error mapping

- No explicit socket and no `XDG_RUNTIME_DIR`: `target_unavailable`.
- Missing Desktop socket or absent parent: `target_unavailable`.
- Malformed initialize identity: preserve the existing initialization-stage `target_unavailable` error.
- Relative, malformed, symlinked, foreign-owned, wrong-type, or permissive endpoint: `authority_transport_failure`.
- Refusal, WebSocket failure, EOF, or transport timeout: existing `authority_transport_failure`, unless mutation dispatch made the outcome ambiguous.
- Native failures: preserve native category, code/message, tool name, target ID, and stage.
- Public errors must not expose full paths, environment values, or unrelated metadata.

### Restart and no replay

Desktop restart may replace the authority and socket inode. The same stdio MCP process recovers on the next independent operation by resolving, validating, and connecting again.

Do not retain an inode, transport, canonicalized parent, or initialize result across operations. Do not put retry logic around a mutation.

After dispatch may have occurred, transport failure remains exact `outcome_unknown` with dispatch evidence. Read-only reconciliation may reconnect, but cannot replay or claim acceptance/rejection without existing contract proof.

### Tested native protocol version

Preserve the public exact-version compatibility warning. Extend `scripts/set-supported-codex-version.sh`, `build.rs`, and `tests/workflow_contract.rs` from stable-only parsing to canonical full SemVer. Rust validation uses `semver::Version::parse` plus exact round-trip equality; the shell setter implements the complete ASCII SemVer 2.0 grammar and rejects noncanonical numeric identifiers, empty identifiers, unsafe delimiters, whitespace, controls, slash, and backslash.

Run `scripts/set-supported-codex-version.sh 0.150.0-alpha.12.2`, then recapture `tests/fixtures/app-server-contract.json` through `/opt/codex-desktop/resources/codex`. Do not hand-edit the version or fixture. The fixture's schema digest and native exemplars are version-bound compatibility evidence.

README generated copy must say that the native app-server protocol was validated against the pinned version. It must not claim that this Desktop-bundled prerelease is the normal Codex CLI required on `PATH`; Codex CLI 0.149.1 remains the separate plugin-host target.

## Exact Thirteen-Tool Contract

The catalog contains exactly these tools, in existing order:

1. `thread_create`
2. `thread_fork`
3. `threads_list`
4. `thread_read`
5. `threads_wait`
6. `thread_message_send`
7. `thread_title_set`
8. `thread_goal_get`
9. `thread_goal_set`
10. `thread_goal_pause`
11. `thread_goal_resume`
12. `thread_goal_clear`
13. `thread_interrupt`

For every tool, preserve its name, order, title, description, annotations, input schema, requiredness, defaults, validation, native mapping, result schema, field names, error category, error shape, error stage, and dispatch evidence.

Do not filter by host. Desktop, Codex CLI, and generic stdio clients receive the same catalog. Preserve caller/target separation, cross-thread authorization, descendant interruption, goal semantics, wait snapshots, timeout behavior, and non-mutation on rejected requests.

### Persisted `notLoaded` resume

`thread_message_send` must perform this sequence:

1. Compact-read the exact requested thread ID before mutation.
2. Keep `Idle` and `SystemError` on the existing `turn/start` path.
3. Split `NotLoaded` into a resume-before-start path.
4. On the same initialized connection, call `thread/resume` with exactly `{"threadId": <requested-id>, "excludeTurns": true}`.
5. Parse the returned `thread` with the normal native parser.
6. Require the returned ID to equal the requested ID.
7. Reject a result that remains `notLoaded`.
8. Treat returned `Active` as `NativeConflict`; do not steer or start another turn.
9. Treat malformed data and identity drift as `NativeError` at `thread/resume`, with the requested ID attached.
10. Only after validation, use existing `turn/start` with the original prompt, model, effort, and `CompactThreadRead` reconciliation.

Resume failure, mismatch, still-unloaded state, or active race dispatches no prompt. `thread/resume` remains a preparatory request. If its transport fails after loading, report failure and send no prompt; retry re-snapshots before `turn/start`.

The prompt is dispatched exactly once and only through `turn/start`. Never replace the target or create a fallback thread.

All mutations retain dispatch-state-before-write behavior. Future-operation reconnect and read-only reconciliation must never replay create, fork, message, title, goal, or interrupt mutations.

## Packaging and Lifecycle

Use this repository-root marketplace layout:

```text
<clone>/
├── .agents/plugins/marketplace.json
├── plugins/codex-session-control/
│   ├── .codex-plugin/plugin.json
│   ├── .mcp.json
│   └── bin/codex-session-control
└── scripts/install-local-plugin.sh
```

The generated binary is gitignored and mode `0755`. Codex's cache must contain a regular copied file, not a symlink. Marketplace name is `codex-session-control-local`, source is `./plugins/codex-session-control`, and plugin version is concrete and synchronized with `Cargo.toml`.

The initial `.mcp.json` is exactly:

```json
{
  "mcpServers": {
    "codex-session-control": {
      "type": "stdio",
      "command": "./bin/codex-session-control",
      "cwd": ".",
      "env_vars": [
        "XDG_RUNTIME_DIR",
        "CODEX_LINUX_APP_ID",
        "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET"
      ],
      "tool_timeout_sec": 86460
    }
  }
}
```

Do not add `args`, literal endpoint values, `CODEX_HOME`, or extra forwarding. Do not switch to v1 root `mcp.json` until a supported Codex version forwards all three variables.

### Installer

`scripts/install-local-plugin.sh` must:

1. Resolve the clone root from its own path.
2. Reject hosts other than x86-64/amd64 and AArch64/arm64 before Codex mutation.
3. Run `cargo build --release --locked` in the clone.
4. Verify a regular executable matching the current host architecture.
5. Stage through a temporary regular file in the plugin `bin` directory.
6. Set mode `0755` and atomically rename to `bin/codex-session-control`.
7. Validate manifests and reject placeholders or path escape.
8. Add this canonical clone root only when the marketplace name is absent.
9. No-op when that name already targets the same root.
10. Fail before plugin mutation when that name targets another root.
11. Run `codex plugin add codex-session-control@codex-session-control-local`.

It must not download, read release metadata, verify downloads, select prebuilts, ship a dispatcher, manage a service, edit legacy state, or delete the clone.

Update means update the clone, rerun the installer, and start a new normal CLI session or Desktop task. Restart Desktop only when its build or feature environment changes, not merely to refresh CSC. Codex owns enable/disable through `/plugins` and the Desktop Plugins UI.

Removal uses `codex plugin remove codex-session-control@codex-session-control-local` and optionally `codex plugin marketplace remove codex-session-control-local`. It must not remove the clone or built binary.

Generic clients launch `<clone>/plugins/codex-session-control/bin/codex-session-control` directly and provide either the explicit socket or XDG/app-ID inputs. Runtime and catalog are identical to the plugin.

Host lifecycle behavior is explicit:

| Action | Normal Codex CLI | Desktop | Existing tasks/processes |
| --- | --- | --- | --- |
| Install | Run the checkout installer, then start a new CLI session. | **[MANUAL]** Verify the same installed plugin appears in Plugins UI, then start a new Desktop task. | Installation does not hot-load an existing task. |
| Disable | **[MANUAL]** Toggle with `/plugins`, then start a new session. | **[MANUAL]** Toggle off in Plugins UI, then start a new task. | Do not promise that toggling terminates an MCP process already loaded by an active task. |
| Re-enable | **[MANUAL]** Toggle with `/plugins`, then start a new session. | **[MANUAL]** Toggle on in Plugins UI, then start a new task. | A new task/session is the verification boundary. |
| Update | Update the checkout, rerun the installer, then start a new session. | Start a new Desktop task after restaging. | Existing tasks may keep the old executable until they end. |
| Remove | Run native plugin removal; optionally remove the marketplace afterward. | **[MANUAL]** Verify removal in Plugins UI and a new task. | Removal must not delete the clone and is not claimed to terminate already-running task processes. |
| Generic host | Restart or recycle the host's stdio child after updating. | Not applicable. | No Codex registration is involved. |

Manual acceptance must cover both normal CLI and Desktop for install visibility, exact catalog presence, disable absence, re-enable presence, same-version refresh, version-bump refresh, removal absence, and new-task/session behavior. Desktop-launched CSC must receive the exact three forwarded variables. Retain the evidence without modifying any pre-existing task.

`docs/upgrading.md` is the release-note-equivalent upgrade path and must contain this verified manual sequence:

1. **[MANUAL]** Disable or remove the old CSC plugin and stop/disable the old CSC user service using commands proven against the installed 0.3.x state.
2. **[MANUAL]** Start the upstream Desktop build with `shared-app-server-socket` enabled and verify the private socket exists.
3. Run the new checkout-local installer and verify native marketplace/plugin registration.
4. **[MANUAL]** Relaunch Desktop and normal Codex CLI tasks so they load the newly staged MCP executable.
5. Verify the exact thirteen-tool catalog and absence of the old CSC service/process authority.

Final commands and state checks must come from fresh post-implementation evidence. Do not publish guessed cleanup commands and do not add migration behavior to the installer.

## Exact File Targets

### Create

- `.agents/plugins/marketplace.json`
- `plugins/codex-session-control/.codex-plugin/plugin.json`
- `plugins/codex-session-control/.mcp.json`
- `plugins/codex-session-control/bin/.gitignore`
- `scripts/install-local-plugin.sh`
- `src/app_server/endpoint.rs`
- `tests/desktop_shared_socket_contract.rs`
- `tests/plugin_packaging_contract.rs`
- `docs/upgrading.md`

### Modify

- `Cargo.toml`, `Cargo.lock`, `build.rs`
- `supported-codex-version.txt`
- `src/main.rs`, `src/mcp.rs`, `src/mcp/operations.rs`
- `src/mcp/tests/support.rs`, `src/mcp/tests/mutation_mapping.rs`
- `src/mcp/tests/descendant_interrupt.rs`, `src/mcp/tests/goal_matrix.rs`
- `src/mcp/tests/outcome_unknown.rs`, `src/mcp/tests/read_tools.rs`
- `src/mcp/tests/threads_wait.rs`, `src/mcp/tests/timeout.rs`
- `src/app_server.rs`, `src/app_server/tests.rs`
- `src/app_server/tests/transport.rs`, `src/app_server/tests/live_capture.rs`
- `src/model.rs`, `src/error.rs`
- `tests/mcp_contract.rs`, `tests/workflow_contract.rs`
- `tests/fixtures/app-server-contract.json`
- `tests/app_server_integration.rs`
- `tests/app_server_integration/cases.rs`
- `tests/app_server_integration/live_harness.rs`
- `scripts/check.sh`, `scripts/set-supported-codex-version.sh`
- `.github/workflows/ci.yml`
- `.github/ISSUE_TEMPLATE/bug.yml`
- `README.md`, `CONTRIBUTING.md`, `SECURITY.md`
- `docs/architecture.md`, `docs/desktop.md`, `docs/security.md`, `docs/troubleshooting.md`

### Delete

- `src/install.rs`, `src/install/`
- `src/desktop.rs`, `src/desktop/`
- `src/cli.rs`, `src/cli_output.rs`, `src/diagnostics.rs`
- `assets/systemd/codex-session-control.service.in`
- `assets/marketplace/.agents/plugins/marketplace.json`
- `assets/marketplace/plugins/codex-session-control/.codex-plugin/plugin.json`
- `assets/marketplace/plugins/codex-session-control/.mcp.json`
- `install.sh`, `scripts/ci/disposable-systemd-user-contract.sh`
- `tests/cli_contract.rs`, `tests/cli_contract/`
- `tests/app_server_integration/normal_home.rs`
- `tests/app_server_integration/normal_home_paths.rs`
- `tests/app_server_integration/protocol_support.rs` after moving only the retained direct native connection seam into `tests/app_server_integration/live_harness.rs`
- `.github/workflows/release.yml`, `.github/workflows/publish.yml`

No production or packaging file outside these targets may change without updating the implementation plan against this specification.

## Verification

Run focused gates first:

```bash
cargo test --locked --bin codex-session-control app_server::endpoint::tests::
cargo test --locked --bin codex-session-control app_server::tests::transport::websocket_upgrade_uses_exact_rpc_path -- --exact
cargo test --locked --bin codex-session-control app_server::tests::transport::
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::not_loaded_message_
cargo test --locked --bin codex-session-control mcp::tests::outcome_unknown::
cargo test --locked --test workflow_contract tested_codex_version -- --nocapture
cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
cargo test --locked --test desktop_shared_socket_contract
cargo test --locked --test plugin_packaging_contract
./scripts/check.sh
```

Endpoint tests cover precedence, app-ID fallback, missing XDG, relative paths, traversal, symlinks, absence, wrong type, owner/mode failures, and replacement.

Persisted-message tests cover success ordering, different ID, still-unloaded result, active race, native failure, idle/system-error behavior, and zero dispatch on rejection.

Mutation tests prove one write maximum, unchanged `outcome_unknown`, read-only reconciliation, and later-operation recovery after replacement.

Packaging tests use isolated `HOME` and `CODEX_HOME`. Prove parsing, exact environment forwarding, mode, atomic restaging, marketplace collision safety, install, same-version reinstall, version-bump refresh, removal, clone retention, and generic `initialize` plus `tools/list` from another working directory.

The completed isolated prerequisite spike already proved relative contained execution, all three forwarded environment values, mode preservation through the private cache, same-version and version-bump refresh behavior, native removal, and the expected negative result for v1 environment forwarding. Reproduce those contracts in repository-owned tests before deleting the old installer. Desktop-only UI and new-task lifecycle gates remain manual.

Canonical full-SemVer verification must accept `0.150.0-alpha.12.2` and representative build metadata; reject leading-zero prerelease identifiers, empty identifiers, traversal delimiters, whitespace, controls, slash, and backslash; and preserve transactional rollback. Recapture the protocol fixture from the exact Desktop binary:

```bash
./scripts/set-supported-codex-version.sh 0.150.0-alpha.12.2

env \
  PATH="/opt/codex-desktop/resources:$PATH" \
  CODEX_SESSION_CONTROL_FIXTURE_OUT="$PWD/tests/fixtures/app-server-contract.json" \
  cargo test --locked --bin codex-session-control \
    app_server::tests::live_capture::capture_protocol_fixture -- \
    --exact --ignored --nocapture
```

The fake native server records the WebSocket upgrade request and rejects anything except exact `/rpc`. The retained live harness also uses `ws://localhost/rpc`.

Native x86-64 and AArch64 CI each run the locked release build, installer staging tests, ELF assertion, direct stdio catalog test, and full Rust gate. Cross-compilation is not acceptance evidence.

The live gate is exactly one ignored end-to-end test:

```bash
CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS=1 \
cargo test --locked --test app_server_integration \
  live_desktop_authority_all_thirteen_tools_are_disposable \
  -- --ignored --exact --nocapture --test-threads=1
```

It fails before mutation unless the opt-in equals `1`, the production endpoint validates, the initialized authority version is supported, and a new random run workspace contains no active or archived tasks. Before use, persist every created or forked ID into a run-scoped `owned-thread-ids.json` under a private `/tmp/codex-session-control-live-*` directory using temp-file, `fsync`, atomic rename, and parent-directory `fsync`. All mutating helpers accept only a test-only `OwnedThreadId`; unrecorded IDs are never mutation targets.

Drive the built CSC binary over stdio MCP, assert the exact catalog, then exercise all thirteen tools on the ledger-owned tasks. On normal error or panic, unconditionally stop/reap the MCP child, recover only IDs belonging to the exact unique workspace, and persist them. Then, over a fresh native connection, exact-read each ledger ID, validate exact ID equality, and classify storage only by initialized `codexHome` subtree (`sessions` or `archived_sessions`). Skip IDs already classified archived, archive only IDs classified active, then poll exact reads until every target is classified archived. Fail closed for missing, mismatched, or unclassifiable targets. `thread/list` stays in the recovery flow only for exact-workspace ownership and never as archive-state proof because preview-empty threads can be omitted there. Delete the run directory only after exact-read proof confirms all targets are archived. On cleanup failure, retain and report the exact ledger path and remaining IDs.

Hard-kill recovery requires both `CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS=1` and `CODEX_SESSION_CONTROL_LIVE_RECOVER_LEDGER=/absolute/path/owned-thread-ids.json`. Never scan `/tmp`, titles, timestamps, or global before/after diffs for ownership.

Run reviews in this order:

1. Specification compliance.
2. Dedicated DRY/YAGNI review.
3. Code quality.

The DRY/YAGNI review inventories every new production module, helper, dependency, installer branch, compatibility path, and test layer. Each addition must have one concrete responsibility not already covered by retained code or a smaller deletion-oriented design. Delete redundant abstractions, speculative extension points, duplicate manifest/config sources, unused fallback machinery, equivalent tests, and compatibility code outside the approved current architecture. Test count or changed-line coverage alone never justifies a test.

Reader-facing workflow checks must prove that README, current docs, contribution guidance, and issue forms do not instruct users to invoke removed CSC commands. `docs/upgrading.md` may name a historical 0.3.x command only when explicitly labeled as one verified manual cleanup option. The bug form must request evidence that exists in the new product: host surface, plugin version/visibility, Desktop shared-socket availability, exact stderr or MCP error, and reproduction steps.

Fix accepted findings and rerun affected gates. Only fresh post-change results count.

## Acceptance Criteria

1. The binary starts stdio MCP directly and exposes exactly thirteen tools.
2. Catalog, schemas, annotations, results, errors, and ordering preserve the pre-refactor contract.
3. No CSC authority, service, socket, lock, descriptor, wrapper, updater, or saved target state remains.
4. Every operation freshly resolves and security-validates the Desktop endpoint.
5. Explicit bridge socket wins over XDG/app-ID derivation.
6. Missing explicit socket plus missing XDG fails `target_unavailable` without scanning or fallback.
7. Exact lexical, app-ID, ownership, symlink, type, and mode predicates are table-tested; accepted socket modes are only `0600` and `0700`.
8. Unsafe endpoint metadata or races fail `authority_transport_failure` without leaking paths or environment values.
9. Production and retained harnesses upgrade WebSocket on exact `/rpc`; `/` and query variants fail.
10. The next independent call recovers from Desktop socket replacement without restarting the MCP host.
11. An in-flight wait is not silently replayed after disconnect.
12. An ambiguous mutation is never replayed and remains `outcome_unknown` with dispatch evidence.
13. `thread_message_send` resumes a persisted `notLoaded` exact-ID target on the same connection before start.
14. Every failed or invalid resume path sends zero prompts.
15. Successful persisted messaging sends the original prompt exactly once.
16. The canonical full-SemVer pipeline pins and recaptures the exact Desktop authority `0.150.0-alpha.12.2` fixture while preserving public mismatch warnings.
17. The plugin uses legacy manifests and forwards exactly the three required variables; the v1 format remains excluded by a negative regression test.
18. One installer builds locked and atomically stages one mode-0755 native binary.
19. No download logic, checksum catalog, release selector, separately installed executable outside the plugin bundle, or architecture dispatcher remains.
20. Native x86-64 and AArch64 CI each build and execute the staged binary.
21. Normal Codex CLI and Desktop receive the same thirteen-tool catalog in newly started sessions/tasks.
22. A generic stdio client initializes through the stable clone path with no Hermes-specific mode.
23. Rerunning the installer refreshes same-version and version-bump cache content without a CSC updater or legacy-state mutation.
24. Manual CLI/Desktop lifecycle evidence proves install, disable, re-enable, update, removal, and new-task/session visibility boundaries.
25. Native removal removes Codex registration and tools without deleting the clone or claiming termination of already-running task processes.
26. Documentation gives the implementation-verified five-step manual 0.3.x cutover and contains no migration-code claim.
27. Live validation exercises all tools only on run-ledger-owned disposable tasks and archives exactly those tasks on success or recoverable failure.
28. Every added production or test construct survives a dedicated DRY/YAGNI review with a concrete unique responsibility; unjustified additions are removed.
29. No current reader-facing workflow or issue form advertises a removed CSC command; the bug form requests evidence available from the plugin-contained product.
30. `./scripts/check.sh`, specification-compliance review, DRY/YAGNI review, and code-quality review pass with fresh evidence in that order.
31. Phase 1 stops after the internal CSC branch push and pull request; no CSC merge, tag, release, or publish occurs.
32. Phase 2 opens only an internal Desktop-fork pull request and never contacts official upstream without later explicit authorization.
