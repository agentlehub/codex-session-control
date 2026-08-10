# User-Facing CLI Output Review Trace

## Review Surface

Reviewed: `docs/superpowers/brainstorming/2026-08-09-user-facing-cli-output.md`
Review type: brainstorming
Saved because: The review changed lifecycle outcomes, safety recovery, status exit semantics, diagnostic privacy, and setup/enable composition that the specification and implementation must preserve.

## Material Outcomes

- `MAJOR` Default-visible lifecycle failure and recovery coverage was incomplete.
  - Disposition: fixed
  - Evidence pointer: Q23
  - Next action: Use the closed typed problem, recovery, retry-safety, partial-outcome, and cancellation contract in the specification.
- `MAJOR` Already-running client guidance was missing and initially composed ambiguously with generic guidance.
  - Disposition: fixed
  - Evidence pointer: Q20 and Q26
  - Next action: Derive CLI and Desktop process facts independently and apply the approved precedence matrices.
- `MAJOR` An already-current update did not retain disabled-service guidance.
  - Disposition: fixed
  - Evidence pointer: Q21
  - Next action: Retain service enablement in the typed already-current outcome.
- `MAJOR` Rich status states lacked an explicit exit-code contract.
  - Disposition: fixed
  - Evidence pointer: Q22
  - Next action: Implement and test the approved top-level exit matrix.
- `MAJOR` Unrestricted error strings could violate the verbose-output privacy contract.
  - Disposition: fixed
  - Evidence pointer: Q16 and Q18
  - Next action: Render only typed allowlisted diagnostic evidence and cover prohibited data with sentinel tests.
- `MAJOR` Partial completion and manual-gate outcomes could be rendered as false total failures.
  - Disposition: fixed
  - Evidence pointer: Q23, Q24, and Q25
  - Next action: Preserve the existing active-task prompt and use the approved partial, refusal, and cancellation outcomes.
- `MINOR` The active decision index cited fully superseded Q9.
  - Disposition: fixed
  - Evidence pointer: Current Decisions and Q17
  - Next action: none
- `MAJOR` The reviewed implementation architecture was disproportionate to a CLI copy refactor.
  - Disposition: fixed
  - Evidence pointer: Q27
  - Next action: Use one renderer boundary, one concrete diagnostic emitter, one final status result, one bounded failure enum, and existing command-local stage/evidence types.
- `MAJOR` The initial verbose contract risked becoming an exhaustive second product surface.
  - Disposition: fixed
  - Evidence pointer: Q28
  - Next action: Implement only the demand-driven diagnostic set and require a concrete support question for expansion.
- `MAJOR` Presentation tests risked a command and state Cartesian product.
  - Disposition: fixed
  - Evidence pointer: Q29
  - Next action: Reuse operational fixtures and cover distinct rendering, classification, privacy, and process-boundary risks with focused tables.
- `MINOR` The future specification could accidentally reproduce superseded brainstorming history and provisional architecture.
  - Disposition: fixed
  - Evidence pointer: Q31
  - Next action: Synthesize only active requirements in the specification and retain this artifact as provenance.
- `MINOR` Q27's two-module wording contradicted Q30's permission for focused module splitting.
  - Disposition: fixed
  - Evidence pointer: Q27 and Q30
  - Next action: Start with the two named responsibilities and split further only for a concrete clearer boundary under ordinary review.

## Decisions

- The operator approved Q20-Q23 and Q25-Q26 after review-driven revisions.
- The operator rejected rewriting the previously designed active-task confirmation prompt; Q24 preserves it unchanged.
- The initial three-role review was followed by the one automatic rerun allowed by the brainstorming workflow. Design Branch Coverage and Spec-Handoff Readiness passed. Decision Closure found the Q25 and Q26 gaps; Q25 was resolved with the operator, and the same Decision Closure reviewer supplied the corrected Q26 matrix that the operator approved.
- The review-stage absence of whole-artifact approval was rejected as a finding because final approval follows review handling by design.
- A fresh Sol/xhigh simplicity review returned `OVERENGINEERED` for the implementation architecture while affirming the approved user experience. The operator required every finding to be presented rather than auto-applied, then approved the architecture, diagnostic, testing, complexity-policy, and spec-synthesis dispositions individually in Q27-Q31.
- The simplicity reviewer's blunt module and pattern limits were not adopted. Q30 permits focused module splitting and ordinary implementation judgment, with exceptional pauses only for changed approved behavior, substantial measured growth, runtime/dependency/protocol expansion, or unrelated subsystem scope.
- The same simplicity reviewer reran against Q27-Q31 and returned `PASS_WITH_ISSUES`: every substantive overengineering concern was resolved, and the sole module-count wording issue was fixed with operator approval. Its proportionate implementation estimate is 700–1,300 net production lines and 700–1,400 net test/documentation lines.

## Final State

Passed. All review findings were resolved, and the operator approved the complete brainstorming artifact.
