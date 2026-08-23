# Descendant Cancellation Gap Implementation Handoff

## Objective

Implement the operator-approved surgical `thread_interrupt` extension from `docs/superpowers/plans/2026-08-23-descendant-cancellation-gap.md`. Execute the reviewed TDD slices exactly, preserve the fixed contract, complete the mandated gates, create one local implementation commit, and stop without publication or live-session mutation.

## Relevant Skills

- `$superpowers:subagent-driven-development`: recommended execution mode for implementing the approved plan task by task with isolated worker context.
- `$superpowers:executing-plans`: permitted inline execution mode when keeping this localized change in one implementation context is more efficient.
- `$superpowers:test-driven-development`: enforce each compile-coherent RED before its corresponding GREEN production change.
- `$superpowers:verification-before-completion`: require fresh focused and repository-gate evidence before completion claims or commit.

Execution-mode choice does not authorize new task splits, artifacts, milestones, review waves, abstractions, or adjacent work.

## Start Here

### Read

- `AGENTS.md`
- `docs/superpowers/plans/2026-08-23-descendant-cancellation-gap.md`
- `docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md`
- `docs/superpowers/reviews/2026-08-23-descendant-cancellation-gap-plan-review.md`

The implementation worker does not need to reread the brainstorming source or earlier handoffs during startup unless current repository evidence contradicts the approved plan or review trace. Task 6's spec-compliance reviewer must still read the approved brainstorming source exactly as the plan requires.

### Check

Run before any edit:

```bash
git status --short --branch
git log -1 --oneline
git rev-parse HEAD
git merge-base HEAD main
rg -n '^\*\*Status:\*\* approved$' docs/superpowers/plans/2026-08-23-descendant-cancellation-gap.md docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md
rg -n '^Passed on the operator-authorized third combined review pass with no findings\.$' docs/superpowers/reviews/2026-08-23-descendant-cancellation-gap-plan-review.md
```

Require branch `fix/descendant-cancellation-gap`, merge-base `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`, the workflow boundary commit containing this handoff, and a clean worktree. Stop on unrelated dirt instead of cleaning or absorbing it.

### Resume

Choose or confirm one execution mode, then start Task 1 of the approved plan. Use `subagent-driven-development` by default; use `executing-plans` when inline execution avoids needless coordination. The plan is the source of truth, planning is complete, and approved behavior must not be reopened without contradictory current code evidence.

## Current State

- Branch: `fix/descendant-cancellation-gap`.
- Main merge-base: `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`.
- Pre-boundary HEAD: `c94e7b495b9f0e381f82b03d8885958c18cc34ce` (`docs: approve descendant cancellation spec`).
- The implementation plan is operator-approved.
- The operator-authorized third combined plan review returned `PASS` with no findings; the durable review trace records all repaired findings and the final pass.
- No production code, tests, README content, dependency, or implementation-support file has been changed.
- Codex App Server support remains `0.147.0`.
- The implementation base is the workflow boundary commit containing the approved plan, review trace, and this handoff; obtain its exact SHA from `git rev-parse HEAD` during preflight.

## Guardrails

### Do Not Reopen

- Do not rename `interrupted`, add confirmation semantics, or reinterpret native interrupt responses.
- Do not replace `includeDescendants: boolean` with an enum, scope abstraction, second tool, or public lineage API.
- Do not change root-first behavior, warning/error/result shapes, all-depth authoritative discovery, stable first-seen deduplication, concurrent independent connections, discovery-ordered returned entries, caller protection, failure isolation, or at-most-once mutation.
- Do not couple interruption to goal state or add retries, catch-up passes, recursion, quiescence loops, post-response proof, or transport multiplexing.
- Do not rerun brainstorming, specification writing, or plan design. Planning is closed unless current code makes the approved plan unsafe or infeasible.

### Constraints

- Apply this scope filter before every edit: identify the approved acceptance criterion, numbered plan task, and named file seam it implements. If any one is missing, do not make the edit.
- Follow Tasks 1-7 in order. Do not add process artifacts, status ledgers, speculative investigations, generic frameworks, helper registries, services, managers, runners, new dependencies, extra milestones, or redundant review waves.
- Keep the implementation to the eleven files allowlisted by the plan. Do not change `src/error.rs`, `src/model.rs`, `src/app_server/protocol.rs`, Cargo files, unrelated tests, or adjacent documentation.
- Test-harness work is only the source-compatible `FakeResponse::Controlled` path, concurrent initialized handlers, and initialization-failure scripting required by the approved concurrency/failure proofs. Do not redesign the harness.
- Concurrent target request arrival and completion order are unspecified. Assert only completed discovery before target reads, actual overlap, at-most-once target mutation, and discovery-ordered result entries.
- Use exact-file reads and targeted `rg`; do not perform a broad architecture audit or refactor nearby code because it looks improvable.
- Process theater is a defect here. Every command, artifact, reviewer, and code change must be required by the approved plan or a concrete failing gate.
- Run the mandated sequence literally: executable RED/GREEN slices, focused GREEN set, pre-review `./scripts/check.sh`, spec-compliance review, code-quality review, final post-review `./scripts/check.sh`, staged allowlist validation, one local Conventional Commit.
- Do not push, create/update a PR, merge, tag, release, restart services, or mutate live Codex sessions.

### Ask The Human If

- Current code contradicts an approved behavior or makes a plan step unsafe or infeasible.
- Any fix requires a new dependency, public API, architecture/transport change, generalized abstraction, extra production file, or changed product behavior.
- The checkout is dirty with unrelated work or the plan/spec/review approval state is missing or contradictory.
- A reviewer proposes scope expansion, preference-only redesign, or process not required by the plan; do not accept it silently.

## Next Work

1. Run the exact preflight and choose or confirm the execution mode.
2. Execute plan Tasks 1-4 in order, preserving each executable RED before GREEN and refusing unapproved scope.
3. Execute Tasks 5-7 exactly: documentation, fresh gates, ordered reviews, final gate, one allowlisted local commit, clean-state proof, and stop.

## Pointers

- Approved plan: `docs/superpowers/plans/2026-08-23-descendant-cancellation-gap.md`
- Approved spec: `docs/superpowers/specs/2026-08-23-descendant-cancellation-gap-design.md`
- Passed plan review: `docs/superpowers/reviews/2026-08-23-descendant-cancellation-gap-plan-review.md`
- Planning handoff: `docs/handoffs/2026-08-23-014548-descendant-cancellation-gap-writing-plans.md`
- Branch: `fix/descendant-cancellation-gap`
- Main merge-base: `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`
- Pre-boundary HEAD: `c94e7b495b9f0e381f82b03d8885958c18cc34ce`
- Focused and final commands are authoritative only as written in the approved plan; do not invent parallel gate sequences.
