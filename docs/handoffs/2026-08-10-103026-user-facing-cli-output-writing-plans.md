# User-Facing CLI Output Writing Plans Handoff

## Objective

Write the TDD-ready implementation plan for the approved user-facing CLI output specification. This is a fresh planning-only phase; specification writing is complete and implementation must not begin.

## Relevant Skills

- `$superpowers:writing-plans`: turn the approved implementation-ready specification into an exact, executable TDD plan.

## Start Here

### Read

1. `AGENTS.md`
2. `docs/superpowers/specs/2026-08-10-user-facing-cli-output-design.md`
3. `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-design-review.md`

Use the approved specification as the active source of truth. Consult `docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md` only when the specification links it for provenance or when current repository evidence appears to contradict the active contract.

### Check

In the worktree containing this handoff, verify the exact checkout and artifact boundary before writing:

```bash
pwd
git status --short
git log -1 --oneline --decorate
git merge-base HEAD origin/main
rg -n '^\*\*(Status|Source|Review|Next):' \
  docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md \
  docs/superpowers/specs/2026-08-10-user-facing-cli-output-design.md
rg -n '^## Final State|^passed$' \
  docs/superpowers/reviews/2026-08-10-user-facing-cli-output-brainstorming-review.md \
  docs/superpowers/reviews/2026-08-10-user-facing-cli-output-design-review.md
```

Expected: clean worktree; brainstorming and specification status `approved`; both review traces passed; merge base `528ef31566048520617929a45ea446ff9af70559`.

### Resume

Invoke `$superpowers:writing-plans` and write the implementation plan from the approved specification. Decompose the exact expected file targets into coherent red/green/refactor slices with spec-compliance review before code-quality review and fresh verification before completion.

## Current State

- The approved specification is `docs/superpowers/specs/2026-08-10-user-facing-cli-output-design.md`.
- The specification review trace passed after the required three-role gate; the final Contract, Feasibility, and User Workflow verdicts were all `PASS` with no findings.
- The source brainstorming artifact and its review trace are approved and passed after the narrow Return From Spec Writing repair.
- Repository classification found no current producer that requires new user-facing behavior beyond Q23/Q25/Q27 and the approved `UpdateCompletionUnknown` result.
- At handoff generation, the specification worktree was `/home/korty/.codex/worktrees/6ae4/codex-session-control`, detached at source commit `3d73ce13eda5cf9052e817617d736faac48ca719`; the boundary commit containing this handoff should be the next session's current `HEAD`.

## Guardrails

### Do Not Reopen

- The exact approved success, warning, failure, status, help, prompt, wrapper, verbose/privacy, exit-code, and channel copy.
- Normal staged-candidate exit `0`/`1` is candidate-owned and propagated without a second friendly result.
- Wait failure, signal termination, or exit outside `0`/`1` after successful candidate spawn uses the exact stderr-only, exit-`1`, no-immediate-retry `UpdateCompletionUnknown` result.
- Producer mapping is technical planning/implementation work under Q23/Q27. Existing branches select complete bounded `UserFailure` variants directly; stages remain diagnostic-only.
- The active-task gate, wrapper silence, and MCP stdout isolation contracts.

### Constraints

- Planning only. Do not modify production code or tests in this task.
- Do not add independent problem/recovery/mutation/retry axes, a stage-message table, string parsing, a cross-process result protocol, a generic output framework, or a broad error-system rewrite.
- Preserve the two narrow evidence seams: candidate `spawn` versus `wait`, and descriptor final/stage residue plus cleanup evidence.
- Keep tests focused by distinct behavioral contract and existing operational fixture; do not create a Cartesian matrix.
- The implementation plan requires Operator approval before production or test changes begin.

### Ask The Human If

- Current repository evidence proves a material contradiction with the approved specification.
- One concrete producer requires user-facing copy or safety semantics not already approved.
- Planning requires a new dependency, background/concurrent subsystem, cross-process protocol, unrelated MCP/app-server/domain scope, or scope beyond the specification's escalation threshold.

## Next Work

1. Validate the approved spec/review boundary and exact checkout.
2. Use `$superpowers:writing-plans` to create the complete TDD-ready implementation plan with prerequisites, numbered actionable tasks, exact file/test targets, verification, review order, and measurable acceptance gates.
3. Run the plan's required review/approval boundary, then stop or create the explicitly approved fresh implementation handoff; do not implement from this task.

## Pointers

- Approved spec: `docs/superpowers/specs/2026-08-10-user-facing-cli-output-design.md`
- Spec review: `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-design-review.md`
- Approved source: `docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md`
- Source review: `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-brainstorming-review.md`
- Source-writing handoff: `docs/handoffs/2026-08-10-040021-user-facing-cli-output-writing-specs.md`
- Source commit before this boundary: `3d73ce13eda5cf9052e817617d736faac48ca719`
- Base: `528ef31566048520617929a45ea446ff9af70559`
