# Brainstorming: Descendant Cancellation Gap

**Status:** approved
**Source:** Codex task `01a02698-2bab-7013-acf0-346a837ff8f5`
**Review:** `docs/superpowers/reviews/2026-08-22-descendant-cancellation-gap-review.md`
**Next:** `docs/handoffs/2026-08-23-005055-descendant-cancellation-gap-writing-specs.md`

## Context

Codex Session Control can interrupt an exact active turn, but stopping an orchestrator does not stop the subagents it spawned. The observed incident involved parent thread `01a02a76-82a3-71a3-96e5-daca2781233d` and child thread `01a02a8e-40e9-7432-bcd7-0ff66d86f74f`: the parent became idle after a successful interrupt while the child continued editing the shared workspace for more than seven minutes.

The repository is on branch `fix/descendant-cancellation-gap`, created from clean `main` commit `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`. The supported and installed Codex version is `0.147.0`.

The current `thread_interrupt` implementation reads one fresh active turn ID and sends one native `turn/interrupt` request for that exact thread and turn. Its test contract explicitly requires exact-turn targeting. This behavior is correct for the existing narrow operation but does not contain delegated work.

The public `threads_list` path forwards no `sourceKinds`, so native Codex defaults it to interactive sources and omits subagents. Codex Session Control already uses an all-source list internally when checking active sessions before updates, including every `subAgent*` source kind.

The Codex `0.147.0` experimental app-server schema provides the lineage needed for a reliable solution:

- Returned subagent threads carry `parentThreadId`.
- `thread/list` accepts `parentThreadId` for direct children.
- `thread/list` accepts `ancestorThreadId` for spawned descendants at any depth, excluding the ancestor itself.
- `sourceKinds` can explicitly include subagent sources instead of relying on the interactive-only default.

The [official Codex App Server documentation](https://learn.chatgpt.com/docs/app-server#api-overview) confirms that `turn/interrupt` requests cancellation of one in-flight turn and that `thread/list` supports the experimental parent and ancestor filters. Codex Session Control currently exposes `forkedFromId` but not `parentThreadId`, and it does not expose either lineage filter.

The incident child returned `goal: null` from a live `thread_goal_get`, consistent with spawned subagents not receiving persistent goals automatically. This is not a protocol guarantee that subagents can never have goals: the supported Codex schema and official goal APIs address threads by `threadId` without excluding subagent source kinds. The cancellation design therefore does not need automatic goal handling, but it must not claim that a descendant goal is impossible.

The existing success result follows the native Codex `0.147.0` response contract. For a normal active turn, app-server records the interrupt request, submits `Op::Interrupt`, and withholds the empty response until it handles the turn's terminal event. It responds to pending interrupt requests on `TurnAborted`, but also on a normal `TurnComplete` race. Codex Session Control then returns `interrupted: true` without a separate post-response read. The result is therefore stronger than mutation dispatch acknowledgement but is not an independent assertion that the final status was specifically `interrupted`. The existing field and response behavior remain unchanged. A connection loss after possible dispatch already returns `outcome_unknown` with a read-only observation instead of claiming success.

The current installed Desktop bundle, based on upstream package `26.810.52044`, has explicit descendant cleanup for the user Stop action. It interrupts the root first and then performs descendant cleanup in the background. Cleanup immediately interrupts active descendants already known in the Desktop cache, then obtains an authoritative spawned-descendant snapshot and interrupts remaining active descendants. On app-server versions including `0.147.0`, discovery uses paginated `thread/list` calls with `ancestorThreadId` and `sourceKinds: ["subAgentThreadSpawn"]`; the ancestor filter includes children, grandchildren, and every deeper spawned descendant. The routine does not loop until quiescence. Its initial cached pass is a Desktop latency optimization, not a native tree-interrupt operation. Codex Session Control has no equivalent UI cache and can use the authoritative ancestor query directly.

The Codex CLI TUI `0.147.0` Stop path sends one exact `turn/interrupt` for the selected thread. Its spawned-descendant tree walk is used to populate agent navigation, not to cascade interruption. Desktop, not the CLI TUI, is the useful native behavior to mirror for this issue.

The design must preserve the product's fail-closed, at-most-once mutation behavior. A dropped connection after an interrupt may produce `outcome_unknown`; the controller must not blindly replay mutations. Brainstorming is design-only: no production code, tests, specification, or implementation plan may be written before this artifact is reviewed, approved, and committed.

The implementation scope is surgical: extend `thread_interrupt` only as needed to discover and interrupt spawned descendants and to report active descendants left outside the requested scope. It must not rename existing result fields, alter goal behavior, expand the public thread-list contract, add confirmation modes, or include unrelated refactoring or cleanup.

## Current Decisions

- Preserve exact-thread behavior by default, add optional `includeDescendants`, and surface omitted-scope risk as structured data only. ([Q1](#q1-what-should-thread_interrupt-mean-when-the-target-has-spawned-descendants), [Q2](#q2-how-should-the-active-descendant-warning-be-represented), [Q3](#q3-what-input-shape-should-select-descendant-interruption))
- Interrupt active turns only; preserve goal state, the existing root result fields, and the native interrupt-response semantics. ([Q4](#q4-should-descendant-interruption-change-persistent-goal-state), [Q5](#q5-should-success-wait-for-terminal-interruption-or-change-the-existing-result-field))
- Adapt Desktop's single authoritative, paginated, all-depth spawned-descendant discovery and handle active descendants concurrently through independent initialized connections. ([Q6](#q6-which-native-descendant-interruption-strategy-should-thread_interrupt-follow), [Q11](#q11-should-discovered-descendants-be-interrupted-concurrently-or-sequentially))
- Isolate per-descendant failures and preserve at-most-once `outcome_unknown` handling; a discovery failure is a structured descendant error, an actual root error retains the existing top-level failure, and an implicit caller descendant retains the existing self-target policy as a per-target error. ([Q7](#q7-what-should-happen-when-one-descendant-interruption-fails), [Q9](#q9-how-should-an-authoritative-descendant-discovery-failure-be-shaped), [Q10](#q10-what-should-includedescendants-do-when-the-root-interruption-returns-an-error), [Q13](#q13-what-should-happen-when-the-caller-is-an-active-descendant-of-the-requested-root))
- Keep all additive subtree information under `descendants`; an opted-in successful scan always returns `results`, including an empty array. ([Q8](#q8-how-should-descendant-information-be-added-to-the-existing-result), [Q12](#q12-what-should-includedescendants-return-when-no-active-descendants-exist))

## Agreed Behavior

1. Handle the requested root thread first through the existing fresh-snapshot and exact-turn interruption path. An actual root error returns unchanged and stops before descendant discovery or mutation. A root with no active turn returns `interrupted: false` and continues into descendant handling.
2. Exhaust the authoritative native descendant query before issuing any descendant mutation. Use paginated `thread/list` calls with `ancestorThreadId` set to the requested root and `sourceKinds: ["subAgentThreadSpawn"]`; this selects spawned descendants at every depth and excludes the root.
3. Treat descendants reported active by the authoritative list as the target set. When `includeDescendants` is omitted or `false`, do not mutate that set; return its count and thread IDs in `descendants.warning`. Omit `descendants` when that set is empty.
4. When `includeDescendants` is `true`, account for every target. If a target is the caller identified by MCP `_meta.threadId`, do not connect to or mutate it; return the existing `policy_rejected` self-target error in that target's result entry. Handle every other target concurrently through its own initialized connection and obtain a fresh exact active-turn snapshot immediately before interruption. A target that became inactive returns `interrupted: false`; an exact interruption returns the existing success shape; and a connection, snapshot, or mutation failure returns the existing structured error under that target's result entry.
5. Collect every per-target outcome and return entries in authoritative discovery order, independent of concurrent completion order. A protected caller does not prevent any other target from being handled. Never retry an `outcome_unknown` mutation. Preserve its dispatch and read-only reconciliation evidence for the exact affected descendant.
6. If any page of authoritative discovery fails, return `descendants.error` and perform no descendant mutations. The root result remains visible because it may already have been applied.
7. Use one authoritative snapshot only. Do not add Desktop cache state, recursive traversal, catch-up passes, or a loop until quiescence. A descendant that becomes active or is spawned after the snapshot is outside this operation, matching native Desktop's finite cleanup behavior.

## Result Examples

An omitted or false `includeDescendants` with active descendants preserves the root result and reports the work deliberately left running:

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

An opted-in call returns an outcome for each descendant that was active at discovery. Root inactivity does not prevent descendant handling:

```json
{
  "interrupted": false,
  "descendants": {
    "results": [
      {
        "threadId": "child-a",
        "result": {
          "interrupted": true,
          "turnId": "child-a-turn"
        }
      },
      {
        "threadId": "grandchild-b",
        "result": {
          "interrupted": false
        }
      },
      {
        "threadId": "caller-thread",
        "error": {
          "category": "policy_rejected",
          "message": "request rejected by session-control policy",
          "tool": "thread_interrupt",
          "stage": "self_target"
        }
      },
      {
        "threadId": "child-c",
        "error": {
          "category": "outcome_unknown",
          "message": "Mutation outcome is unknown. The request may already have been applied.",
          "tool": "thread_interrupt",
          "stage": "turn/interrupt",
          "threadId": "child-c",
          "turnId": "child-c-turn",
          "dispatch": "may_have_been_dispatched"
        }
      }
    ]
  }
}
```

An authoritative discovery failure is an operation-level descendant error with a stable semantic code and the existing error fields flattened beside it:

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

A successful opted-in scan with no active descendants reports that the requested scan completed:

```json
{
  "interrupted": true,
  "turnId": "root-turn",
  "descendants": {
    "results": []
  }
}
```

When `includeDescendants` is omitted or false and no active descendants exist, the output remains exactly the existing root-only result with no `descendants` member. Optional native, dispatch, observation, and reconciliation fields remain present on descendant errors whenever the existing `ToolErrorData` would include them.

## Success Criteria

- Existing `thread_interrupt` requests remain schema-compatible and retain their root-only fields and semantics. Omitted or false `includeDescendants` never interrupts a descendant.
- A root-only request cannot silently imply full containment: every active spawned descendant from the authoritative all-depth snapshot appears in the structured warning.
- An opted-in request fully paginates native spawned-descendant discovery before mutation, reaches children and deeper descendants, and accounts for every target that was active in that snapshot.
- The implicit caller target returns the existing structured `policy_rejected` self-target error without a connection or mutation attempt. Every other descendant attempt runs independently and concurrently; one protected, timed-out, or failed target does not prevent other discovered active descendants from being handled, and response order does not depend on completion timing.
- Root errors, discovery errors, descendant no-ops, exact successes, ordinary failures, and uncertain outcomes follow the agreed shapes without unsafe mutation retries.
- Goal state, public `threads_list`, existing connection transport semantics, and unrelated tools remain unchanged.
- Contract, protocol-mapping, pagination, concurrency, race, and failure-path tests cover the new behavior, all existing tests remain green, and the repository's canonical `./scripts/check.sh` gate passes after implementation.

## Q&A

### Q1: What should `thread_interrupt` mean when the target has spawned descendants?

**Recommended**

The initial recommendation was to interrupt the root and its entire spawned subtree by default, with exact-thread interruption available through an explicit scope. That contract most directly matched the operator's intent to stop an orchestrated task and made the safe behavior hard to omit. The alternatives were to preserve exact-thread interruption and require an opt-in descendant scope, which retained compatibility but risked repeating the incident, or to add a separate tree-interruption tool, which made the distinction explicit but expanded and fragmented the tool surface.

**Resolved**

Preserve exact-thread interruption as the behavior when the new parameter is omitted. Add a parameter that explicitly requests interruption of spawned descendants. When the parameter is omitted and active descendants exist, return a warning to the caller instead of silently reporting an apparently complete stop.

**Rationale**

This hybrid keeps existing callers and the current exact-turn contract stable while directly addressing the dangerous part of the incident: the caller believed the orchestrated task had stopped even though a child was still mutating the workspace. A warning makes the remaining work visible without silently broadening a destructive operation, and the opt-in parameter avoids adding an overlapping MCP tool. Q2 through Q13 resolve the structured representation, discovery timing, mutation order, race handling, caller safety, and partial or uncertain outcomes.

### Q2: How should the active-descendant warning be represented?

**Recommended**

Return a successful root-interruption result with a stable structured warning code, the number of active descendants that were not interrupted, and their thread IDs. The requested root mutation succeeded, so an MCP error would misrepresent the applied state and could encourage an unsafe retry. Prose alone would not give an agent or programmatic caller a reliable branch condition.

**Resolved**

Use structured result fields only. Return the warning code, active-descendant count, and descendant thread IDs in the authoritative structured result, with no separate human-readable warning text.

**Rationale**

The MCP surface is designed for agents rather than humans. A separate prose mirror would duplicate the authoritative data without improving machine actionability. Codex Session Control's existing MCP response transport may continue serializing the same JSON into its content block for client compatibility; that serialization is not an additional human-oriented warning contract.

**Approaches Considered**

1. **Structured success warning** - Preserve the successful root result and add stable machine-readable warning fields. Selected because it truthfully represents both the applied mutation and the active descendants that remain.
2. **Prose-only warning** - Prefix the successful result with human-readable warning text. Rejected because agents cannot depend on prose as a stable control contract and it duplicates the structured data.
3. **MCP error** - Fail the call when descendants remain. Rejected because the root interruption already occurred, so an error would conflate a successful mutation with incomplete broader containment and make retries dangerous.

### Q3: What input shape should select descendant interruption?

**Recommended**

Add `includeDescendants?: boolean` and treat omission as `false`. This is the smallest additive contract, directly describes the requested expansion of the target set, and preserves every existing call. A `scope: "thread" | "tree"` enum would be more extensible but adds an abstraction without a second real scope today. A `descendantMode` enum would encode the default warning behavior into the input even though warning is an output consequence, not a requested mutation mode.

**Resolved**

Use the optional boolean `includeDescendants`. Omitted or `false` preserves exact-thread interruption; `true` requests interruption of the target thread and its spawned descendants.

**Rationale**

The boolean is explicit at the call site, additive to the existing schema, and avoids both a new tool and a speculative enum. Its positive name makes the broader destructive scope visible without changing the meaning of existing requests.

**Approaches Considered**

1. **Optional `includeDescendants` boolean** - Selected as the narrowest backward-compatible input extension.
2. **`scope: "thread" | "tree"` enum** - Rejected because the product currently has only one optional expansion and does not need an extensibility abstraction.
3. **`descendantMode: "warn" | "interrupt"` enum** - Rejected because warning is the result of omitting broader interruption, not an independent action requested by the caller.

### Q4: Should descendant interruption change persistent goal state?

**Recommended**

Interrupt active turns only and never pause or clear goals implicitly. Codex Session Control already exposes explicit goal tools, and combining goal-state mutation with interruption would create surprising side effects and multi-mutation partial-failure semantics. Spawned subagents normally have no persistent goal, so automatic descendant goal handling does not address the observed incident.

**Resolved**

Confirmed that `includeDescendants` affects active turn interruption only. It does not pause, clear, or otherwise change the target thread's or any descendant thread's persistent goal state.

**Rationale**

The incident descendant had no goal, and normal subagent spawning does not automatically create one. Keeping interruption and persistent goal control separate preserves the existing single-purpose tool boundaries. Although the generic Codex goal protocol does not formally prohibit a goal on a subagent thread, that edge case does not justify coupling goal mutations into this fix; callers that use goals remain responsible for explicit goal control.

### Q5: Should success wait for terminal interruption or change the existing result field?

**Recommended**

Preserve the existing native response behavior. The initial recommendation incorrectly described the response as immediate request acceptance and proposed renaming `interrupted` to reflect that interpretation. Exact Codex `0.147.0` source shows that a normal interrupt response is withheld until app-server handles the turn's terminal event, although a normal-completion race also releases it. Renaming the field was both evidenceually wrong and unnecessary scope expansion.

**Resolved**

Keep the existing `interrupted` result field exactly as it is. A successful call continues to return after the correlated native interrupt response, with no additional Codex Session Control read or confirmation step. Do not add a confirmation parameter or another success mode.

**Rationale**

The descendants bug does not require changing the existing root-interruption result contract. The native app-server already controls when its response is released; adding another observation phase or renaming the field would change an unrelated established contract without improving descendant containment. The surgical fix preserves the public contract and limits new behavior to descendant handling.

**Approaches Considered**

1. **Preserve `interrupted` and the current native response behavior** - Selected because it changes no unrelated contract and keeps existing callers stable.
2. **Rename the field to describe request acceptance** - Rejected because the premise was inaccurate and the breaking change was unrelated to descendant containment.
3. **Add a post-response terminal-status read** - Rejected because it adds a second confirmation contract unrelated to the descendants fix.
4. **Add an optional confirmation mode** - Rejected because existing read and wait tools already provide that composition.

### Q6: Which native descendant-interruption strategy should `thread_interrupt` follow?

**Recommended**

Adapt the authoritative half of Desktop's user-Stop behavior. After handling the root with the existing exact-turn operation, exhaust a paginated native `thread/list` query using `ancestorThreadId` and `sourceKinds: ["subAgentThreadSpawn"]`. The ancestor filter returns spawned descendants at every depth. For each active descendant eligible under the existing caller policy, obtain a fresh exact active turn snapshot and invoke the existing exact-turn interrupt path. Do not recreate Desktop's pre-existing UI cache, add a recursive parent walk, or loop until quiescence.

**Resolved**

Use the Desktop adaptation and keep it as close to native behavior as Codex Session Control's architecture allows. Use one authoritative all-depth ancestor discovery and exact per-descendant interruption, without adding cache state, catch-up passes, recursion, or broader public thread-list features.

**Rationale**

Desktop's first pass operates on topology it already maintains for rendering and navigation; it is a latency optimization before authoritative discovery, not a separate server-side cancellation guarantee. Codex Session Control has no such cache, and creating one only for this operation would expand state and failure surface without improving the native lineage source. Codex `0.147.0` already provides the exact all-depth `ancestorThreadId` query needed to include children, grandchildren, and deeper spawned agents. Fully paginating that query and refreshing each exact active turn preserves both native topology semantics and the controller's existing mutation-safety pattern.

**Approaches Considered**

1. **Desktop authoritative-discovery adaptation** - Selected because it uses Codex's native all-depth lineage query and exact interrupts while omitting Desktop-only cache machinery.
2. **Literal Desktop cache plus discovery implementation** - Rejected because Codex Session Control has no UI topology cache and introducing one would be unrelated persistent state and complexity.
3. **Recursive direct-child traversal with catch-up passes** - Rejected because `ancestorThreadId` already returns the complete spawned subtree, making custom traversal and retry policy redundant.
4. **Loop until no active descendants remain** - Rejected because Desktop does not do this and it would create a potentially blocking quiescence contract outside the surgical fix.

### Q7: What should happen when one descendant interruption fails?

**Recommended**

Continue attempting every other discovered active descendant and return machine-readable per-descendant outcomes alongside the unchanged root result. Reuse the existing interrupt success/no-op shape and existing structured `ToolErrorData` rather than inventing prose or hiding errors in logs. Preserve `outcome_unknown` for the exact affected descendant and never retry it. This follows Desktop's per-descendant failure isolation while making the outcome visible to an MCP caller.

**Resolved**

Use isolated best-effort descendant handling with structured outcomes. One descendant failure or policy rejection must not prevent eligible attempts against the remaining discovered active descendants, and the tool must not collapse already-applied root or descendant mutations into a single fail-fast MCP error. Q13 classifies an implicit caller descendant as a structured policy rejection rather than an interruption attempt.

**Rationale**

Tree interruption is necessarily a sequence of exact native mutations. By the time one descendant fails, the root and other concurrent descendants may already have been interrupted, so a top-level error would obscure partially applied state and could encourage an unsafe whole-call retry. Continuing across independent targets matches native Desktop cleanup behavior; returning the outcomes rather than merely logging them preserves the agent-facing MCP requirement. Q8 and Q9 resolve the exact result nesting and discovery-error fields.

**Approaches Considered**

1. **Continue and return structured per-descendant outcomes** - Selected because it isolates independent failures, exposes partial state, and preserves at-most-once handling for uncertain mutations.
2. **Fail fast on the first descendant error** - Rejected because it can leave later descendants running and cannot undo mutations already applied.
3. **Run descendant cleanup entirely in the background and only log failures** - Rejected because it mirrors Desktop UI timing but deprives an MCP agent of actionable completion evidence.

### Q8: How should descendant information be added to the existing result?

**Recommended**

Add one optional nested `descendants` object and leave the existing top-level `interrupted` and `turnId` fields unchanged. When descendant interruption was not requested, the object contains a structured warning only if active descendants remain. When `includeDescendants` is true, it contains ordered per-thread entries whose `result` reuses the existing `{ interrupted, turnId? }` shape or whose `error` reuses the existing structured mutation error. Omit the entire object for existing calls when no active descendants need reporting.

**Resolved**

Use the nested `descendants` envelope. Preserve the examples in which root success, root no-op, descendant success, descendant no-op, and per-descendant uncertain failure are represented without changing the meaning or names of the existing root fields.

**Rationale**

A single additive envelope keeps descendant-specific state out of the established root contract and gives an agent one stable place to inspect. Flat fields would spread warning, results, and operation-level failures across the root object, while summary counts would hide exact target outcomes. Reusing existing result and error shapes minimizes new concepts. Q9 resolves the operation-level discovery failure as `descendants.error` with a stable semantic code.

**Approaches Considered**

1. **Nested `descendants` envelope** - Selected because it preserves root compatibility and groups all additive subtree state.
2. **Flat top-level descendant fields** - Rejected because warning, result, and failure fields would clutter and blur the root contract.
3. **Counts and thread IDs only** - Rejected because agents could not distinguish exact successful, inactive, failed, and uncertain targets.

### Q9: How should an authoritative descendant-discovery failure be shaped?

**Recommended**

Use `descendants.error` with a stable semantic `code: "descendant_discovery_failed"`, then flatten the existing structured error fields alongside that code. The semantic code tells an agent which subtree operation failed; `category`, `stage`, native details, dispatch state, and observation retain the existing error taxonomy and evidence. This is more consistent with `descendants.warning` than a specially named `discoveryError` member.

**Resolved**

Use the flattened `descendants.error` shape. Its `code` is `descendant_discovery_failed`; the remaining members reuse the existing structured error contract, including `category`, `message`, `tool`, `stage`, `threadId`, and any applicable native, dispatch, observation, or reconciliation fields.

**Rationale**

The earlier `discoveryError` proposal made the failed phase visible in the field name and preserved `ToolErrorData` verbatim, but those advantages do not justify inconsistent branching. The existing `stage: "thread/list"` already carries the native phase, while a sibling `warning` or `error` object with a stable code gives agents a uniform control shape. Flattening avoids an unnecessary `details` or `cause` layer. Individual `results[].error` entries do not need another semantic code because their position and `threadId` already identify an exact descendant interruption failure.

**Approaches Considered**

1. **`descendants.error` with flattened semantic code and error fields** - Selected for consistent agent branching and minimal nesting.
2. **`descendants.error` with nested `details: ToolErrorData`** - Rejected because it adds structure without clarifying a single operation-level failure.
3. **`descendants.discoveryError` containing `ToolErrorData`** - Rejected because the field-name distinction duplicates `stage` and diverges from the warning shape.

### Q10: What should `includeDescendants` do when the root interruption returns an error?

**Recommended**

Preserve the current top-level root error and stop before descendant discovery or mutation. A normal `{ "interrupted": false }` root result is not an error and must still allow the requested descendant handling. Continuing after an actual root error would either hide applied descendant mutations behind an MCP error or require a new composite root-error result contract.

**Resolved**

On any actual root `ToolErrorData`, including `outcome_unknown`, return the existing error unchanged and do not discover or interrupt descendants. When the root has no active turn and returns `interrupted: false`, continue with descendant discovery and either interrupt or warn about active descendants according to `includeDescendants`.

**Rationale**

This preserves the existing root error contract, prevents unreported descendant mutations after a top-level failure, and avoids inventing a composite result solely for this edge case. The caller can inspect and resolve the root error explicitly before making another mutation decision.

**Approaches Considered**

1. **Preserve the root error and stop** - Selected because it keeps the current contract and mutation reporting truthful.
2. **Continue descendant handling but return the root MCP error** - Rejected because descendant mutations would be hidden from the caller and unsafe to retry.
3. **Return a new composite root-error result** - Rejected because it expands the API beyond the surgical descendant fix.

### Q11: Should discovered descendants be interrupted concurrently or sequentially?

**Recommended**

Handle eligible discovered descendants concurrently, using one independently initialized app-server connection per attempted descendant. Desktop issues its descendant interrupts in parallel, which prevents one slow or failed descendant from delaying containment of the others. Codex Session Control's current `AppServerConnection` serializes requests through mutable access, so independent connections reproduce that behavior without redesigning the transport into a multiplexed client. Q13 preserves the existing caller self-target prohibition before connection initialization.

**Resolved**

Use concurrent handling through independent initialized connections for every eligible non-caller descendant. A protected caller receives only its ordered policy-error result entry. Do not change the existing connection abstraction or add a controller-specific concurrency limit.

**Rationale**

Parallel handling most closely matches native Desktop behavior and the previously selected failure isolation: every eligible non-caller descendant gets an independent attempt even if another connection or interruption stalls or fails, while the protected caller is accounted for without a connection or mutation. `AppServerClient` is already cloneable, every MCP call already creates an initialized connection, and outcome-unknown reconciliation already opens separate initialized connections, so this remains localized rather than introducing a transport refactor. Results retain discovery order rather than completion order so concurrent scheduling does not make the response shape nondeterministic.

**Approaches Considered**

1. **Concurrent independent connections** - Selected as the closest practical equivalent to Desktop's parallel requests under the controller's current transport.
2. **Sequential requests on the existing connection** - Rejected because one timeout would delay every later descendant and enlarge the containment window.
3. **Bounded concurrency** - Rejected because it adds an arbitrary controller-specific policy that native Desktop does not have and is unnecessary for this scoped fix.

### Q12: What should `includeDescendants` return when no active descendants exist?

**Recommended**

When `includeDescendants: true` completes authoritative discovery successfully and finds no active descendants, return `"descendants": { "results": [] }`. The explicit empty result distinguishes a completed subtree scan from a root-only operation. Existing calls that omit the parameter or pass `false` should still omit `descendants` when no warning is needed, preserving their exact prior output.

**Resolved**

Return an explicit empty `descendants.results` array for a successful opted-in scan with no active targets. Preserve the old root-only output for omitted or false `includeDescendants` calls when there are no active descendants.

**Rationale**

The opted-in call requests an additional authoritative operation, so its result should prove that operation ran even when it produced no targets. An empty array provides that proof using the same field callers inspect when targets exist, without adding a redundant count or changing any pre-existing call result.

**Approaches Considered**

1. **Explicit empty results array** - Selected because it reports successful evaluation through the established descendant-result shape.
2. **Omit the descendants envelope** - Rejected because it makes a completed opted-in scan look identical to a root-only result.
3. **Return a separate zero count** - Rejected because the empty array already communicates the same fact and remains the canonical iterable result.

### Q13: What should happen when the caller is an active descendant of the requested root?

**Recommended**

Preserve the controller's existing caller self-interruption prohibition as a per-descendant outcome. Keep the caller in result accounting, return the existing `ToolErrorData` shape with `category: "policy_rejected"`, `tool: "thread_interrupt"`, and `stage: "self_target"`, issue no connection or mutation for that target, and continue handling every other active descendant. This fits the isolated-error contract without silently bypassing or weakening an established safety boundary.

**Resolved**

Use the per-descendant policy error and continue. The caller is identified through the existing MCP `_meta.threadId`; if that ID occurs in the active descendant target set, its ordered result entry contains the existing self-target policy error and no interruption is attempted. Omitted or false `includeDescendants` behavior is unchanged, so an active caller descendant remains visible in the warning like any other active descendant.

**Rationale**

Allowing implicit self-interruption could terminate the turn responsible for collecting and returning subtree outcomes. Rejecting the entire call would require preflight discovery before the root mutation, diverge from the selected native root-first flow, and prevent containment of unrelated descendants. A visible per-target policy outcome preserves complete accounting, retains the existing safety rule, and composes directly with the Q7 best-effort failure model.

**Approaches Considered**

1. **Return a per-descendant policy error and continue** - Selected because it preserves self-target safety, complete target accounting, root-first ordering, and containment of other descendants.
2. **Reject the entire operation before root interruption** - Rejected because it requires an extra preflight discovery phase and prevents otherwise valid containment work.
3. **Allow implicit caller interruption** - Rejected because it violates the existing self-target boundary and can terminate the call before outcomes are returned.
4. **Silently exclude the caller** - Not presented because it would contradict the explicit result-accounting requirement and hide an active descendant left running.
