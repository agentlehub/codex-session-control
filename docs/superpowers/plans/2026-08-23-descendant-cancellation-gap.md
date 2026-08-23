# Descendant Cancellation Gap Implementation Plan

**Status:** approved
**Source:** `docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md`
**Review:** `docs/superpowers/reviews/2026-08-23-descendant-cancellation-gap-plan-review.md`
**Next:** `docs/handoffs/2026-08-23-030753-descendant-cancellation-gap-implementation.md`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `thread_interrupt` with opt-in, all-depth spawned-descendant interruption while preserving exact-root semantics, truthful partial results, deterministic ordering, and at-most-once mutation.

**Architecture:** Preserve the existing exact snapshot/interruption/reconciliation path as one private helper. After the root succeeds, reuse its connection for a fully paginated private descendant query, then either return an exact-scope warning or use `futures_util::future::join_all` over independently initialized descendant connections; serialize outcomes in first-seen discovery order.

**Tech Stack:** Rust 1.95, Tokio, `futures-util` 0.3.33, Schemars/Serde, the existing Unix-WebSocket App Server client, and the existing fake App Server harness.

---

## Prerequisites

- **[MANUAL]** The operator must approve this reviewed plan before implementation begins.
- Begin from branch `fix/descendant-cancellation-gap` with the approved planning boundary recorded by the implementation handoff, merge-base `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`, and a clean `git status --short --branch`.
- Confirm the source spec remains `Status: approved`, is not superseded, and has no unresolved `BLOCKER` or `MAJOR` review outcome.
- Use the repository Rust toolchain and locked dependency graph. `futures-util = 0.3.33` and Tokio's `test-util` feature already exist; do not edit `Cargo.toml` or `Cargo.lock`.
- Use only fake App Server tests. Do not mutate a live Codex session, restart the controller, publish, push, open a PR, or change protected refs.
- Stop before editing if the worktree is dirty with unrelated changes or if implementation requires a new dependency, public lineage API, transport redesign, protocol-model expansion, or any production file outside the approved list.
- Never retry an `outcome_unknown`; preserve the existing mutation dispatch and read-only reconciliation evidence.

## Acceptance Criteria

1. **AC1 — Exact-scope compatibility:** omitted/false `includeDescendants` retains the exact root result and mutation order, never connects to or mutates descendants, returns the ordered structured warning for every active spawned descendant, and omits `descendants` for an empty target set.
2. **AC2 — Authoritative discovery:** calls exhaust native `thread/list` pages using only `ancestorThreadId`, `sourceKinds: ["subAgentThreadSpawn"]`, and the prior cursor before descendant mutation; active IDs are retained once in stable first-seen order.
3. **AC3 — Independent concurrent handling:** every eligible non-caller target gets an independent initialized connection and fresh compact snapshot; operations overlap, failures do not fail fast, and completion order cannot reorder results.
4. **AC4 — Caller protection:** a discovered caller target receives the existing `policy_rejected` / `self_target` entry without a connection while other targets continue.
5. **AC5 — Error boundary:** root errors remain top-level and stop discovery; discovery errors are flattened under `descendants.error` beside the completed root result and cause zero descendant mutation.
6. **AC6 — Truthful target results:** inactive refreshes return `interrupted:false`; successes, native errors, connection/snapshot errors, timeouts, and uncertain outcomes keep exact target attribution and all applicable existing evidence, with no mutation retry.
7. **AC7 — Scope containment:** goal state, public `threads_list`, `Thread`, protocol models, transport architecture, unrelated tools, dependencies, and descendants activated after discovery remain unchanged.
8. **AC8 — Proof:** every TDD slice produces an executable RED assertion before its GREEN change; focused suites and the repository gate pass; spec-compliance review passes before code-quality review; one final post-review `./scripts/check.sh` passes.

## File Structure

| File | Responsibility |
| --- | --- |
| `src/mcp/contract.rs` | Optional input flag, trusted caller carrier, exact/descendant result types, tool text, and closed conditional output schema. |
| `src/mcp.rs` | Pass the validated caller ID into interrupt execution without changing request or top-level error handling. |
| `src/mcp/operations.rs` | Preserve exact interruption, paginate/select descendants, compose warning/discovery errors, and join isolated target operations in order. |
| `src/app_server.rs` | Send one private spawned-descendant `thread/list` page request and reuse the current native parser. |
| `src/mcp/tests.rs` | Register the focused descendant-interrupt module. |
| `src/mcp/tests/descendant_interrupt.rs` | Prove ordering, pagination, deduplication, warnings, concurrency, races, caller policy, and isolated failures. |
| `src/mcp/tests/support.rs` | Serve initialized fake connections concurrently and provide source-compatible deterministic response controls plus initialization-failure scripting. |
| `src/mcp/tests/validation.rs` | Prove input acceptance/rejection, defaulting, trusted caller separation, and direct self-target rejection. |
| `src/mcp/tests/mutation_mapping.rs` | Keep the old exact interruption path and serialization semantics under direct regression tests. |
| `tests/mcp_contract.rs` | Assert model-facing text and the complete closed input/output schema, including full `ToolErrorData` evidence. |
| `README.md` | Update only the `thread_interrupt` tool-summary row. |

No other file is in scope. In particular, do not change `src/error.rs`, `src/model.rs`, `src/app_server/protocol.rs`, `Cargo.toml`, `Cargo.lock`, or existing unrelated test modules.

## Review Milestones

- **Intermediate milestones:** none. Tasks 2-4 are compile-coherent TDD slices with their own executable RED and focused GREEN gates, not review ceremonies.
- **Pre-review readiness:** Task 5 runs the complete focused suite and `./scripts/check.sh`, satisfying the approved spec's repository-gate-before-review order.
- **Final review:** Task 6 covers Tasks 1-5 from the implementation base through the complete diff. Run spec-compliance review first and code-quality review second.
- **Final post-review gate:** Task 7 runs the planning handoff's required final `./scripts/check.sh`, then stages and commits only the approved implementation paths.
- Any code/test repair after the pre-review gate returns to affected focused checks, the Task 5 repository gate, and both Task 6 reviews in order. Any repair after Task 6 also repeats that sequence before the final gate.
- Stop for the operator only when a fix would change approved behavior, scope, architecture, dependency policy, or review policy.

## Tasks

### Task 1: Prepare the Deterministic Multi-Connection Harness

**Acceptance criteria:** AC3, AC6, AC7

**Files:**
- Modify: `src/mcp/tests/support.rs`

- [ ] **Step 1: Add source-compatible response controls**

Do not add fields to `FakeStep`; existing direct struct literals outside the allowlist must keep compiling. Add one wrapper variant instead:

```rust
#[derive(Clone, Debug)]
pub(super) enum FakeResponse {
    Result(Value),
    Error(Value),
    Pending,
    Disconnect,
    Controlled {
        response: Box<FakeResponse>,
        release: Option<Arc<tokio::sync::Notify>>,
        sent: Option<Arc<tokio::sync::Notify>>,
    },
}

impl FakeStep {
    pub(super) fn controlled(
        mut self,
        release: Option<Arc<tokio::sync::Notify>>,
        sent: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        self.response = FakeResponse::Controlled {
            response: Box::new(self.response),
            release,
            sent,
        };
        self
    }
}
```

In the connection handler, unwrap `Controlled` after request validation/logging and before any delay: await `release` when present, apply the existing response behavior, then call `sent.notify_one()` when present. Preserve the existing `delay` and native `notify_after` ordering. Reject nested `Controlled` wrappers with a test-harness panic instead of silently recursing.

- [ ] **Step 2: Add initialization-failure scripts and concurrent handlers**

```rust
#[derive(Clone, Debug)]
pub(super) enum FakeInitialize {
    Success,
    Disconnect,
}

#[derive(Clone, Debug)]
pub(super) struct FakeConnectionScript {
    pub(super) initialize: FakeInitialize,
    pub(super) steps: Vec<FakeStep>,
}

impl FakeConnectionScript {
    pub(super) fn initialized(steps: Vec<FakeStep>) -> Self {
        Self { initialize: FakeInitialize::Success, steps }
    }

    pub(super) fn disconnect_on_initialize() -> Self {
        Self { initialize: FakeInitialize::Disconnect, steps: Vec::new() }
    }
}
```

Implement `start_scripted_connections(Vec<FakeConnectionScript>)`; have `start_connections` wrap old scripts with `initialized`. After accepting and initializing each socket, spawn its existing step loop into a `tokio::task::JoinSet` and immediately accept the next script. The top-level fake task owns the `JoinSet`, so aborting `FakeAppServer` aborts all connection handlers. Do not detach tasks or redesign the protocol harness.

- [ ] **Step 3: Prove the harness remains source-compatible**

Run:

```bash
cargo test --locked --bin codex-session-control mcp::tests
```

Expected: every non-ignored MCP unit test passes. Existing `FakeStep` literals compile unchanged, and no timing-based assertion was added.

### Task 2: Lock the Public Contract and Exact-Root Scaffold

**Acceptance criteria:** AC1, AC4, AC5, AC6, AC7

**Files:**
- Modify: `src/mcp/tests/validation.rs`
- Modify: `tests/mcp_contract.rs`
- Modify: `src/mcp/contract.rs`
- Modify: `src/mcp/operations.rs`
- Modify: `src/mcp.rs`
- Modify: `src/mcp/tests/mutation_mapping.rs`

- [ ] **Step 1: Add executable validation and schema RED tests**

Add a runtime RED test that current code can compile:

```rust
#[test]
fn validation_interrupt_accepts_descendant_scope_booleans() {
    for include_descendants in [false, true] {
        let result = validate_input(
            "thread_interrupt",
            arguments(json!({
                "threadId": "target",
                "includeDescendants": include_descendants,
            })),
            &meta("caller"),
        );
        assert!(result.is_ok(), "explicit boolean must be accepted: {result:?}");
    }

    let error = validate_input(
        "thread_interrupt",
        arguments(json!({"threadId": "target", "includeDescendants": "true"})),
        &meta("caller"),
    )
    .unwrap_err();
    assert_eq!(error.tool, "thread_interrupt");
    assert_eq!(error.stage, "input");
    assert_category(error, ToolErrorCategory::InvalidRequest);
}
```

Add `validation_rejects_direct_self_interrupt_with_descendant_opt_in`; current code must execute and fail because it reports input rejection rather than the expected `PolicyRejected`, `tool == "thread_interrupt"`, and `stage == "self_target"`. Add `includeDescendants:null` to the optional-field null table; keep the generic unknown-field test unchanged.

In `tests/mcp_contract.rs`, update only the `thread_interrupt` entries. Expect input properties `includeDescendants` and `threadId`, only `threadId` required, neither field nullable, the exact new tool/property descriptions, and the closed output union below. Extend the interrupt-specific helpers to require:

- two closed root variants keyed by `interrupted.const`, with `turnId` required only for true and optional non-null `descendants` in both;
- closed `warning`, `results`, and `error` descendant variants;
- warning code/count/string-ID array, empty results permitted, exactly-one-of target result/error, and full `ToolErrorData` evidence;
- flattened discovery `code.const == "descendant_discovery_failed"` plus the full error object;
- unchanged catalog order/count, annotations, caller-ID leak check, other tool schemas, and nested-definition no-description rule.

- [ ] **Step 2: Run the contract slice and verify executable RED**

```bash
cargo test --locked --bin codex-session-control mcp::tests::validation::validation_interrupt_accepts_descendant_scope_booleans
cargo test --locked --bin codex-session-control mcp::tests::validation::validation_rejects_direct_self_interrupt_with_descendant_opt_in
cargo test --locked --test mcp_contract public_catalog_is_exact
```

Expected: tests compile, then FAIL on explicit boolean acceptance, opted-in self-target policy attribution, and the missing additive schema/text. Fix fixture errors before production changes.

- [ ] **Step 3: Implement the public contract and trusted carrier**

Add this field to `ThreadInterruptInput`:

```rust
#[serde(default)]
#[schemars(description = "When true, also interrupt active spawned descendants discovered at every depth. Omit or use false for exact-thread scope.")]
pub(super) include_descendants: bool,
```

Change only the validated interrupt variant to:

```rust
ThreadInterrupt {
    input: ThreadInterruptInput,
    caller_thread_id: String,
}
```

In validation, parse input, validate `threadId`, obtain `_meta.threadId` through the existing `caller_thread_id`, call `require_other_thread`, and retain the owned caller ID. Do not change other validated variants or put caller identity in public input.

Represent the old result once and add only the approved containers:

```rust
#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub(super) enum ExactThreadInterruptResult {
    Interrupted { interrupted: bool, turn_id: String },
    NotInterrupted { interrupted: bool },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadInterruptResult {
    #[serde(flatten)]
    pub(super) exact: ExactThreadInterruptResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) descendants: Option<ThreadInterruptDescendants>,
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub(super) enum ThreadInterruptDescendants {
    Warning { warning: ActiveDescendantsWarning },
    Results { results: Vec<DescendantInterruptEntry> },
    Error { error: DescendantDiscoveryError },
}
```

Add `ActiveDescendantsWarning { code: &'static str, active_count: usize, active_thread_ids: Vec<String> }`; an untagged `DescendantInterruptEntry` with exactly `Result { thread_id, result: ExactThreadInterruptResult }` and `Error { thread_id, error: ToolErrorData }`; and `DescendantDiscoveryError { code: &'static str, #[serde(flatten)] error: ToolErrorData }`. Apply camelCase field serialization. Generate the complete reusable error fragment from `raw_schema::<ToolErrorData>()`, lift its `$defs` to the output document root, and reuse/augment that fragment instead of hand-copying error fields. All contract-owned objects remain closed.

Set the tool description to: `Interrupt exactly the target thread's active turn. Set includeDescendants to also interrupt active spawned descendants; exact-thread scope may return a structured warning for active descendants left running. An active goal may start another turn.`

- [ ] **Step 4: Extract the exact helper and keep root-only runtime behavior**

Extract the current body without semantic change:

```rust
pub(super) async fn interrupt_exact_thread(
    client: &AppServerClient,
    connection: &mut AppServerConnection,
    thread_id: &str,
) -> Result<ExactThreadInterruptResult, ToolErrorData>
```

It still runs `compact_snapshot`, returns false without an active turn, builds `MutationContext::for_turn(..., LatestTurnRead)`, invokes `mutation_request` once, and returns the same turn ID. The coordinator now accepts `caller_thread_id: String`, temporarily names it `_caller_thread_id`, calls the exact helper, and returns `ThreadInterruptResult { exact, descendants: None }` so runtime root behavior remains unchanged until the next RED slice.

Pass the trusted caller through the existing `src/mcp.rs` validated match arm. Redirect the four existing exact interrupt tests in `mutation_mapping.rs` to `interrupt_exact_thread` / `ExactThreadInterruptResult` without weakening request-order, serialization, inactive, malformed-snapshot, one-connection, or no-goal assertions.

- [ ] **Step 5: Add the exact default/carrier assertion and verify GREEN**

After the new fields exist, extend validation coverage by matching `ValidatedInput::ThreadInterrupt { input, caller_thread_id }` for omitted input and assert `input.include_descendants == false` plus `caller_thread_id == "caller"`.

Run:

```bash
cargo fmt --all
cargo test --locked --bin codex-session-control mcp::tests::validation
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping
cargo test --locked --test mcp_contract
```

Expected: PASS with unchanged exact-root runtime behavior and a closed additive public schema.

### Task 3: Add Root-First Discovery, Warning, and Discovery-Error Behavior

**Acceptance criteria:** AC1, AC2, AC5, AC7

**Files:**
- Modify: `src/mcp/tests.rs`
- Create: `src/mcp/tests/descendant_interrupt.rs`
- Modify: `src/app_server.rs`
- Modify: `src/mcp/operations.rs`

- [ ] **Step 1: Register the focused module and add compile-ready helpers**

Add `mod descendant_interrupt;` to `src/mcp/tests.rs`. In the new module use `use super::*;`, add a `descendant_page(root, cursor, data, next_cursor)` helper that expects only `ancestorThreadId`, `sourceKinds: ["subAgentThreadSpawn"]`, and optional cursor, and add:

```rust
async fn run_descendant_interrupt(
    harness: &FakeAppServer,
    input: ThreadInterruptInput,
    caller_thread_id: &str,
) -> Result<ThreadInterruptResult, ToolErrorData> {
    let client = AppServerClient::from_config(&harness.config);
    let mut root_connection = client.connect_initialized().await.unwrap();
    interrupt_thread(
        &client,
        &mut root_connection,
        input,
        caller_thread_id.to_owned(),
    )
    .await
}
```

- [ ] **Step 2: Add and run the root/discovery RED tests**

Add these exact tests:

| Test | Required proof |
| --- | --- |
| `root_interrupt_precedes_discovery_and_inactive_root_still_discovers` | Active root mutates before list; inactive root still lists; old root JSON remains flattened. |
| `root_error_stops_before_descendant_discovery` | Root read/latest-turn/native/uncertain errors stay top-level and log no `thread/list`. |
| `discovery_paginates_and_warning_preserves_stable_active_order` | Two pages, idle exclusion, deeper ID retention, duplicate active ID once, exact filters/cursor, ordered warning, one connection. |
| `exact_scope_empty_scan_preserves_root_shape` | Empty/idle-only false scan has no `descendants` and no descendant connection. |
| `opted_in_empty_scan_returns_empty_results` | True plus empty scan returns `results: []`. |
| `discovery_failure_on_first_or_later_page_preserves_root_and_stops_descendants` | Flattened semantic code/full root-attributed error, visible root result, one connection, zero descendant requests. |

Run:

```bash
cargo test --locked --bin codex-session-control descendant_interrupt
```

Expected: tests compile, then FAIL because the current root-only scaffold never sends `thread/list` or returns descendant result data.

- [ ] **Step 3: Add the one private page request**

```rust
pub(crate) async fn spawned_descendants_page(
    &mut self,
    ancestor_thread_id: &str,
    cursor: Option<&str>,
) -> Result<(Vec<Thread>, Option<String>), ToolErrorData> {
    let mut params = serde_json::Map::new();
    params.insert("ancestorThreadId".to_owned(), json!(ancestor_thread_id));
    params.insert("sourceKinds".to_owned(), json!(["subAgentThreadSpawn"]));
    insert_optional(&mut params, "cursor", cursor)?;
    let response: Value = self.request("thread/list", params).await?;
    thread_list_from_native(&response)
}
```

Do not send `archived`, `cwd`, `limit`, or public filters. Do not change the parser, `Thread`, protocol models, or public `threads_list`.

- [ ] **Step 4: Implement full discovery and non-mutating branches**

After the exact root succeeds, exhaust every page before descendant handling. Select only `ThreadStatus::Active` and use `HashSet<String>` plus `Vec<String>` for stable first-seen deduplication. On page error, set `tool = "thread_interrupt"` and root `threadId`, flatten `code = "descendant_discovery_failed"`, attach the error beside the root result, and return without descendant mutation.

For false/omitted scope, return no descendants for an empty target set or the exact ordered `active_descendants_not_interrupted` warning, without opening a descendant connection. For true with an empty target set, return `results: []`. Until Task 4 GREEN, true with a nonempty target set also returns an empty results vector; Task 4's executable RED assertions must expose and replace that minimal incomplete branch before closeout.

- [ ] **Step 5: Verify the discovery slice GREEN**

```bash
cargo fmt --all
cargo test --locked --bin codex-session-control descendant_interrupt
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping
```

Expected: all currently registered descendant tests pass, exact-root regression tests remain unchanged, and no descendant connection exists in this slice.

### Task 4: Add Concurrent Target Handling and Failure Isolation

**Acceptance criteria:** AC2, AC3, AC4, AC6, AC7

**Files:**
- Modify: `src/mcp/tests/descendant_interrupt.rs`
- Modify: `src/mcp/operations.rs`

- [ ] **Step 1: Add and run the opted-in target RED tests**

Add these exact tests:

| Test | Required proof |
| --- | --- |
| `discovery_fully_paginates_before_mutation_and_preserves_stable_active_order` | All pages precede the first target read; active A/deeper B/C each receive at most one exact mutation; result entries are A/B/C in discovery order. Do not assert cross-connection target-read or mutation arrival/completion order. |
| `active_at_discovery_but_inactive_at_refresh_returns_false_without_mutation` | Fresh idle snapshot returns nested false and no target mutation. |
| `caller_descendant_is_rejected_without_connection_while_other_targets_continue` | Ordered self-target error, no caller connection, other targets succeed. |
| `descendant_attempts_overlap_and_reversed_completion_preserves_discovery_order` | Gate A, observe A/B dispatch, acknowledge B response before releasing A, retain A/B output order. |
| `descendant_connection_snapshot_and_native_errors_are_isolated` | Initialize disconnect, snapshot error, native mutation error, and success all return ordered target-attributed entries. |
| `descendant_timeout_is_isolated_and_does_not_block_other_dispatch` | Paused Tokio time plus request notifications prove other dispatch and target-specific timeout. |
| `descendant_outcome_unknown_retains_evidence_and_is_never_retried` | One dispatched mutation, read-only latest-turn reconciliation, full evidence, another success, no replay. |

Use `FakeResponse::Controlled` for release/response acknowledgements and `start_scripted_connections` for initialization failure. Assert exact target request params, at-most-one mutation per target, and no goal operation.

Run:

```bash
cargo test --locked --bin codex-session-control descendant_interrupt
```

Expected: tests compile; the new tests FAIL because nonempty opted-in discovery still returns no entries or connections. Previously GREEN Task 3 tests remain passing.

- [ ] **Step 2: Add narrow error attribution and ordered concurrent operations**

```rust
fn attribute_interrupt_error(mut error: ToolErrorData, thread_id: &str) -> ToolErrorData {
    error.tool = "thread_interrupt".to_owned();
    error.thread_id = Some(thread_id.to_owned());
    error
}
```

Apply it to discovery and non-caller target connection/snapshot/exact-helper errors while preserving category, stage, native, turn ID, dispatch, observation, and reconciliation fields. Keep the existing `require_other_thread` error unchanged for the caller entry.

For true/nonempty scope, map ordered target IDs to futures and await `futures_util::future::join_all`:

- caller target: return the existing self-target policy error without connecting;
- every other target: initialize an independent connection, call `interrupt_exact_thread` for a fresh snapshot and exact mutation, and convert success/error into one entry rather than propagating;
- return `join_all` output directly so completion order cannot reorder discovery order.

Do not add detached tasks, a shared connection, concurrency limit, retry, catch-up pass, quiescence loop, service, or manager.

- [ ] **Step 3: Run the full focused GREEN set**

```bash
cargo fmt --all
cargo test --locked --bin codex-session-control descendant_interrupt
cargo test --locked --bin codex-session-control mcp::tests::validation
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping
cargo test --locked --test mcp_contract
```

Expected: PASS. If the overlap test can pass under sequential target handling, repair its synchronization before proceeding.

### Task 5: Update Documentation and Establish Pre-Review Readiness

**Acceptance criteria:** AC7, AC8

**Files:**
- Modify: `README.md`
- Verify: all files listed under `## File Structure`

- [ ] **Step 1: Replace only the `thread_interrupt` README row**

```markdown
| thread_interrupt | Interrupt a session's active response, optionally including active spawned descendants; exact-thread scope may return a structured warning for descendants left running. |
```

Do not change unrelated README content.

- [ ] **Step 2: Run fresh focused verification**

```bash
cargo test --locked --bin codex-session-control descendant_interrupt
cargo test --locked --bin codex-session-control mcp::tests::validation
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping
cargo test --locked --test mcp_contract
```

Expected: every command exits 0.

- [ ] **Step 3: Audit scope and run the approved pre-review repository gate**

```bash
git diff --check
{ git diff --name-only; git ls-files --others --exclude-standard; } | sort -u
git status --short --branch
./scripts/check.sh
```

Expected: exactly the eleven implementation paths are changed and the repository gate exits 0. Stop on any extra path. Any repair repeats affected focused checks and this gate.

### Task 6: Final Whole-Implementation Review

**Covers:** Tasks 1-5
**Review contract:** `## Review Milestones` → `Final review`

- [ ] Dispatch one dedicated spec-compliance reviewer over the approved spec, brainstorming source, and entire implementation diff. Fix and repeat until it passes.
- [ ] Only after spec compliance passes, dispatch one dedicated code-quality reviewer. Require explicit review of failure isolation, at-most-once mutation, deterministic concurrency proof, full error-schema evidence, harness cleanup, and scope containment. Fix Critical/Important findings and repeat until it passes.
- [ ] Fix valid Minor findings. A non-code/non-test correction reruns its affected validation but does not restart implementation review.
- [ ] Every code/test change, regardless of finding severity, returns to affected focused checks, Task 5's `./scripts/check.sh`, then restarts spec-compliance and code-quality review in order.
- [ ] Stop for the operator if a finding requires changed behavior, dependency, public API, architecture, transport, or an out-of-scope file.
- [ ] Complete this task only when both reviews pass and no material finding remains.

### Task 7: Run the Final Post-Review Gate and Create One Scoped Commit

**Acceptance criteria:** AC8

**Files:**
- Stage: only the eleven paths listed under `## File Structure`

- [ ] **Step 1: Run the handoff-required final gate**

```bash
./scripts/check.sh
```

Expected: exit 0 on the reviewed implementation. If a repair is needed, rerun affected focused checks, Task 5's gate, both Task 6 reviews, and this final gate; earlier evidence becomes stale.

- [ ] **Step 2: Recheck and stage the exact allowlist**

```bash
{ git diff --name-only; git ls-files --others --exclude-standard; } | sort -u
git status --short --branch
git add README.md src/app_server.rs src/mcp.rs src/mcp/contract.rs src/mcp/operations.rs src/mcp/tests.rs src/mcp/tests/descendant_interrupt.rs src/mcp/tests/mutation_mapping.rs src/mcp/tests/support.rs src/mcp/tests/validation.rs tests/mcp_contract.rs
git diff --cached --check
git diff --cached --name-only
git diff --cached --stat
```

Expected: exactly the eleven allowlisted paths, no whitespace error, and no workflow/Cargo/unrelated path staged.

- [ ] **Step 3: Commit locally and stop**

```bash
git commit -m "fix(mcp): interrupt active spawned descendants"
git show --stat --oneline --decorate HEAD
git status --short --branch
```

Expected: one scoped commit and a clean worktree. Do not bypass hooks, push, open/update a PR, tag, release, restart services, or mutate live sessions.

## Verification

Per-slice RED commands are defined in Tasks 2-4. Complete focused GREEN evidence is:

```bash
cargo test --locked --bin codex-session-control descendant_interrupt
cargo test --locked --bin codex-session-control mcp::tests::validation
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping
cargo test --locked --test mcp_contract
```

Closeout order is mandatory:

1. focused GREEN evidence;
2. pre-review `./scripts/check.sh`;
3. spec-compliance review;
4. code-quality review;
5. final post-review `./scripts/check.sh`;
6. staged-diff validation and one local Conventional Commit.

No live/manual Codex mutation is part of acceptance. No completion claim may rely on pre-fix or earlier-session output.
