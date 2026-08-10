# User-Facing CLI Output Design Review Trace

## Review Surface

Reviewed: `docs/superpowers/specs/2026-08-10-user-facing-cli-output-design.md`
Review type: spec
Saved because: The gate found and resolved material staged-update safety and producer-classification gaps before implementation planning.

## Material Outcomes

- `BLOCKER` Abnormal staged-candidate termination has no approved safety-valid parent result.
  - Disposition: fixed
  - Evidence pointer: source `Return From Spec Writing`; `Failure, Safety, and Partial-Outcome Contract`; `Staged Update Ownership`; `Acceptance Criteria` item 14
  - Next action: none
- `BLOCKER` Current overloaded lifecycle stages do not uniquely select the approved failure contract.
  - Disposition: fixed
  - Evidence pointer: source `Return From Spec Writing`; `Producer-Boundary Failure Classification`; `Expected File Targets`; `Verification Guidance`
  - Next action: none
- `MAJOR` Escalation triggers did not preserve Q30's approved boundary.
  - Disposition: fixed
  - Evidence pointer: `Escalation Boundary`
  - Next action: none
- `MAJOR` Final-only status projection discarded required evidence strength.
  - Disposition: fixed
  - Evidence pointer: `Design Boundaries`; `Status Contract`
  - Next action: none
- `MAJOR` Local verification did not execute the authoritative ignored normal-home staged-update cases.
  - Disposition: fixed
  - Evidence pointer: `Verification Guidance`
  - Next action: none
- `MINOR` Warning acceptance criterion overreached into failure and partial-outcome stderr.
  - Disposition: fixed
  - Evidence pointer: `Acceptance Criteria` item 4
  - Next action: none
- `MAJOR` Several grouped uninstall producers did not select one exact approved problem and complete bounded result.
  - Disposition: fixed
  - Evidence pointer: `Producer-Boundary Failure Classification` → `Uninstall producers`
  - Next action: none
- `MINOR` Update unit rendering was incorrectly grouped under the CLI-integration problem.
  - Disposition: fixed
  - Evidence pointer: `Producer-Boundary Failure Classification` → `Update producers`
  - Next action: none
- `MINOR` Enable service failure and descriptor-cleanup rows overlapped.
  - Disposition: fixed
  - Evidence pointer: `Producer-Boundary Failure Classification` → `Enable producers`
  - Next action: none
- `MINOR` Exact expected targets omitted current test owners of descriptor evidence, status shape, and retained stage-string assertions.
  - Disposition: fixed
  - Evidence pointer: `Expected File Targets`; `Verification Guidance`
  - Next action: none

## Decisions

- The default three-role unattended specification review completed one wave.
- Clear independent findings were repaired without changing approved UX or architecture.
- The Operator approved the exact stderr-only, exit-`1`, no-immediate-retry completion-unknown result for abnormal staged-candidate termination.
- The Operator corrected producer-level mapping to technical specification work under Q23/Q27. A repository producer audit found no branch that requires new user-facing behavior or safety semantics.
- Candidate spawn/wait separation and descriptor publication/residue evidence are narrow implementation seams required to select existing bounded variants. They do not add a protocol, independent failure axes, a stage-message table, or a broad error-system rewrite.
- The repaired source return passed one proportional artifact review and was approved before specification work resumed.
- The repaired three-role wave returned Contract `PASS`, Feasibility `PASS_WITH_ISSUES`, and User Workflow `PASS_WITH_ISSUES`. All findings were source-faithful mapping or target-list corrections; none required new user-facing behavior or a product decision.
- Because the User Workflow role's uninstall precision finding was `MAJOR`, the writing-specs policy requires one final repaired wave within its three-wave unattended limit.
- The final repaired wave returned Contract `PASS`, Feasibility `PASS`, and User Workflow `PASS`, each with no findings.
- The Operator's unattended directive made that passing gate the approval condition. The specification is approved for a fresh `writing-plans` task; implementation remains prohibited.

## Final State

passed
