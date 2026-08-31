# Desktop-Owned Session Control Design Review Trace

**Status:** passed
**Current review scope:** M4 R0

## Review Surface

Reviewed: `docs/superpowers/specs/2026-08-29-desktop-owned-session-control-design.md`
Review type: spec
Saved because: the first readiness review found material packaging, protocol-version, transport, security, lifecycle, and mission-boundary gaps that affect planning and implementation.

## Material Outcomes

- `BLOCKER` Preferred packaging lacked a proven branch disposition
  - Disposition: fixed
  - Evidence pointer: `Gap Resolutions`, `Packaging and Lifecycle`, prerequisite contained-plugin spike
  - Next action: none
- `BLOCKER` Desktop prerelease version was rejected by the stable-only tested-version pipeline
  - Disposition: fixed
  - Evidence pointer: `Tested native protocol version`, exact file targets, verification, acceptance criterion 16
  - Next action: none
- `MAJOR` Autonomous orchestration authority was missing
  - Disposition: fixed
  - Evidence pointer: `Execution and Authority`
  - Next action: none
- `MAJOR` The attached-CLI internal-fork follow-on phase was omitted
  - Disposition: fixed
  - Evidence pointer: `Mission Phase Boundaries`
  - Next action: none
- `MAJOR` Desktop and CLI plugin lifecycle verification was incomplete
  - Disposition: fixed
  - Evidence pointer: host lifecycle matrix and acceptance criteria 21-25
  - Next action: none
- `MAJOR` Exact WebSocket `/rpc` transport was untested
  - Disposition: fixed
  - Evidence pointer: `Ownership`, verification transport test, acceptance criterion 9
  - Next action: none
- `MAJOR` Live all-tool validation lacked failure-safe ownership and cleanup
  - Disposition: fixed
  - Evidence pointer: ignored live-gate command, run ledger, cleanup and hard-kill recovery contract
  - Next action: none
- `MAJOR` Endpoint security predicates were ambiguous
  - Disposition: fixed
  - Evidence pointer: `Resolution`, `Security validation`, acceptance criteria 7-8
  - Next action: none
- `MAJOR` Exact file targets omitted compiled client/config call sites
  - Disposition: fixed
  - Evidence pointer: expanded `Exact File Targets`
  - Next action: none
- `MAJOR` Unapproved `0.4.0` version was mandatory
  - Disposition: fixed
  - Evidence pointer: hard-coded version removed; same-version cache refresh is proven and required
  - Next action: none
- `MINOR` Manual upgrade and branch-base gates were incomplete
  - Disposition: fixed
  - Evidence pointer: `Prerequisites`, verified five-step `docs/upgrading.md` contract
  - Next action: none
- `MAJOR` Required bug-report workflow referenced deleted CSC commands
  - Disposition: fixed
  - Evidence pointer: `.github/ISSUE_TEMPLATE/bug.yml` target, reader-workflow verification, acceptance criterion 29
  - Next action: none
- `MAJOR` Plan discovery found an unclassified integration support module
  - Disposition: fixed
  - Evidence pointer: exact deletion of `tests/app_server_integration/protocol_support.rs` after moving only the retained native connection seam
  - Next action: none

## Decisions

- The isolated Codex CLI 0.149.1 spike proved legacy plugin-contained execution, exact environment forwarding, cache mode/hash preservation, same-version and version-bump refresh, and removal. The standalone fallback is not triggered.
- Agent Plugins v1 remains excluded because its negative control received none of the required host variables.
- Preserve the compatibility-warning mechanism and extend its canonical tested-version pipeline to the Desktop authority's full SemVer prerelease `0.150.0-alpha.12.2`.
- Implementation reviews run specification compliance, DRY/YAGNI, then code quality.

## Final State

passed

## M4 R0 Design Review

**Status:** passed
**Baseline:** `918c21773f26aa2e1cb74f193fb95ccccf87de7c`; **Reviewed artifact:** specification SHA-256 `12a50111c8f5954348913d2f9a383bcba7eff58e76f3b93c95c72fce16120ab3`; **Review type:** specification compliance; **R6:** passed; **Simplicity R1:** passed; **Quality R2:** passed; **Empty-view amendment R1:** passed; **Findings:** none; **Approval:** approved.
