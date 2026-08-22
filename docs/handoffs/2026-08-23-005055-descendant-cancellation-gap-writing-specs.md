# Descendant Cancellation Gap Writing Specs Handoff

## Objective

Write a lean, implementation-ready specification for the approved surgical extension to `thread_interrupt`: optionally interrupt active spawned descendants and warn structurally when the caller retains exact-thread scope. This is a small missing-feature fix, not an architectural project.

## Relevant Skills

- `$superpowers:writing-specs`: validate the approved brainstorming source and produce the implementation-ready specification without reopening settled decisions.

## Start Here

### Read

- `docs/superpowers/brainstorming/2026-08-22-descendant-cancellation-gap.md`
- `docs/superpowers/reviews/2026-08-22-descendant-cancellation-gap-review.md`
- `AGENTS.md`
- Targeted areas only in `src/mcp/contract.rs`, `src/mcp/operations.rs`, `src/app_server.rs`, and `src/error.rs`

### Check

Run `git status --short --branch` and `git log -1 --oneline`. Confirm the branch is `fix/descendant-cancellation-gap`, the brainstorming artifact is `approved`, its review trace ends passed, and no production implementation has started.

### Resume

Invoke `$superpowers:writing-specs`, treat the approved brainstorming artifact as the source of truth, validate it against only the directly relevant code, and write the smallest spec that makes this feature unambiguous to implement and test.

## Current State

The branch was created from clean `main` commit `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`. Codex Session Control supports Codex `0.147.0`. Brainstorming is complete and operator-approved; the material review finding about an implicit caller descendant was resolved in Q13, and the review trace is passed after one minor wording correction. No production code, tests, implementation spec, or implementation plan has been written.

The approved behavior is additive: `includeDescendants?: boolean` defaults to `false`; the root keeps its existing exact-turn behavior and result fields; authoritative all-depth spawned-descendant discovery mirrors native Desktop behavior; opted-in eligible descendants are handled concurrently through independent initialized connections; failures remain structured and target-specific; and an implicit caller descendant receives the existing `policy_rejected` self-target outcome without mutation.

## Guardrails

### Do Not Reopen

- Do not rename `interrupted`, add a confirmation mode, or reinterpret the existing native interrupt response.
- Do not replace `includeDescendants` with an enum, scope abstraction, or separate tool.
- Do not add catch-up passes, recursive tree traversal, quiescence loops, mutation retries, or Desktop-style cache state.
- Do not couple interruption to goal state.
- Do not reopen the structured warning, result envelope, discovery-error, partial-failure, empty-results, concurrency, or caller self-target decisions unless new repository evidence directly contradicts the approved artifact.

### Constraints

- Keep the spec and the resulting plan proportional to a localized feature. Avoid new frameworks, generalized cancellation abstractions, transport redesigns, unrelated refactors, and speculative extensibility.
- Limit production scope to the existing `thread_interrupt` contract and the smallest private app-server support required for authoritative descendant discovery and exact descendant handling.
- Do not expand public `threads_list`, expose lineage generally, change unrelated tools, or clean up nearby code opportunistically.
- Preserve fail-closed, at-most-once mutation behavior and the current self-target safety boundary.
- Specify focused contract, protocol-mapping, pagination, concurrency, race, and failure tests. Retain the repository's two-stage review order and final `./scripts/check.sh` gate, but do not inflate the plan with redundant ceremony.

### Ask The Human If

- Direct code evidence makes an approved behavior infeasible or unsafe.
- Implementing the feature would require a public API change or architectural refactor not authorized by the artifact.
- A genuinely material behavior decision is absent from the approved artifact; do not invent one or broaden scope to avoid asking.

## Next Work

1. Validate the approved artifact against the narrow current code paths without broad repository exploration.
2. Write the lean specification and run its required review and approval flow.
3. Hand the approved spec to a fresh `$superpowers:writing-plans` session, explicitly preserving the same anti-overengineering and no-scope-creep constraints.

## Pointers

- Approved source: `docs/superpowers/brainstorming/2026-08-22-descendant-cancellation-gap.md`
- Review trace: `docs/superpowers/reviews/2026-08-22-descendant-cancellation-gap-review.md`
- Source task: `01a02698-2bab-7013-acf0-346a837ff8f5`
- Incident root: `01a02a76-82a3-71a3-96e5-daca2781233d`
- Incident child: `01a02a8e-40e9-7432-bcd7-0ff66d86f74f`
- Branch: `fix/descendant-cancellation-gap`
- Base commit: `ac81dbd09be5e5fdae1fe23cdfd03ea6b0661eb5`
- Supported and inspected Codex version: `0.147.0`
