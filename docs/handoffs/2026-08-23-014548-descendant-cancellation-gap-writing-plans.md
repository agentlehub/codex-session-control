# Descendant Cancellation Gap Writing Plans Handoff

## Objective

Write a lean, TDD-ready implementation plan for the approved surgical `thread_interrupt` extension in `docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md`. This session is plan-only: turn the settled contract into precise RED/GREEN implementation steps without writing production code or reopening approved design decisions.

## Relevant Skills

- `$superpowers:writing-plans`: validate the approved spec and produce the implementation plan, review gate, approval boundary, and fresh implementation handoff.

## Start Here

### Read

- `docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md`
- `AGENTS.md`
- Only the exact production and test seams listed under the spec's `Expected File Changes` section.

Read the brainstorming source or its review trace only if spec validation requires it. The approved spec is the implementation contract; do not use earlier exploration to reopen settled behavior.

### Check

Run `git status --short --branch` and `git log -1 --oneline`. Confirm the branch is `fix/descendant-cancellation-gap`, the spec is `approved`, the boundary commit contains only the approved spec and this handoff, and no production implementation or implementation plan has started.

### Resume

Invoke `$superpowers:writing-plans`, validate the approved spec against the narrow current code seams, and write the smallest unambiguous TDD-ready plan that satisfies every acceptance criterion. Stop at the plan review and approval boundary; do not implement in the planning session.

## Current State

The branch is `fix/descendant-cancellation-gap`, originally based on clean `main` commit `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`. The approved brainstorming work is committed at `cceb74c9d8fd852deb11207097c22f337a747eec`; Codex Session Control supports Codex `0.147.0`.

The implementation-ready spec is operator-approved. One combined artifact-readiness review covering contract fidelity, repository feasibility, and workflow clarity returned `PASS` with no findings, so no durable spec review trace was needed. No production code, tests, implementation plan, dependency change, or public lineage API has been created.

The spec already resolves the only minor implementation gap: active descendant IDs are stably deduplicated across paginated results so every target is accounted for once and mutation remains at-most-once.

## Guardrails

### Do Not Reopen

- Do not rename `interrupted`, add confirmation semantics, or reinterpret the native interrupt response.
- Do not replace optional `includeDescendants: boolean` with an enum, scope abstraction, or separate tool.
- Do not change root-first ordering, the false/omitted warning behavior, the true/empty `results: []` behavior, or the nested discovery-error and per-target result shapes.
- Do not replace authoritative all-depth `ancestorThreadId` discovery with recursion, direct-child traversal, cache state, catch-up passes, or a quiescence loop.
- Do not change concurrent independent descendant connections, deterministic discovery-order results, caller self-target rejection, or failure isolation.
- Do not couple interruption to goal state or reopen the approved stable first-seen deduplication rule.

### Constraints

- Every plan task must map directly to an approved acceptance criterion and an exact file seam listed in the spec. Remove tasks that exist only to introduce flexibility, layers, or speculative reuse.
- Reuse the existing exact interrupt path, `ToolErrorData`, thread status/parser behavior, `AppServerClient`, and current `futures-util` dependency. Do not introduce a cancellation service, subtree manager, generic concurrency framework, transport redesign, protocol-model expansion, or new dependency.
- Keep the private app-server addition to one spawned-descendant page request. Do not expand public `threads_list` or expose lineage generally.
- Limit test-harness work to deterministic simultaneous initialized connections needed to prove actual concurrency. Do not redesign the fake server or test framework.
- Keep documentation to the existing tool contract text and concise README row identified by the spec.
- Use focused RED tests before GREEN production changes. Preserve fail-closed, at-most-once mutation behavior and never plan a retry for `outcome_unknown`.
- Keep review proportional: spec-compliance review first, code-quality review second, then one final `./scripts/check.sh` gate. Do not split the localized feature into ceremonial milestones or redundant review waves.
- The planning session must not edit production code, tests, README content, dependencies, or implementation-support files.

### Ask The Human If

- Current code evidence contradicts an approved behavior or makes it unsafe or infeasible.
- The plan would require a new dependency, public API, architectural refactor, transport change, generalized abstraction, or production file outside the approved spec scope.
- A behavior decision is genuinely missing from the approved spec. Do not invent one or broaden scope to avoid asking.
- The plan cannot remain localized and TDD-ready without adding steps that do not map to an acceptance criterion.

## Next Work

1. Validate the approved spec metadata and its exact contract against the narrow current source/test seams.
2. Write a concise TDD-ready plan with prerequisites, measurable acceptance criteria, numbered RED/GREEN steps, exact file paths, focused commands, and the final `./scripts/check.sh` gate.
3. Run the plan's required review and operator-approval flow, then create a fresh implementation handoff and stop without coding.

## Pointers

- Approved spec: `docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md`
- Approved source: `docs/superpowers/brainstorming/2026-08-22-descendant-cancellation-gap.md`
- Source review trace: `docs/superpowers/reviews/2026-08-22-descendant-cancellation-gap-review.md`
- Writing-specs handoff: `docs/handoffs/2026-08-23-005055-descendant-cancellation-gap-writing-specs.md`
- Branch: `fix/descendant-cancellation-gap`
- Base commit: `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`
- Approved brainstorming commit: `cceb74c9d8fd852deb11207097c22f337a747eec`
- Supported and inspected Codex version: `0.147.0`
