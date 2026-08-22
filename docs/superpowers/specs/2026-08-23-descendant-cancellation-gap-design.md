# Descendant Cancellation Gap Design

**Status:** approved
**Source:** `docs/superpowers/brainstorming/2026-08-22-descendant-cancellation-gap.md`
**Next:** `docs/handoffs/2026-08-23-014548-descendant-cancellation-gap-writing-plans.md`

## Objective

Extend `thread_interrupt` without changing its existing root semantics:

- omitted or false `includeDescendants` keeps exact-thread mutation scope and reports any active spawned descendants left running;
- true `includeDescendants` interrupts eligible active spawned descendants at every depth and returns one ordered outcome per discovered target;
- root, discovery, and descendant failures remain truthful about partial application and never cause a blind mutation retry.

This is a localized missing-feature fix. It is not a general cancellation framework or a public lineage API.

## Prerequisites

- Work on branch `fix/descendant-cancellation-gap`, based on `main` commit `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`.
- Treat the approved brainstorming artifact and its passed review trace at `docs/superpowers/reviews/2026-08-22-descendant-cancellation-gap-review.md` as the behavior source of truth.
- Preserve compatibility with supported Codex App Server `0.147.0`.
- Begin implementation only after this spec and a subsequent implementation plan are reviewed and approved.
- Use the existing fake app-server tests for automated protocol and concurrency proof; no live session mutation is required for this feature.

## Scope

- Add optional `includeDescendants: boolean` to the `thread_interrupt` input, defaulting to `false` when omitted.
- Keep the existing exact root snapshot and `turn/interrupt` operation first.
- Privately discover active spawned descendants through fully paginated native `thread/list` calls.
- Return a structured warning for active descendants outside the requested mutation scope.
- When opted in, handle eligible descendants concurrently through independent initialized connections and return ordered per-target results or errors.
- Preserve caller self-target protection, root error behavior, goal state, mutation reconciliation, and current output fields.
- Update the public tool description and concise README tool summary.

## Non-Goals

- No change to `threads_list`, `thread_read`, goal tools, or unrelated MCP tools.
- No public `parentThreadId`, `ancestorThreadId`, or general lineage surface.
- No enum or scope abstraction replacing `includeDescendants`, and no second cancellation tool.
- No recursive direct-child traversal, Desktop cache, catch-up pass, quiescence loop, mutation retry, confirmation mode, or post-response status proof.
- No goal pause, clear, or other goal-state coupling.
- No transport multiplexing, connection abstraction rewrite, controller-specific concurrency limit, generalized cancellation helper, or opportunistic refactor.
- No claim that an opted-in call contains descendants spawned or activated after its completed discovery snapshot.

## Public Contract

### Input

`thread_interrupt` accepts:

```json
{
  "threadId": "root-thread",
  "includeDescendants": true
}
```

- `threadId` remains required and must identify a thread other than the MCP caller.
- `includeDescendants` is optional, boolean, and defaults to `false`.
- Unknown fields remain rejected.

The tool description must state that the root is interrupted exactly, that `includeDescendants` opts into spawned-descendant interruption, and that exact-thread scope can return a structured active-descendant warning.

### Root result

The existing root fields and meanings remain unchanged:

- successful exact interruption: `{ "interrupted": true, "turnId": "..." }`;
- no active exact turn: `{ "interrupted": false }`.

The additive `descendants` member is optional. It must not weaken the conditional root schema: `turnId` remains required only when `interrupted` is true. Contract-owned objects reject additional properties.

### Exact-thread warning

When descendant discovery succeeds, `includeDescendants` is false, and at least one discovered spawned descendant is active, return:

```json
{
  "interrupted": true,
  "turnId": "root-turn",
  "descendants": {
    "warning": {
      "code": "active_descendants_not_interrupted",
      "activeCount": 2,
      "activeThreadIds": ["child-a", "grandchild-b"]
    }
  }
}
```

`activeThreadIds` follows authoritative discovery order and `activeCount` equals its length. This is structured result data only; do not add a second human-readable warning. If no active descendant exists, omit `descendants` so omitted and false calls retain the old root-only output.

### Opted-in results

When `includeDescendants` is true and discovery succeeds, always return `descendants.results`, including an empty array. Each target active in the discovery snapshot appears exactly once in discovery order:

```json
{
  "interrupted": false,
  "descendants": {
    "results": [
      {
        "threadId": "child-a",
        "result": { "interrupted": true, "turnId": "child-turn" }
      },
      {
        "threadId": "child-b",
        "result": { "interrupted": false }
      },
      {
        "threadId": "caller-thread",
        "error": {
          "category": "policy_rejected",
          "message": "request rejected by session-control policy",
          "tool": "thread_interrupt",
          "stage": "self_target"
        }
      }
    ]
  }
}
```

Every entry has `threadId` and exactly one of:

- `result`, reusing the exact `{ interrupted, turnId? }` result shape; or
- `error`, reusing the serialized `ToolErrorData` shape.

The caller entry uses the existing self-target policy error and does not initialize a connection. Connection and snapshot errors for other descendants must be attributed to `tool: "thread_interrupt"` and the exact descendant `threadId`; mutation errors retain the existing exact thread/turn, dispatch, native, observation, and reconciliation evidence.

### Discovery error

If any discovery page fails, return the already-completed root result plus `descendants.error` and perform no descendant mutation:

```json
{
  "interrupted": true,
  "turnId": "root-turn",
  "descendants": {
    "error": {
      "code": "descendant_discovery_failed",
      "category": "authority_transport_failure",
      "message": "app-server transport failed",
      "tool": "thread_interrupt",
      "stage": "thread/list",
      "threadId": "root-thread"
    }
  }
}
```

The semantic `code` and existing `ToolErrorData` fields are flattened into one error object. Optional native, dispatch, observation, and reconciliation fields remain present whenever supplied by the existing error contract. This descendant operation error is result data rather than a top-level MCP error because the root mutation may already have been applied.

## Execution Design

1. Validate the input before opening the root connection. Preserve the existing direct self-target rejection for `threadId`, and carry the validated MCP `_meta.threadId` with the interrupt request into execution so implicit caller descendants can be recognized later.
2. Run the existing fresh exact root snapshot and exact `turn/interrupt` path first. If it returns any `ToolErrorData`, including `outcome_unknown`, return that top-level error unchanged and do not discover or mutate descendants. If it returns `interrupted: false`, continue.
3. Reuse the root connection to exhaust a private page-by-page `thread/list` query with `ancestorThreadId` equal to the root and `sourceKinds: ["subAgentThreadSpawn"]`. Follow `nextCursor` until null before issuing any descendant mutation. Do not expose these filters through public `threads_list`.
4. Select only threads whose authoritative listed status is `active`. Preserve native discovery order and stable first occurrence of each thread ID.
5. If discovery fails, attribute the error to `thread_interrupt` and the root thread, wrap it with `code: "descendant_discovery_failed"`, attach it to the root result, and stop descendant handling.
6. If `includeDescendants` is false, return the warning for a nonempty target set or the unchanged root result for an empty set. Do not open descendant connections.
7. If `includeDescendants` is true, build one asynchronous operation per target and poll all operations concurrently without adding a concurrency limit:
   - for the caller ID, return the existing `policy_rejected`/`self_target` error immediately with no connection;
   - for every other target, initialize an independent app-server connection, take a fresh compact snapshot, and invoke the existing exact interrupt helper only if that snapshot still has an active turn;
   - map inactive-at-refresh targets to `{ "interrupted": false }`;
   - collect all outcomes instead of failing fast.
8. Return results in discovery order, not completion order. Reuse the existing `mutation_request` and `LatestTurnRead` reconciliation path so `outcome_unknown` remains at-most-once and is never replayed.

Using an order-preserving future join over independent connections satisfies concurrency and deterministic output without spawning detached tasks or changing `AppServerConnection`.

## Internal Design

### Contract and caller identity

Keep public input separate from trusted request context. `ThreadInterruptInput` owns only public fields. The validated interrupt variant carries both that input and the caller thread ID obtained from `_meta`; `src/mcp.rs` passes both to the operation. Do not add caller identity to the public schema or recover it from global state.

Represent the old exact result shape once and reuse it for the flattened root and nested descendant results. Add narrow serializable types for the warning, ordered result/error entries, and flattened discovery error. The manually supplied `thread_interrupt` output schema must describe all root-plus-descendant variants and the full reusable `ToolErrorData` fields without relaxing `additionalProperties` behavior.

### Private descendant discovery

Add one private app-server page method dedicated to spawned-descendant discovery. It accepts the ancestor thread ID and optional cursor, sends only the native lineage/source filter plus cursor when present, and delegates response parsing to the existing `thread_list_from_native` mapping. Pagination orchestration belongs in the interrupt operation so root result and error attribution remain visible there.

Do not change `Thread`, `thread_from_native`, or `ToolErrorData`; existing thread status parsing and error fields are sufficient.

### Exact interruption reuse

Extract or retain a private exact-thread operation that accepts a thread ID, uses `compact_snapshot`, and performs the current exact `turn/interrupt` mutation and reconciliation. Use it unchanged for the root and each eligible descendant. The subtree coordinator owns only discovery, caller-policy branching, concurrency, ordering, and result composition.

## Gap Resolutions

- The approved artifact defines active descendants as a target set, requires each target to be accounted for once, and preserves at-most-once mutation behavior. Therefore, if paginated native results overlap, keep the first occurrence of each active thread ID and do not issue duplicate mutations. This does not add another discovery pass or change native order.

## Expected File Changes

| File | Required change |
| --- | --- |
| `src/mcp/contract.rs` | Add the optional input flag, trusted caller-context plumbing, result types, tool text, and exact output schema. |
| `src/mcp.rs` | Pass the validated caller ID into interrupt execution. |
| `src/mcp/operations.rs` | Preserve the exact helper and add root-first discovery, warning/error composition, concurrent per-target handling, ordering, and error attribution. |
| `src/app_server.rs` | Add the private spawned-descendant `thread/list` page request. |
| `src/mcp/tests.rs` | Register a focused descendant-interrupt test module. |
| `src/mcp/tests/descendant_interrupt.rs` | Add focused protocol, pagination, warning, concurrency, race, caller-policy, and failure tests. |
| `src/mcp/tests/support.rs` | Extend the fake app-server only as needed to accept and script simultaneous initialized connections deterministically. |
| `src/mcp/tests/validation.rs` | Cover omitted/false/true input, unknown-field rejection, and unchanged direct self-target rejection. |
| `src/mcp/tests/mutation_mapping.rs` | Keep existing exact-root tests compiling and prove their unchanged result/mutation semantics. |
| `tests/mcp_contract.rs` | Assert the additive input and complete warning/results/error output schema plus updated model-facing descriptions. |
| `README.md` | Update the one-line `thread_interrupt` summary to mention optional spawned-descendant interruption and structured warning behavior. |

No changes are expected in `src/error.rs`, `src/model.rs`, or `src/app_server/protocol.rs`.

## Test Requirements

Write failing focused tests before production changes, then make them pass. The automated suite must prove:

1. Contract and validation: omission defaults to false; explicit false and true are accepted; unknown fields and direct caller targeting remain rejected; schemas enforce all result branches and descriptions.
2. Root ordering: exact root snapshot/interruption happens before discovery; root inactivity still proceeds; every actual root error stops before `thread/list`.
3. Discovery mapping: every page sends the root `ancestorThreadId`, only `subAgentThreadSpawn`, and the prior `nextCursor`; all pages complete before any descendant mutation; child and deeper IDs returned by native are retained in order.
4. Target selection: idle listed descendants are excluded; active IDs are stably deduplicated; false/omitted scope returns the exact warning; an empty false scan preserves the old output; an empty true scan returns `results: []`.
5. Race handling: a target active during discovery but inactive at its fresh snapshot returns `interrupted: false` with no mutation.
6. Caller safety: an active caller descendant receives the ordered self-target error without a descendant connection, while other targets continue.
7. Concurrency and ordering: independent descendant connections overlap in flight; a delayed or failed target does not delay dispatch to another; reversed completion still returns discovery order.
8. Failure isolation: connection, snapshot, native mutation, timeout, and `outcome_unknown` errors remain target-specific and do not stop other targets. Uncertain outcomes retain dispatch and read-only reconciliation evidence and are never retried.
9. Discovery failure: failure on the first or later page returns flattened `descendants.error`, keeps the root result visible, and issues zero descendant mutations.
10. Regression: existing exact interrupt requests keep the same root fields, native request order, goal independence, and top-level root-error behavior.

Use deterministic fake-server synchronization for concurrency tests; do not treat elapsed wall-clock timing alone as proof.

## Verification

Run focused checks during implementation:

```bash
cargo test --locked --bin codex-session-control descendant_interrupt
cargo test --locked --bin codex-session-control mcp::tests::validation
cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping
cargo test --locked --test mcp_contract
```

After implementation and accepted fixes, run the repository gate:

```bash
./scripts/check.sh
```

Then perform review in this order:

1. spec compliance against this document and the approved brainstorming source;
2. code quality, including failure isolation, at-most-once mutation safety, deterministic concurrency tests, and absence of scope expansion.

Do not claim completion from earlier or pre-change test output.

## Acceptance Criteria

- Existing omitted-field calls are input-compatible, retain exact root fields and native interrupt semantics, never mutate descendants, and expose every active spawned descendant in the structured warning.
- Opted-in calls fully paginate one authoritative all-depth spawned-descendant query before mutation and account exactly once for every active target in stable discovery order.
- Every eligible non-caller target uses an independent initialized connection and fresh exact snapshot; attempts overlap, failures are isolated, and completion order cannot reorder output.
- The implicit caller descendant is never connected to or mutated and receives the existing structured self-target policy error while other targets continue.
- Root errors remain top-level and stop all descendant work; discovery errors remain nested beside the visible root result and cause zero descendant mutations.
- Descendant no-op, success, ordinary error, timeout, and uncertain-outcome entries match the approved shapes and retain all applicable existing error evidence without mutation retry.
- Goal state, public thread listing, transport architecture, unrelated tools, and post-snapshot descendants remain outside the change.
- Focused contract, protocol, pagination, concurrency, race, and failure tests pass, followed by a fresh successful `./scripts/check.sh` and the required two-stage review.

## Approval and Handoff Boundary

- While this document is `in-review`, review dispatch and any implementation planning require operator approval.
- **[MANUAL]** The operator approves the reviewed spec before its status changes to `approved`.
- After approval, create a fresh `writing-plans` handoff. The approved spec is the source of truth; settled behavior must not be reopened without contradictory repository evidence.
- Commit only the approved spec, any material spec review trace, and the approved handoff at that boundary. Do not begin planning or implementation in the spec-writing session.
