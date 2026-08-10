# User-Facing CLI Output Implementation Handoff

## Objective
Implement the approved user-facing CLI output plan in a separate implementation task using TDD, without reopening the approved copy, safety, lifecycle, or process-boundary decisions.

## Relevant Skills
- `$superpowers:executing-plans`: execute the approved plan task-by-task with its milestone review checkpoints.
- `$superpowers:test-driven-development`: preserve every explicit RED, GREEN, and refactor boundary.
- `$superpowers:verification-before-completion`: require fresh focused and repository-wide evidence before completion claims.
- `$superpowers:requesting-code-review`: run spec-compliance review before code-quality review at the two plan-defined checkpoints.

## Start Here

### Read
- `docs/superpowers/plans/2026-08-10-user-facing-cli-output.md`
- `docs/superpowers/specs/2026-08-10-user-facing-cli-output-design.md`
- `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-plan-review.md`
- `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-design-review.md`

### Check
From the dedicated implementation worktree, run:

```bash
implementation_base_sha=$(git log --diff-filter=A --format=%H -1 -- docs/superpowers/plans/2026-08-10-user-facing-cli-output.md)
test -n "$implementation_base_sha"
test "$(git rev-parse HEAD)" = "$implementation_base_sha"
test -z "$(git status --short)"
test "$(git merge-base HEAD origin/main)" = "528ef31566048520617929a45ea446ff9af70559"
```

### Resume
Invoke `$superpowers:executing-plans`, validate the plan exactly as written, and start Task 1. Planning and approval are complete; there is no producer-level or ceremonial approval gate.

## Current State
The design spec is `approved`, its review trace is `passed`, the implementation plan is `approved`, and the plan review trace is `passed`. The plan has four proportional milestones: typed surface, lifecycle truth, update/status/process boundaries, and documentation/final proof. Ordered spec-compliance then code-quality review occurs once after Milestone B and once after fresh final verification.

## Guardrails

### Do Not Reopen
- Exact user-facing copy, channels, exit codes, and safety/recovery decisions in the approved spec.
- Direct bounded `UserFailure` selection at concrete producer boundaries.
- Candidate normal exit `0`/`1` ownership, spawn-versus-wait distinction, exact `UpdateCompletionUnknown`, descriptor residue evidence, active-task prompt/recheck semantics, wrapper silence, verbose privacy, and MCP stdout isolation.

### Constraints
- Do not add orthogonal problem/recovery/mutation/retry axes, a generic stage-message table, raw string parsing, a cross-process result protocol, a broad output framework, or an error-system rewrite.
- Use focused risk-based tests and existing operational fixtures; do not create a Cartesian matrix or duplicate mutation tests merely to restate prose.
- Follow the plan's exact file ownership, RED/GREEN commands, commit boundaries, milestones, review placement, manual disposable-systemd gate, and final verification.
- Do not add dependencies or background/concurrent subsystems. Never stage or commit `.autonomous`.

### Ask The Human If
Only when a plan stop condition is met: a material spec contradiction, a concrete producer requiring new user-facing behavior or safety, a new dependency/background subsystem/protocol, scope beyond the approved threshold, unrelated worktree dirt that cannot be isolated, or another genuine Operator decision.

## Next Work
1. Validate the implementation base, detached/branch state, required tools, and clean worktree exactly as specified in Prerequisites.
2. Execute Tasks 1-4 with their TDD commits, then run the single ordered Milestone B review checkpoint and repair valid findings.
3. Execute Tasks 5-7, run fresh focused and full verification, then run the final ordered review and record the disposable CI gate honestly.

## Pointers
- Pre-plan source boundary: `6714c542e1209d194fa7924a42065481780675e7`
- Approved spec source commit: `3d73ce13eda5cf9052e817617d736faac48ca719`
- Merge base: `528ef31566048520617929a45ea446ff9af70559`
- Resolve the implementation base from the commit that first added the plan; do not substitute the pre-plan source boundary.
- The ignored `live_normal_home_*` cases run only through `bash scripts/ci/disposable-systemd-user-contract.sh` in a disposable systemd-user environment.
