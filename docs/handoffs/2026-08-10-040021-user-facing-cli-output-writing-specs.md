# User-Facing CLI Output Writing Specs Handoff

## Objective

Write an implementation-ready specification for the approved user-facing CLI output refactor. This is a specification-only continuation: synthesize the active product, safety, diagnostic, testing, and complexity decisions without implementing code or reproducing superseded brainstorming history.

## Relevant Skills

- `$superpowers:writing-specs`: validate the approved brainstorming source, synthesize only active decisions, run the specification review workflow, and stop at its phase boundary.

## Start Here

### Read

1. `AGENTS.md`
2. `docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md`
3. `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-brainstorming-review.md`
4. `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-rust-architecture-review.md` only as supporting provenance; Q27-Q31 and the brainstorming artifact's supersession markers are authoritative where it differs.

### Check

Run:

```bash
git status --short --branch
sed -n '1,35p' docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md
```

Confirm branch `codex/user-facing-cli-output`, an approved brainstorming status, coherent `Review`/`Next` metadata, and no unexpected code changes. Artifact whitespace and supersession checks passed before this handoff; code tests were not run because this phase changed documentation only.

### Resume

Invoke `$superpowers:writing-specs` and write a concise active-only specification. Treat brainstorming as complete and approved. Do not reopen approved decisions unless current repository evidence proves a contradiction or invalid state.

## Current State

The repository is on `codex/user-facing-cli-output` at `528ef31566048520617929a45ea446ff9af70559` before the brainstorming boundary commit. No production or test code has changed. The approved artifact contains 31 resolved decisions and links a passed review trace.

The approved default UX includes exact root/subcommand help, setup/update/status/enable/disable/uninstall/wrapper output, warnings, status vocabulary and exit codes, running-client guidance, safety refusals, partial outcomes, cancellation behavior, and the unchanged active-task update prompt.

The final YAGNI direction is authoritative:

- one human rendering boundary;
- one concrete diagnostic emitter, not a sink trait hierarchy;
- one non-generic success enum;
- one bounded failure enum with explicit variants only for materially different safety and partial outcomes;
- one final typed status result using existing internal evidence;
- demand-driven verbose events;
- risk-based tests without Cartesian combinations;
- focused module splitting is allowed;
- active requirements only in the specification.

The reviewed proportionate estimate is 700–1,300 net production lines and 700–1,400 net test/documentation lines. These are review context, not implementation quotas.

## Guardrails

### Do Not Reopen

- Exact approved user-facing copy and output ordering.
- `ready`, `unavailable`, `unhealthy`, and `could not verify` integration vocabulary.
- Top-level status exit matrix from Q22.
- Q24's existing active-task prompt; it must remain unchanged.
- Q26's independent CLI/Desktop fact production and guidance precedence matrix.
- Q27's minimal architecture direction.
- Q28's demand-driven verbose scope and absolute diagnostic privacy boundary.
- Q29's risk-based testing policy.
- Q30's narrow escalation policy.
- Q31's active-only spec synthesis rule.
- Fully superseded Q9 and the superseded portions of Q7, Q16, Q18, and Q23.

### Constraints

- Do not turn the specification into a generic CLI framework, message catalog, localization design, error-system rewrite, or parallel status evidence hierarchy.
- Do not ban focused modules, local traits, builders, or other patterns by name; require a concrete proportional use.
- Do not copy the 31-question chronology into the specification. Extract the active acceptance contract and link the brainstorming artifact for rationale.
- Preserve lifecycle safety, diagnostic privacy, staged-update outer/apply ownership, wrapper silence, and MCP stdout protocol isolation.
- Preserve existing operational tests and fixtures; specify new focused tables only where distinct presentation or interaction behavior needs coverage.
- Do not start implementation, planning, branching, publishing, or pull-request work in the specification session.

### Ask The Human If

- Current code evidence contradicts an approved user-facing or safety decision.
- The specification would change approved output, lifecycle behavior, privacy, exit codes, or active-task semantics.
- The design requires a new dependency, background/concurrent subsystem, cross-process serialization protocol, or unrelated MCP/app-server/domain changes.
- The credible implementation scope exceeds roughly 1,500 net production lines or 1,800 net test/documentation lines.

## Next Work

1. Validate the approved brainstorming artifact and its active/superseded decision boundaries.
2. Write the concise implementation-ready specification with measurable acceptance criteria and the minimal architecture from Q27-Q31.
3. Run the `writing-specs` review and approval workflow, persist only material review state, and stop at the specification boundary.

## Pointers

- Approved source: `docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md`
- Material review decisions: `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-brainstorming-review.md`
- Supporting architecture provenance: `docs/superpowers/reviews/2026-08-10-user-facing-cli-output-rust-architecture-review.md`
- Branch before boundary commit: `codex/user-facing-cli-output`
- Pre-boundary HEAD: `528ef31566048520617929a45ea446ff9af70559`
