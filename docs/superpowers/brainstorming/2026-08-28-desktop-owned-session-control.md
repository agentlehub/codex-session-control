# Brainstorming: Desktop-Owned Session Control

**Status:** approved

## Context

Codex Session Control currently owns a separate Codex app-server authority through a systemd user service and supports Desktop only through the downstream `agentlehub/codex-desktop-linux` external-attachment feature. That architecture is no longer wanted. The installed Desktop now comes from unmodified `ilysenko/codex-desktop-linux` main with its upstream `shared-app-server-socket` feature enabled. The live Desktop-owned authority is listening at `/run/user/1000/codex-desktop/app-server-bridge/app-server.sock`; the parent directory is mode `0700` and the socket is mode `0600`.

The installed Codex Session Control 0.3.2 plugin and service are disabled. Its source checkout is `/home/korty/dev/agentlehub/codex-session-control` at `88f2ac5`, matching `origin/main`. The current Rust source contains 36,698 lines including tests. The `install` and `desktop` trees alone contain about 23,000 lines, largely implementing the service-owned authority, updater, wrapper, downstream Desktop discovery, descriptor publication, and lifecycle safety that the new architecture can delete. The reusable app-server and MCP trees contain about 8,100 lines including tests and already implement the mature 13-tool contract, protocol normalization, validation, mutation ambiguity handling, waiting, and cross-thread safety.

The historical commit `114ba407061de8cccee4886a28ab41ad3558346d` proved that a stdio MCP server can reconnect through `shared-app-server-socket`, call native methods on the same Desktop authority without deadlock, and expose six cross-thread goal/interrupt tools. It was intentionally a non-production JavaScript spike. Current Codex Session Control is substantially more complete: thirteen tools, Rust packaging, typed results, compatibility checks, and extensive regression coverage.

Codex CLI 0.149.1 reports the stable `goals` feature, so it is wrong to say that CLI has no goal support at all. Its built-in goal tools are current-thread oriented. It does not provide Desktop's native cross-thread app-management surface. Official OpenAI documentation confirms that Desktop, Codex CLI, and the IDE share host MCP configuration, so one enabled Session Control plugin can expose the same thirteen tools in both Desktop and CLI. Under the proposed model, those tools work only while Desktop owns the shared authority.

Two reviewed but unmerged commits on `codex/fix-enable-persisted-thread-recovery` contain one relevant behavior improvement—resuming persisted `notLoaded` threads before messaging—and a much larger body of service-lifecycle hardening that becomes obsolete. The redesign should preserve or selectively port the messaging behavior, not adopt the obsolete lifecycle work wholesale.

The operator's governing optimization is the smallest maintainable codebase and smallest user-facing surface that preserve the complete thirteen-tool contract in both Desktop and Codex CLI. Compatibility with the existing Session Control management commands is not a requirement. Native Codex can own plugin enablement and disablement, and a normal `codex` launch loads enabled plugin tools in a new CLI session. Codex 0.149.1's Agent Plugins v1 MCP format accepts a contained `./` executable and resolves it from the installed plugin root, so the retained Rust MCP server can potentially be packaged inside the plugin instead of installed separately in `~/.local/bin`. That would let native plugin installation, versioned cache refresh, and removal own the whole product and could eliminate all CSC lifecycle commands. The specification must not assume this packaging until a focused prerequisite spike proves executable-mode preservation, x86-64/AArch64 dispatch, required environment forwarding, version-bump refresh, enable/disable, and removal on both Desktop and CLI surfaces.

Attaching a CLI session itself to Desktop's authority is distinct from exposing Session Control tools in a normal CLI session. The upstream `codex-desktop-linux` shared-socket feature could justifiably add a generic attached-CLI command because that distribution owns the socket path, app identity, bundled CLI version, and authority lifecycle. Acceptance is not guaranteed, however, because the stock `codex --remote unix://...` command already provides the primitive and a maintainer may reasonably prefer documentation over another public command. Session Control must not depend on that upstream enhancement: plain `codex` must receive all thirteen tools, while an attached-CLI launcher remains optional integration owned outside CSC.

Hermes is a normal stdio MCP host, not a special integration target. It accepts an arbitrary executable through `mcp_servers.<name>.command`, accepts arguments and explicit environment variables, preserves the safe `XDG_*` baseline needed by Session Control's deterministic socket lookup, resolves absolute and `~`-relative commands, and transparently restarts a recycled stdio server. Therefore generic stdio-client support does not require another authority, service, server implementation, or tool catalog. Its incremental cost is a stable documented executable path and one generic-host compatibility test, both of which overlap with the local plugin installation problem.

## Current Decisions

- Preserve the complete thirteen-tool Session Control catalog. [Q1](#q1-which-session-control-tools-must-remain)
- Desktop owns the only app-server authority; operation while Desktop is closed is not required. [Q2](#q2-must-session-control-work-while-desktop-is-closed)
- Depend on the upstream `shared-app-server-socket`; do not require the personal Desktop fork or external-attachment feature. [Q3](#q3-which-desktop-build-may-session-control-require)
- Refactor the existing Rust implementation on `feat/desktop-owned-session-control`; do not rewrite it. Complete a reviewed specification and reviewed implementation plan before production-code changes. [Q4](#q4-should-the-redesign-refactor-the-current-rust-source-or-start-over)
- Preserve persisted-thread messaging by resuming a valid `notLoaded` target before starting its turn. [Q5](#q5-what-must-happen-to-the-unmerged-persisted-thread-resume-fix)
- Resolve the Desktop-owned socket deterministically from the upstream environment contract without persistent CSC configuration or runtime scanning. [Q6](#q6-how-should-session-control-find-the-desktop-owned-shared-socket)
- Do not make CSC depend on an attached-CLI launcher; defer a generic upstream `codex-desktop-linux` enhancement until the refactor is complete. [Q7](#q7-must-the-csc-refactor-provide-an-attached-cli-launcher)
- Preserve the exact current thirteen-tool MCP names, schemas, result shapes, and error semantics. [Q8](#q8-how-strict-is-the-public-mcp-contract)
- Prefer a plugin-contained executable; if the prerequisite spike disproves it, retain the minimum standalone-executable installation needed by the same MCP server. [Q10](#q10-where-should-the-mcp-executable-live)
- Defer registry-backed distribution. The first delivery is a local plugin installed from a cloned checkout, with the least command-driven setup the platform permits. [Q11](#q11-how-will-the-first-version-be-distributed-and-installed)
- Expose no CSC lifecycle commands unless a command is technically necessary to install or operate the local plugin. [Q12](#q12-what-public-command-surface-may-remain)
- Support Linux x86-64 and AArch64 from the first refactored release. [Q13](#q13-which-architectures-must-the-local-plugin-support)
- Support only the current upstream Desktop shared-socket architecture; retain no legacy external-attachment or service-owned-authority compatibility. [Q14](#q14-which-legacy-architectures-must-remain-compatible)
- Write no migration code. Publish a direct upgrade path in the release notes. [Q15](#q15-how-should-existing-installations-upgrade)
- Recover automatically from Desktop restart and socket replacement while the MCP host remains alive. [Q16](#q16-how-should-session-control-handle-desktop-restarts)
- Permit disposable live tasks for complete thirteen-tool validation, with no mutation of existing user tasks. [Q17](#q17-what-live-validation-may-the-refactor-perform)
- Execute autonomously under an orchestrator using isolated subagents for discovery, implementation slices, and ordered review passes. [Q18](#q18-what-execution-and-review-gates-apply)
- Push the CSC feature branch and open a pull request in the internal CSC repository; do not release from this effort. [Q19](#q19-what-is-the-csc-delivery-boundary)
- Implement the attached-CLI enhancement last and open its first pull request only in the operator's internal Desktop fork for review. [Q20](#q20-where-should-the-deferred-attached-cli-enhancement-be-proposed)

## Q&A

### Q1: Which Session Control tools must remain?

**Recommended**

Keep the complete catalog: `thread_create`, `thread_fork`, `threads_list`, `thread_read`, `threads_wait`, `thread_message_send`, `thread_title_set`, `thread_goal_get`, `thread_goal_set`, `thread_goal_pause`, `thread_goal_resume`, `thread_goal_clear`, and `thread_interrupt`. Native Desktop overlap is not a reason to remove tools because Codex CLI lacks Desktop's `codex_app` host surface, and one consistent catalog is simpler than client-dependent filtering.

**Resolved**

The operator explicitly requires every Session Control tool to be preserved.

**Rationale**

Preserving one stable contract keeps CLI useful and avoids maintaining two catalogs. The five cross-thread goal tools and cross-thread interrupt also remain meaningful Desktop extensions even where native tools overlap with the other seven operations.

### Q2: Must Session Control work while Desktop is closed?

**Recommended**

No. Make the Desktop-owned shared authority a hard runtime dependency and return a clear target-unavailable error when its socket is absent.

**Resolved**

The operator is willing to drop standalone operation and accepts that Desktop must be open for Session Control and CLI attachment.

**Rationale**

This removes the separate app-server service, wrapper, authority reconciliation, duplicate session namespace, and most lifecycle code. It is the largest simplification available without removing tools.

### Q3: Which Desktop build may Session Control require?

**Recommended**

Require only the upstream `shared-app-server-socket` capability supplied by `ilysenko/codex-desktop-linux`; remove all coupling to the personal `agentlehub/codex-desktop-linux` fork and its `external-app-server-attachment` descriptor.

**Resolved**

The operator wants to keep upstream `codex-desktop-linux` for Linux-specific features such as computer use but does not want to maintain a personal Desktop fork.

**Rationale**

The shared socket is already upstream, live, private, and protocol-transparent. Session Control can be an ordinary stdio MCP client of that authority, exactly as the earlier spike proved.

### Q4: Should the redesign refactor the current Rust source or start over?

**Recommended**

Refactor the existing Rust implementation in place on a dedicated feature branch. Retain the app-server client, thirteen-tool MCP contract, typed protocol normalization, validation, ambiguous-mutation handling, and focused tests. Delete the service-owned authority, downstream Desktop attachment, and their obsolete lifecycle tests. Prefer packaging the retained MCP executable inside the plugin if the prerequisite packaging spike passes; otherwise reduce installation, update, and removal to the minimum needed to manage a standalone executable and native plugin registration. Whether and where to retain an attached-CLI launcher remains a separate user-surface decision. A new implementation would recreate the highest-risk behavior while discarding its evidence.

**Resolved**

The operator selected the existing-source refactor and requires it to proceed on the separate `feat/desktop-owned-session-control` feature branch with a proper specification and implementation plan before code changes.

**Rationale**

The existing core already implements the desired product; most size comes from the authority ownership model being removed. Aggressive subtraction produces a simpler result than a rewrite without resetting protocol correctness to zero. The branch now exists from `origin/main` commit `88f2ac5` and contains only this in-progress brainstorming artifact.

**Approaches Considered**

1. **Refactor the existing Rust source** — Recommended because it preserves the complete contract and its regression evidence while enabling deletion of roughly 23,000 lifecycle-oriented lines.
2. **Create a new Rust implementation** — Rejected because the apparent repository cleanliness would be bought by reimplementing protocol framing, native mappings, validation, wait semantics, error classification, and mutation safety.
3. **Promote the historical JavaScript spike** — Rejected because the spike exposes only six tools and explicitly lacks production packaging, compatibility, authorization, and operator-facing failure behavior.

### Q5: What must happen to the unmerged persisted-thread resume fix?

**Recommended**

Preserve the behavioral contract, not the obsolete branch wholesale. When `thread_message_send` targets a persisted thread that is readable/listed but reported as `notLoaded`, Session Control must call exact-ID `thread/resume`, validate the returned identity and loaded state, and only then start the turn. Resume failure, identity mismatch, or a target that remains unloaded must fail without sending or replaying the message.

**Resolved**

The operator explicitly requires this behavior not to be forgotten during the redesign.

**Rationale**

Desktop ownership does not eliminate persisted unloaded threads. The fix belongs to the retained MCP/protocol core, while the same unmerged branch's service enablement, restart evidence, and authority lifecycle changes belong to the architecture being deleted. The specification and implementation plan must name this behavior and its non-dispatch regression tests explicitly.

### Q6: How should Session Control find the Desktop-owned shared socket?

**Recommended**

Use `CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET` when it is set; otherwise derive `${XDG_RUNTIME_DIR}/${CODEX_LINUX_APP_ID:-codex-desktop}/app-server-bridge/app-server.sock`. Forward the two optional override variables and `XDG_RUNTIME_DIR` through the plugin manifest. Canonicalize and validate the parent directory and socket ownership, type, and permissions before every connection. Do not keep a CSC socket-path configuration file and do not scan processes or runtime directories.

**Resolved**

The operator selected deterministic upstream path resolution plus environment overrides and reiterated that the overall codebase should be as small as possible while preserving existing features other than external app-server ownership.

**Rationale**

This is the upstream feature's public runtime contract and supports both the default installation and explicit app-ID/socket overrides. It removes saved configuration, discovery heuristics, and contradictory state while retaining strict local endpoint validation.

**Approaches Considered**

1. **Deterministic path plus environment overrides** — Recommended because it is stateless, explicit, upstream-aligned, and secure.
2. **Retain a small CSC configuration file** — Rejected because it duplicates upstream state and creates configuration drift, migration, and repair logic without adding a required capability.
3. **Scan processes or runtime directories** — Rejected because discovery becomes ambiguous with multiple app IDs, relies on unstable process details, and expands the security validation surface.

### Q7: Must the CSC refactor provide an attached-CLI launcher?

**Recommended**

No. A normal `codex` session must receive all thirteen Session Control tools while remaining a separate CLI-owned task. Attaching the CLI session itself to Desktop's authority through stock `codex --remote unix://...` is optional integration owned by `codex-desktop-linux`, not part of CSC's required behavior.

**Resolved**

The operator agreed that the CSC refactor must not depend on upstream accepting a launcher. Work on a generic upstream attached-CLI command is deferred to the final step after the refactor is complete.

**Rationale**

This keeps the core switch independently shippable and removes the personal Desktop fork without sacrificing any Session Control tool. The upstream enhancement has a credible generic use case but remains a convenience over an existing stock CLI primitive, so its acceptance cannot be treated as a prerequisite.

### Q8: How strict is the public MCP contract?

**Resolved**

Preserve the exact current thirteen-tool names, input schemas, output shapes, and error semantics. The refactor may delete internal implementation and lifecycle surfaces, but must not silently change the MCP contract.

### Q9: Which MCP hosts must be supported?

**Recommended**

Support both native Codex plugin hosts and arbitrary conforming stdio MCP clients, including Hermes. The same executable and tool catalog must serve both; there must be no Hermes-specific protocol mode or second implementation.

**Resolved**

Support arbitrary conforming stdio MCP clients, including a future Hermes integration. This refactor must retain the generic stdio protocol boundary and provide a stable executable entrypoint that such clients can launch. Hermes-specific configuration, installation automation, documentation, and live compatibility validation are deferred.

**Rationale**

This is low incremental complexity. The retained component is already a stdio MCP server, and Hermes already launches arbitrary stdio servers with the environment needed for deterministic shared-socket discovery. The only meaningful extra requirement is that the local installation expose a stable executable path outside Codex's versioned plugin cache. That installation concern already exists for building and staging the plugin-contained executable. Generic-host support adds focused compatibility coverage and documentation, not another runtime architecture.

### Q10: Where should the MCP executable live?

**Resolved**

Prefer an executable contained inside the installed plugin. A prerequisite spike must prove executable permissions, architecture dispatch, environment forwarding, cache refresh, enable/disable, and removal. If the spike fails, use the minimum standalone-executable approach without reviving the service-owned authority, wrapper, updater, or management suite.

### Q11: How will the first version be distributed and installed?

**Resolved**

Defer npm or other registry-backed distribution. The first version is a local plugin installed from a cloned repository. Prefer native command-free installation if Codex exposes a reliable user-facing path for local personal plugins; otherwise provide one documented installation command that builds or stages the correct architecture binary and registers the local plugin. Do not pretend that merely cloning a repository makes a personal plugin globally discoverable when the host requires registration.

### Q12: What public command surface may remain?

**Resolved**

Target zero CSC lifecycle or management commands. Native Codex owns plugin enablement and disablement. Retain only commands proven necessary to install or operate the local plugin; an installation script is not a standing runtime management CLI.

### Q13: Which architectures must the local plugin support?

**Resolved**

Support both Linux x86-64 and AArch64 in the first refactored delivery. The packaging spike and final verification must cover both architecture-selection paths even when only one architecture can be executed on the development host.

### Q14: Which legacy architectures must remain compatible?

**Resolved**

Support only the current Desktop-owned `shared-app-server-socket` architecture. Delete compatibility with the CSC-owned app-server service, the personal Desktop fork's external-attachment descriptor, legacy wrappers, and their state formats.

### Q15: How should existing installations upgrade?

**Resolved**

Write no migration code. Release notes must provide the exact manual upgrade path: remove or disable the old CSC service/plugin installation, install the new local plugin, start upstream Desktop with the shared socket enabled, and relaunch clients so the new tool process is loaded. The final wording must be derived from the actual implemented installer and verified state, not guessed in advance.

### Q16: How should Session Control handle Desktop restarts?

**Resolved**

The MCP server must automatically recover when Desktop restarts or replaces its socket while the MCP host remains alive. An in-flight mutation with an ambiguous result must retain the current no-blind-replay safety contract. Recovery may reconnect future operations, but must never replay a possibly accepted mutation.

### Q17: What live validation may the refactor perform?

**Resolved**

Create clearly named disposable tasks to exercise all thirteen tools against the live Desktop-owned authority and archive them afterward. Do not rename, interrupt, message, retarget, or otherwise mutate any pre-existing user task. Automated coverage remains mandatory; live validation complements rather than replaces it.

### Q18: What execution and review gates apply?

**Resolved**

Proceed autonomously from the approved brainstorming artifact through specification, implementation plan, implementation, tests, specification-compliance review, and code-quality review. The operator explicitly overrides separate specification approval, plan approval, and pre-implementation reconfirmation pauses for this refactor and explicitly authorizes review subagents. The primary agent acts as orchestrator and delegates broad discovery, bounded implementation slices, and review passes to isolated subagents so its context remains focused on architecture, decisions, integration, and evidence.

### Q19: What is the CSC delivery boundary?

**Resolved**

Commit and push `feat/desktop-owned-session-control`, then open or update a pull request in the operator's internal Codex Session Control repository. Do not merge, tag, publish a release, or perform registry distribution as part of this effort.

### Q20: Where should the deferred attached-CLI enhancement be proposed?

**Resolved**

After the CSC refactor and its pull request are complete, implement the generic attached-CLI enhancement in the Desktop repository. Open the first pull request only against the operator's internal Desktop fork so the operator can review it. Do not open a pull request, issue, or other proposal against the official upstream repository without a later explicit instruction.
