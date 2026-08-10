# User-Facing CLI Output Implementation Plan

**Status:** approved
**Source:** [Approved user-facing CLI output design](../specs/2026-08-10-user-facing-cli-output-design.md)
**Review:** [Implementation plan review trace](../reviews/2026-08-10-user-facing-cli-output-plan-review.md)
**Next:** [Implementation handoff](../../handoffs/2026-08-10-113356-user-facing-cli-output-implementation.md)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace machine-oriented human CLI receipts with the approved concise typed output without changing lifecycle safety, mutation ordering, wrapper silence, staged-update ownership, active-task semantics, or MCP stdio.

**Architecture:** Add one closed human renderer and one concrete typed diagnostic emitter, then make each existing command producer select a complete bounded outcome at the semantic boundary where its safety evidence is known. Preserve the current operational modules and fixtures; add only the two approved narrow evidence seams for descriptor residue and candidate spawn versus wait.

**Tech Stack:** Rust 1.95.0, Clap 4.6, Tokio 1.53, rmcp 2.2, existing unit/integration fixtures, Bash-based repository verification.

---

## Prerequisites

- Work in a dedicated Codex Session Control implementation worktree whose history contains:
  - approved source commit `3d73ce13eda5cf9052e817617d736faac48ca719`;
  - approved planning source commit `6714c542e1209d194fa7924a42065481780675e7`;
  - merge base `528ef31566048520617929a45ea446ff9af70559`.
- Begin from the committed planning boundary linked through `Next`; verify the plan is `approved`, its review trace is `passed`, and the implementation handoff names `superpowers:executing-plans`.
- Resolve the immutable implementation base from the commit that first added this plan:

  ```bash
  implementation_base_sha=$(git log --diff-filter=A --format=%H -1 -- docs/superpowers/plans/2026-08-10-user-facing-cli-output.md)
  test -n "$implementation_base_sha"
  test "$(git rev-parse HEAD)" = "$implementation_base_sha"
  ```

  Record the exact printed SHA in the implementation task evidence. Recompute it with the same command for later diff/log checks; do not substitute `6714c542e1209d194fa7924a42065481780675e7`, which is only the pre-plan source boundary.
- Do not implement on `main` or `master`. The implementation checkout may be detached or on a feature branch, but it must be the exact dedicated worktree recorded by the implementation task.
- Require an empty `git status --short` before the first edit. Stop if unrelated dirt exists or if the plan/source/review artifacts differ from the committed boundary.
- Use the pinned Rust 1.95 toolchain with `rustfmt` and `clippy`. `./scripts/check.sh` additionally requires Bash, POSIX `sh`, `shellcheck`, `jq`, and `actionlint 1.7.12`.
- Do not change `Cargo.toml` or `Cargo.lock`; the approved design needs no new dependency.
- Preserve the current selected-home, filesystem ownership, systemd-user, Desktop descriptor, release verification, active-task, retry-safety, and MCP catalog fixtures.
- **[MANUAL/CI]** Run `bash scripts/ci/disposable-systemd-user-contract.sh` only in the documented disposable systemd-user environment after the implementation branch is available to CI. Never run ignored `live_normal_home_*` tests directly on a normal development workstation.
- Planning approval is complete. No additional ceremonial approval is required before implementation, but every stop condition below remains binding.

## Acceptance Criteria

| ID | Approved requirement | Measurable implementation proof |
| --- | --- | --- |
| AC1 | Exact root/subcommand help, seven visible human commands, hidden-callable `mcp-server`, global `--verbose`, and native `codex` passthrough. | Task 1 exact help/parse tests plus CI/release help-contract tests pass; Clap parser errors remain exit `2`. |
| AC2 | Exact successful `setup`, `update`, `enable`, `disable`, and `uninstall` blocks and blank-line order. | Tasks 2, 4, and 5 renderer/producer tables pass byte-for-byte assertions. |
| AC3 | Running CLI and Desktop facts are independent; specific guidance replaces duplicates. | Tasks 3-4 table tests include CLI-running with Desktop unavailable and both detected clients. |
| AC4 | Successful stdout is flushed before approved stderr notices; failures and manual gates retain their own ordering. | Tasks 2 and 4 writer-order and notice tests pass; prompt byte oracle remains unchanged. |
| AC5 | Status exposes only four top-level states and four integration states. | Task 6 table-driven typed status/render tests pass. |
| AC6 | CLI/Desktop status is independent and read-only. | Task 6 preserves all no-mutation, unsafe-socket, and descriptor evidence fixtures. |
| AC7 | Status exit matrix is `0` for healthy/disabled and `1` for not-installed/unhealthy. | Task 6 exact renderer/exit table passes in default and verbose modes. |
| AC8 | Every producer table row selects one complete bounded `UserFailure` directly. | Tasks 4-5 producer matrices pass without stage mapping or error-string parsing. |
| AC9 | Safety, rollback, manual-cleanup, verified-release, partial-disable, terminal-uninstall, and cancellation behavior remains distinct. | Tasks 2, 4, and 5 exact variant/render tests and existing mutation/retry fixtures pass. |
| AC10 | Active-task prompt, disclosure, recheck, and goal semantics remain unchanged. | Task 5 exact prompt bytes and active-turn request/mutation evidence pass. |
| AC11 | Default/verbose stdout, default-visible stderr, exit code, and mutation evidence are identical after removing diagnostic lines. | Tasks 4-6 parity assertions pass for every human command. |
| AC12 | Diagnostics are chronological and use `[verbose] <command>[/<phase>]:`. | Task 2 unit tests and Tasks 5-6 subprocess fixtures pass. |
| AC13 | Diagnostics structurally exclude prohibited privacy classes. | Task 2 exact dynamic-field inventory and static-stage table pass; no event or stage operation accepts raw errors, environment/argv maps, task data, PID, timestamp, or telemetry fields. |
| AC14 | Candidate `0`/`1` is propagated; spawn failure is retry-safe; wait/signal/other exit uses exact `UpdateCompletionUnknown`. | Task 5 candidate table and outer/apply ownership tests pass with no second parent prose or result protocol. |
| AC15 | Successful `codex` emits no CSC bytes; failure is exact and safe. | Task 6 bounded native-process and failure-block tests pass, including global verbosity. |
| AC16 | MCP stdout remains JSON-RPC-only and hidden transport behavior remains intact. | Tasks 1 and 6 hidden-callable/catalog/EOF/reaping/isolation tests pass. |
| AC17 | Descriptor evidence is limited to exact final/stage residue and cleanup truth; existing operational safety remains authoritative. | Task 3 publication evidence tests plus Tasks 4-5 producer mappings pass. |
| AC18 | README/Desktop docs use the approved human command and status vocabulary. | Task 7 docs checks pass with every remaining legacy-term match inspected. |
| AC19 | No dependency, background subsystem, protocol, generic output framework, or unrelated domain rewrite is introduced. | Final spec review confirms the diff stays within the File Structure and stop conditions. |
| AC20 | Fresh repository verification passes. | Task 7 records a fresh zero exit from `./scripts/check.sh`; disposable systemd proof remains **[MANUAL/CI]**. |

## File Structure

### Create

- `src/cli_output.rs` — closed success/failure/status values, exact prose composition, channel ownership, and exit codes.
- `src/diagnostics.rs` — one concrete off/stderr/test-record emitter, typed event allowlist, update phases, write-failure disablement, and privacy tests.

### Modify: production

- `src/main.rs` — declare the two modules; preserve MCP-first dispatch; render buffered human results once; flush stdout before success notices; propagate delegated candidate exit `0`/`1`.
- `src/cli.rs` — exact Clap descriptions/help, global verbosity, hidden `mcp-server`, and unchanged native Codex argument boundary.
- `src/install.rs` — remove rendered `LifecycleReceipt`, expose typed command results, retain operational helpers, and replace stage-selected failure prose.
- `src/install/service.rs` — return `RunningClientFacts { cli, desktop }`; never append human prose.
- `src/desktop/descriptor.rs` — return publication-local clean/final/stage cleanup evidence only.
- `src/install/setup.rs` — typed setup success/notices, direct producer failure selection, independent client facts, and typed diagnostics.
- `src/install/enable_disable.rs` — typed enable/disable outcomes, direct safety/cleanup variants, and truthful `PartialDisable`.
- `src/install/uninstall.rs` — typed complete/rollback/manual/terminal results and allowlisted remaining paths.
- `src/install/native.rs` and `src/install/paths.rs` — replace the arbitrary manual-cleanup string with a typed invocation and expose only the existing safe path-quoting helper needed by the renderer.
- `src/install/update.rs` — typed outer/apply outcomes, gate failures, spawn/wait classification, exact completion-unknown result, diagnostics phases, and normal exit propagation.
- `src/install/status.rs` — one typed `StatusResult`, minimum evidence-strength projections at current inspection sites, independent integration classification, and no read-side mutation.
- `src/install/wrapper.rs` — silent successful exec and exact safe failure selection without raw error rendering.

### Modify: focused tests and operational fixtures

- `tests/cli_contract/command_surface.rs` — exact help, visible/hidden commands, global option placement, parser exit `2`, and passthrough.
- `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `tests/workflow_contract.rs`, `tests/cli_contract/release_bundle.rs` — require seven visible help commands while retaining separate hidden-callable MCP proof. These are forced consumers of the approved help contract even though the source spec did not enumerate them.
- `src/desktop/tests/descriptor.rs` and `src/desktop/tests/discovery.rs` — publication residue/cleanup evidence and unchanged successful descriptor bytes.
- `src/install/tests/setup.rs`, `src/install/tests/desktop_start_lifecycle.rs`, and `src/install/tests/normal_home_setup.rs` — setup success/guidance/diagnostics while retaining manifest-last and mutation proof.
- `src/install/tests/enable_disable.rs`, `src/install/tests/desktop_stop_lifecycle.rs`, and `src/install/tests/service_safety.rs` — direct enable/disable variants and unchanged service/caller evidence.
- `src/install/tests/uninstall.rs` and `src/install/tests/failure_retry.rs` — direct rollback/manual/terminal outcomes, normal retry convergence, and removal ordering.
- `src/install/tests/active_turn_gate.rs` and `src/install/tests/update_matrix.rs` — typed gate outcomes, candidate ownership, verbose propagation, and spawn/wait evidence.
- `src/install/tests/status.rs`, `src/install/tests/selected_home_evidence.rs`, and `src/install/tests/codex_wrapper.rs` — typed status, preserved evidence strength/read-only behavior, and wrapper silence/failure.
- `src/install/tests/systemd.rs` — presentation assertions only; preserve disposable live mechanics.
- `tests/app_server_integration/normal_home.rs`, `tests/app_server_integration/live_harness.rs`, and `tests/app_server_integration/cases.rs` — replace raw `completed:` parsing with explicit verbose diagnostic evidence while retaining systemd ordering.
- `tests/mcp_contract.rs` — global-verbosity stdout isolation without weakening catalog, EOF, and reaping coverage.

### Modify: public documentation

- `README.md` — seven human command descriptions; keep MCP tool documentation but remove `mcp-server` from normal interactive commands.
- `docs/desktop.md` — `Codex Desktop integration` and `ready`/`unavailable`/`unhealthy`/`could not verify`.

No other production file is approved. A newly discovered target is allowed only when a concrete compile/test consumer forces it; record that reason before editing.

## Implementation Milestones and Review Placement

1. **Milestone A — typed surface:** Tasks 1-2.
2. **Milestone B — lifecycle truth:** Tasks 3-4.
3. **Intermediate ordered review after Milestone B only:** run focused milestone verification, then spec-compliance review, repair and rerun affected tests, then code-quality review. Do not run reviewer pairs after Tasks 1, 2, or 3.
4. **Milestone C — update/status/process boundaries:** Tasks 5-6.
5. **Milestone D — documentation and final proof:** Task 7.
6. **Final ordered review:** only after fresh full verification; spec compliance first, code quality second. Repair valid findings, rerun affected focused tests and `./scripts/check.sh`, and do not add quota-driven review passes.

## Tasks

### Task 1: Lock the Human CLI Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `tests/cli_contract/command_surface.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `tests/workflow_contract.rs`
- Modify: `tests/cli_contract/release_bundle.rs`

- [ ] **Step 1: Write exact failing CLI/help contracts**

Add these focused tests:

```rust
#[test]
fn root_help_matches_approved_contract_and_hides_mcp_server() {
    let output = command().arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), APPROVED_ROOT_HELP);
    assert!(output.stderr.is_empty());
}

#[test]
fn mcp_server_remains_callable_while_hidden() {
    assert!(Cli::try_parse_from(["csc", "mcp-server"]).is_ok());
    assert!(!APPROVED_ROOT_HELP.contains("mcp-server"));
}

#[test]
fn verbose_placement_and_codex_passthrough_are_exact() {
    let cli = Cli::try_parse_from(["csc", "--verbose", "codex", "--verbose"]).unwrap();
    assert!(cli.verbose);
    let Command::Codex { args } = cli.command else { panic!("expected codex") };
    assert_eq!(args, vec![OsString::from("--verbose")]);
    assert!(Cli::try_parse_from(["csc", "setup", "--verbose"]).unwrap().verbose);
}
```

Put the direct `Cli::try_parse_from` assertion in the existing `src/cli.rs` unit-test module; put subprocess help/exit assertions in `tests/cli_contract/command_surface.rs`. Use byte-for-byte constants from the approved spec for root, setup, codex, and the shared detailed help form. Retain the existing alias/parser-error test and assert exit `2`.

Before RED, rename `native_ci_and_release_binaries_expose_exactly_eight_commands` to `native_ci_and_release_binaries_expose_exactly_seven_visible_commands` and change only that test's expected help list/message to seven. Change `release_asset_rules` to expect seven visible help commands while leaving both workflow YAML command lists unchanged at eight. This makes the test consumers fail first; do not edit workflow producers until Step 3.

- [ ] **Step 2: Run the RED commands**

```bash
cargo test --test cli_contract --locked command_surface
cargo test --bin codex-session-control --locked cli::tests::verbose_placement_and_codex_passthrough_are_exact -- --exact
cargo test --test workflow_contract --locked native_ci_and_release_binaries_expose_exactly_seven_visible_commands
cargo test --test cli_contract --locked release_bundle::release_asset_rules
```

Expected RED: the exact help/parser tests fail because `mcp-server` is visible and `Cli` has no `verbose`; the two consumer tests each execute one test and fail because workflow/release help producers still expose eight visible commands.

- [ ] **Step 3: Implement the minimal Clap contract**

Use this public shape and no new public controls:

```rust
#[derive(Debug, Parser)]
#[command(about = "Manage Codex Session Control")]
pub(crate) struct Cli {
    #[arg(long, global = true, help = "Show diagnostic details")]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: Command,
}
```

Set each visible variant's `about` to the approved one-line description, add the exact setup launcher help/value name, and add `#[command(hide = true)]` to `McpServer`. Preserve `Codex { args: Vec<OsString> }` with `trailing_var_arg` and `allow_hyphen_values`; arguments after `codex` remain native even when named `--verbose`.

Change only the four forced CI/release consumers from eight visible help commands to seven. Keep all internal matrices that intentionally enumerate eight callable variants.

- [ ] **Step 4: Run the GREEN commands**

Run all four Step 2 commands again.

Expected GREEN: exact help snapshots pass, `mcp-server` is absent from root help but parses, parser errors remain exit `2`, and both CI/release contracts accept seven visible commands.

- [ ] **Step 5: Refactor and recheck**

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo test --test cli_contract --locked command_surface
```

Expected: no duplicate help-copy source outside the exact test constants/Clap attributes and all command-surface tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs tests/cli_contract/command_surface.rs .github/workflows/ci.yml .github/workflows/release.yml tests/workflow_contract.rs tests/cli_contract/release_bundle.rs
git commit -m "feat(cli): define the human command surface"
```

### Task 2: Add the Closed Renderer and Concrete Diagnostics

**Files:**
- Create: `src/cli_output.rs`
- Create: `src/diagnostics.rs`
- Modify: `src/main.rs` (module declarations and unit-test writer seam only)

- [ ] **Step 1: Write failing renderer, writer, and diagnostic tests**

Create unit tests with these exact names:

```rust
#[test]
fn update_completion_unknown_is_stderr_only_exit_one_without_retry() {
    let rendered = UserFailure::UpdateCompletionUnknown.render();
    assert!(rendered.stdout.is_empty());
    assert_eq!(rendered.stderr, UPDATE_COMPLETION_UNKNOWN);
    assert_eq!(rendered.exit_code, 1);
    assert!(!rendered.stderr.contains("Try again"));
}

#[test]
fn every_materially_distinct_failure_block_is_exact() {
    for case in failure_render_cases() {
        assert_eq!(case.failure.render(), case.expected);
        assert!(case.expected.stdout.is_empty());
        assert_eq!(case.expected.exit_code, 1);
        assert!(case.expected.stderr.ends_with('\n'));
    }
}

#[test]
fn every_materially_distinct_success_and_notice_block_is_exact() {
    for case in success_render_cases() {
        assert_eq!(case.success.render(), case.expected);
        assert_eq!(case.expected.exit_code, case.expected_exit_code);
        assert!(case.expected.stdout.ends_with('\n'));
    }
}

#[test]
fn status_renderer_and_exit_matrix_are_exact() {
    for (state, exit_code) in STATUS_EXIT_CASES {
        assert_eq!(render_status(state).exit_code, exit_code);
    }
}

#[test]
fn success_writer_flushes_stdout_before_default_visible_stderr() {
    assert_eq!(record_write_order(success_with_notice()), ["stdout", "flush-stdout", "stderr", "flush-stderr"]);
}

#[test]
fn writer_failure_exits_one_without_a_second_friendly_or_raw_error() {
    let (exit, stderr_attempts) = run_with_failing_stdout(success_with_notice());
    assert_eq!(exit, 1);
    assert_eq!(stderr_attempts, 0);
}

#[test]
fn prefixes_and_update_phases_are_exact() {
    assert_eq!(recorded_update_lines(), APPROVED_DIAGNOSTIC_LINES);
}

#[test]
fn every_dynamic_diagnostic_field_is_exactly_rendered() {
    assert_eq!(rendered_dynamic_fields_and_causes(), APPROVED_DIAGNOSTIC_LINES);
}

#[test]
fn first_write_failure_disables_later_output_without_changing_result() {
    assert_eq!(emit_after_first_failure(), (0, ORIGINAL_RESULT));
}
```

Build both case-table functions from typed values and independent literal `RenderedCli` expectations copied byte-for-byte from the approved spec; expected values must not call production render/composition helpers. Use `Vec` rather than `const`/`static` storage because samples contain owned paths and strings. Include final newlines and blank lines in the literals. `failure_render_cases()` contains:

- `ordinary_literal_cases()` contains one row for every `OrdinaryFailure` variant listed in Step 3, with its complete expected stderr bytes;
- one row for every `RollbackPrimary` using one managed path, plus one representative multiple-path row to prove list composition;
- one row for every `StopThenRetry` and `IndependentTerminal` variant;
- plugin and marketplace `ManualCleanup` rows with and without the optional validated executable;
- `VerifiedRelease` with fixed allowlisted test URLs;
- `InteractiveTerminal`, pathless and exact-path `PartialDisable`, one-path and multiple-path `TerminalPartialUninstall`, `Cancellation`, and `WrapperUnavailable`;
- the separate exact `UpdateCompletionUnknown` assertion above, which remains the explicit no-retry oracle.

`success_render_cases()` contains one independent literal row for every materially distinct block, without a Cartesian product: setup primary, running-CLI, running-Desktop, generic Desktop restart, Desktop-unavailable warning, compatibility warning, and PATH notice; applied/already-current update with enabled/disabled service and the Desktop-change follow-up; enable primary plus its CLI/Desktop guidance blocks; disable and uninstall with and without Desktop removal; all four top-level status forms; every distinct status problem/action block and identical-action grouping; and the compatibility, Desktop-discovery, and PATH notice blocks on their required channel. Add one representative multi-block success per command to prove one-blank-line composition. Group cases only when the complete stdout, stderr, and exit bytes are identical. Every failure case asserts empty stdout, exact stderr, final newline, and exit `1`; every success/status case asserts exact channels and its approved `0`/`1` exit.

Define the exact completion-unknown constant from the approved spec, including final newline. Assert empty stdout, exact stderr, exit `1`, and absence of `Try again`.

- [ ] **Step 2: Run the RED commands**

```bash
cargo test --bin codex-session-control --locked cli_output::tests
cargo test --bin codex-session-control --locked diagnostics::tests
cargo test --bin codex-session-control --locked tests::success_writer_flushes_stdout_before_default_visible_stderr -- --exact
cargo test --bin codex-session-control --locked tests::writer_failure_exits_one_without_a_second_friendly_or_raw_error -- --exact
```

Expected RED: modules/types/tests do not exist and main has no buffered stdout-first writer.

- [ ] **Step 3: Implement the minimal closed types**

Use this boundary; fields remain private and constructors enforce valid combinations:

```rust
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RenderedCli {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: u8,
}

pub(crate) enum UserSuccess {
    Setup(SetupSuccess),
    Update(UpdateSuccess),
    Enable(EnableSuccess),
    Disable(DisableSuccess),
    Uninstall(UninstallSuccess),
    Status(StatusResult),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunningClientFacts {
    pub(crate) cli: bool,
    pub(crate) desktop: bool,
}

pub(crate) enum DesktopAvailability {
    Available,
    Unavailable,
    CouldNotVerify,
    SetupRequired,
}

pub(crate) enum UserNotice {
    Compatibility { codex: semver::Version, product: semver::Version },
    DesktopLauncherUnavailable,
    LocalBinMissingFromPath { local_bin: PathBuf },
}

pub(crate) struct SetupSuccess {
    version: semver::Version,
    running: RunningClientFacts,
    desktop: DesktopAvailability,
    desktop_changed: bool,
    notices: Vec<UserNotice>,
}

pub(crate) enum UpdateState { Applied, AlreadyCurrent }
pub(crate) struct UpdateSuccess {
    state: UpdateState,
    version: semver::Version,
    service_enabled: bool,
    desktop_changed: bool,
    notices: Vec<UserNotice>,
}

pub(crate) struct EnableSuccess {
    running: RunningClientFacts,
    desktop: DesktopAvailability,
    desktop_changed: bool,
    notices: Vec<UserNotice>,
}

pub(crate) struct DisableSuccess { desktop_removed: bool }
pub(crate) struct UninstallSuccess { desktop_removed: bool }

pub(crate) enum UserFailure {
    Ordinary(OrdinaryFailure),
    RollbackIncomplete(RollbackIncomplete),
    StopThenRetry(StopThenRetry),
    ManualCleanup(ManualCleanup),
    VerifiedRelease(VerifiedReleaseRecovery),
    IndependentTerminal(IndependentTerminal),
    InteractiveTerminal,
    PartialDisable(PartialDisable),
    TerminalPartialUninstall(TerminalPartialUninstall),
    UpdateCompletionUnknown,
    Cancellation,
    WrapperUnavailable,
}

pub(crate) enum StatusState { Healthy, Disabled, NotInstalled, Unhealthy }
pub(crate) enum IntegrationState { Ready, Unavailable, Unhealthy, CouldNotVerify }

pub(crate) enum ServiceSummary {
    RunningAutomatic,
    StoppedAutomaticOff,
    StoppedUnexpectedAutomaticOn,
    CouldNotVerify,
}

pub(crate) struct StatusResult {
    state: StatusState,
    version: Option<semver::Version>,
    service: Option<ServiceSummary>,
    cli: IntegrationState,
    desktop: IntegrationState,
    problems: Vec<StatusProblem>,
}

pub(crate) enum OrdinaryFailure {
    SetupUnsafeTerminalRetry,
    SetupUnexpectedRetry,
    SetupInstalledStateCheckStatus,
    SetupInstallationFilesRetryUpdate,
    SetupInstalledStateRepair { binary: PathBuf },
    SetupCliIntegrationRetry,
    SetupCliIntegrationCheckStatus,
    SetupInstallationFilesRetry,
    SetupServiceConfigurationRetry,
    SetupDesktopIntegrationRetry,
    SetupDesktopIntegrationCheckStatus,
    SetupServiceStartRetry,
    SetupServiceStateRetryUpdate,
    UpdateUnexpectedRetry,
    UpdateInstalledStateCheckStatus,
    UpdateReleaseRetry,
    UpdateChecksumRetry,
    UpdateCliIntegrationRetry,
    UpdateCliIntegrationCheckStatus,
    UpdateServiceConfigurationRetry,
    UpdateServiceStateCheckStatus,
    UpdateActiveTasksRetry,
    UpdateInstallationFilesRetry,
    UpdateDesktopIntegrationRetry,
    UpdateDesktopIntegrationCheckStatus,
    UpdateServiceConfigurationLogs,
    UpdateServiceStartLogs,
    UpdateServiceStateLogs,
    UpdateInstalledStatePostMutationCheckStatus,
    EnableUnexpectedRetry,
    EnableInstalledStateRepairSetup,
    EnableServiceConfigurationRepairSetup,
    EnableDesktopIntegrationCheckStatus,
    EnableDesktopIntegrationRetry,
    EnableServiceStartRetry,
    EnableServiceStateRetry,
    EnableUnexpectedCheckStatus,
    DisableUnexpectedRetry,
    DisableServiceStopRetry,
    DisableUnexpectedCheckStatus,
    UninstallUnexpectedRetry,
    UninstallServiceStopRetry,
}

pub(crate) enum StopThenRetry {
    UpdateServiceStateDisableUpdateEnable,
    EnableServiceStartStopThenEnable,
    EnableServiceStateStopThenEnable,
    DisableUnsafeStopThenDisable,
    DisableServiceStopThenDisable,
    DisableServiceStateStopThenDisable,
    UninstallUnsafeStopThenUninstall,
    UninstallServiceStateStopThenUninstall,
}

pub(crate) enum IndependentTerminal { Update, Disable, Uninstall }
pub(crate) enum NativeCleanupCommand { RemovePlugin, RemoveMarketplace }
```

Each `OrdinaryFailure` variant above encodes one complete command headline + problem + recovery combination. Do not replace it with separately combinable problem/recovery fields. `StopThenRetry`, `IndependentTerminal`, `NativeCleanupCommand`, rollback-primary, status-problem, and special-result enums likewise use variant-per-complete approved form. Only typed version/path/URL/client/status fields may be interpolated.

Use one nonempty managed-path value:

```rust
pub(crate) struct ManagedPaths {
    first: PathBuf,
    rest: Vec<PathBuf>,
}

pub(crate) enum RollbackPrimary {
    SetupDesktopRetry,
    SetupServiceConfigurationRetry,
    SetupServiceStartRetry,
    SetupServiceStateRetryUpdate,
    UpdateDesktopRetry,
    EnableDesktopRetry,
    EnableServiceStateCheckStatus,
    UninstallInstalledStateCheckStatus,
    UninstallDesktopCheckStatus,
    UninstallCleanupRetry,
}

pub(crate) struct RollbackIncomplete {
    primary: RollbackPrimary,
    paths: ManagedPaths,
}

pub(crate) struct ManualCleanup {
    command: NativeCleanupCommand,
    codex_home: PathBuf,
    codex_executable: Option<PathBuf>,
}

pub(crate) struct VerifiedReleaseRecovery {
    release_url: String,
    checksums_url: String,
}

pub(crate) struct PartialDisable {
    managed_path: Option<PathBuf>,
}

pub(crate) struct TerminalPartialUninstall {
    remaining: ManagedPaths,
}

pub(crate) enum StatusProblem {
    InvocationContextCouldNotBeVerified,
    InstalledStateCouldNotBeVerified,
    NativeRegistrationFault,
    NativeRegistrationCouldNotBeVerified,
    ProjectionFault,
    ProjectionCouldNotBeVerified,
    ServiceEnablementCouldNotBeVerified,
    ServiceConfiguredButStopped,
    ServiceActivityCouldNotBeVerified,
    SocketMissing,
    SocketUnsafe,
    AppServerUnavailable,
    AppServerCouldNotBeVerified,
    DesktopDescriptorFault,
    DesktopCouldNotBeVerified,
}
```

Construct managed paths only from validated product-owned paths. Construct `ManualCleanup` only after selected-home validation and native executable selection; render it with the existing `install::paths::shell_quote_path`, re-exported crate-wide without changing its implementation. Construct verified-release URLs only from the existing typed release metadata/endpoints. Do not accept arbitrary display strings, raw `ControllerError`, stage names, or pre-rendered recovery commands.

Implement `UserSuccess::render` and `UserFailure::render` as exhaustive direct matches. Put exact block composition in small private functions, with one blank line between nonempty blocks. No output AST, builder family, renderer trait, generic command report, localization table, or serialization.

- [ ] **Step 4: Implement the concrete diagnostics emitter**

Use one concrete type with private sinks:

```rust
pub(crate) struct Diagnostics {
    command: DiagnosticCommand,
    phase: Option<UpdatePhase>,
    sink: DiagnosticSink,
}

enum DiagnosticSink {
    Off,
    Stderr(std::io::Stderr),
    #[cfg(test)]
    Record(Vec<String>),
    #[cfg(test)]
    FailOnce,
}

pub(crate) enum DiagnosticCommand { Setup, Update, Status, Enable, Disable, Uninstall, Codex }
pub(crate) enum UpdatePhase { Outer, Apply }
pub(crate) enum DiagnosticCause { Unexpected, Validation, ReleaseDownload, Checksum, ServiceConfiguration, ServiceStart, ServiceStop, ServiceState, CliIntegration, DesktopIntegration, ActiveTasks, Cleanup }
```

Command modules own closed stage enums and return one compile-time `&'static str` diagnostic name from each enum. `Diagnostics::completed` and `Diagnostics::failed` accept only that static label; typed special events remain limited to dynamic evidence and non-stage semantics. Maintain exact rendering coverage for every dynamic event field and diagnostic cause. The emitter never accepts raw error/display text, runtime stage strings, arbitrary argv/environment/configuration/task data, PIDs, timestamps, or telemetry identifiers. A failed write, including the test-only `FailOnce` sink, replaces the sink with `Off`; `emit` and `flush` are best-effort and cannot alter results, mutations, or exit codes.

- [ ] **Step 5: Add the writer seam**

Add a private writer helper used later by `main`:

```rust
fn write_rendered(
    rendered: &RenderedCli,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<()> {
    stdout.write_all(rendered.stdout.as_bytes())?;
    stdout.flush()?;
    stderr.write_all(rendered.stderr.as_bytes())?;
    stderr.flush()
}

enum ProcessOutcome {
    Render(RenderedCli),
    Exit(u8),
}
```

On any writer or flush error, the top-level process exits `1` immediately and makes no second friendly/raw-error write attempt to either channel. `ProcessOutcome::Exit` is also the normal staged-candidate `0`/`1` propagation seam. MCP remains a separate branch before `ProcessOutcome`. The production human wiring stays for Tasks 4-6; this task proves the complete writer/error policy without adapting raw receipts.

- [ ] **Step 6: Run GREEN and refactor checks**

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked cli_output::tests
cargo test --bin codex-session-control --locked diagnostics::tests
cargo test --bin codex-session-control --locked tests::success_writer_flushes_stdout_before_default_visible_stderr -- --exact
cargo test --bin codex-session-control --locked tests::writer_failure_exits_one_without_a_second_friendly_or_raw_error -- --exact
```

Expected GREEN: the complete independent renderer tables, exact exit codes, and completion-unknown no-retry oracle pass; diagnostic prefixes/privacy/write-failure behavior passes; and the exact writer test reports one passed test with stdout flush before stderr write. Do not run warnings-denied Clippy in this task because the closed output surface is intentionally not fully production-reachable until Task 6; run it there and again at final verification.

- [ ] **Step 7: Commit**

```bash
git add src/cli_output.rs src/diagnostics.rs src/main.rs
git commit -m "feat(cli): add typed output and diagnostics"
```

### Task 3: Preserve Only the Shared Evidence Needed for Truthful Results

**Files:**
- Modify: `src/install/service.rs`
- Modify: `src/desktop/descriptor.rs`
- Modify: `src/install.rs`
- Modify: `src/install/setup.rs`
- Modify: `src/install/enable_disable.rs`
- Modify: `src/install/update.rs`
- Modify: `src/desktop/tests/descriptor.rs`
- Modify: `src/desktop/tests/discovery.rs`
- Modify: `src/install/tests/desktop_start_lifecycle.rs`

- [ ] **Step 1: Write failing independent-client and descriptor-evidence tests**

Add:

```rust
#[test]
fn running_client_facts_are_independent_of_desktop_availability() {
    let euid = rustix::process::geteuid().as_raw();
    let facts = detect_running_unattached_clients_from_snapshot(
        euid,
        [(euid, b"/usr/bin/codex\0resume\0".as_slice())],
    );
    assert_eq!(facts, RunningClientFacts { cli: true, desktop: false });
}

#[test]
fn descriptor_publication_reports_only_exact_managed_residue() {
    assert_eq!(failure_before_stage.residue, None);
    assert_eq!(unverified_stage_cleanup.residue, Some(Stage(exact_stage_path)));
    assert_eq!(post_rename_failure.residue, Some(Final(exact_final_path)));
}
```

Keep existing successful publication bytes and non-following/identity/mode tests unchanged in substance.

- [ ] **Step 2: Run the RED commands**

```bash
cargo test --bin codex-session-control --locked desktop::tests::descriptor::descriptor_publication_reports_only_exact_managed_residue -- --exact
cargo test --bin codex-session-control --locked install::tests::desktop_start_lifecycle
cargo test --bin codex-session-control --locked desktop::tests::discovery
```

Expected RED: `publish_descriptor` exposes only `ControllerError`; cleanup truth/path is discarded; current client detection returns human strings and setup/enable can suppress CLI detection behind Desktop availability.

- [ ] **Step 3: Return typed client facts**

Replace `append_unattached_client_guidance` and string labels by returning the shared renderer fact:

```rust
pub(super) fn detect_running_unattached_clients(
    source: &ClientProcessSource,
    euid: u32,
) -> RunningClientFacts;

pub(super) fn detect_running_unattached_clients_from_snapshot<'a>(
    euid: u32,
    snapshot: impl IntoIterator<Item = (u32, &'a [u8])>,
) -> RunningClientFacts;
```

Keep the current `/proc`/snapshot mechanics and matching rules. Always collect both facts; command renderers decide guidance.

- [ ] **Step 4: Return publication-local cleanup evidence**

Use the approved narrow seam:

```rust
pub(crate) enum DescriptorPublicationResidue {
    Stage(PathBuf),
    Final(PathBuf),
}

pub(crate) struct DescriptorPublicationFailure {
    pub(crate) source: ControllerError,
    pub(crate) residue: Option<DescriptorPublicationResidue>,
}

pub(crate) fn publish_descriptor(
    identity: &DesktopAttachmentIdentity,
    expected: &[u8],
) -> Result<bool, DescriptorPublicationFailure>;

#[cfg(test)]
enum DescriptorPublicationTestPoint {
    BeforeStage,
    AfterStage { cleanup_unverified: bool },
    AfterRename,
}
```

`None` is allowed only when no stage was created or stage cleanup and required verification/sync prove no managed residue. Failed/unverified stage cleanup carries only the exact stage path. Any failure after successful rename carries only the final path. Do not return a vector, mutation-state enum, transaction abstraction, PID diagnostic, or recovery string.

Keep the test point private to `desktop::descriptor`; production calls the same internal implementation with no test point. Do not add descriptor flags to `LifecycleTarget`. The direct test checks that the returned stage path is under the validated descriptor parent and names the stage file that actually remains; it must not assert or emit a hard-coded PID.

Change `cleanup_changed_descriptor_after_start_failure` to return only exact clean versus one known descriptor residue/unverified result; callers in Task 4 decide whether to promote the already-selected primary failure to `RollbackIncomplete`.

- [ ] **Step 5: Adapt all existing consumers without changing user output yet**

Update setup, enable, and update to compile against `DescriptorPublicationFailure` and `RunningClientFacts`. In this commit, pass `failure.source` through the current internal `ControllerError` path without stringifying or inspecting it; Tasks 4-5 replace those exact call-site mappings before the first milestone review. This is a compile-preserving internal step inside the same approved plan, not a compatibility layer or public result.

- [ ] **Step 6: Run GREEN and refactor checks**

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked desktop::tests::descriptor
cargo test --bin codex-session-control --locked desktop::tests::discovery
cargo test --bin codex-session-control --locked install::tests::desktop_start_lifecycle
```

Expected GREEN: independent client facts pass; all descriptor safety tests still pass; clean/final/stage evidence is exact; no renderer copy or generic transaction state appears in these modules.

- [ ] **Step 7: Commit**

```bash
git add src/install/service.rs src/desktop/descriptor.rs src/install.rs src/install/setup.rs src/install/enable_disable.rs src/install/update.rs src/desktop/tests/descriptor.rs src/desktop/tests/discovery.rs src/install/tests/desktop_start_lifecycle.rs
git commit -m "refactor(lifecycle): preserve client and descriptor evidence"
```

### Task 4: Migrate Lifecycle Producers in Three Atomic TDD Slices

**Files:**
- Modify: `src/main.rs`
- Modify: `src/install.rs`
- Modify: `src/install/setup.rs`
- Modify: `src/install/enable_disable.rs`
- Modify: `src/install/uninstall.rs`
- Modify: `src/install/native.rs`
- Modify: `src/install/paths.rs`
- Modify: `src/install/tests/setup.rs`
- Modify: `src/install/tests/desktop_start_lifecycle.rs`
- Modify: `src/install/tests/normal_home_setup.rs`
- Modify: `src/install/tests/enable_disable.rs`
- Modify: `src/install/tests/desktop_stop_lifecycle.rs`
- Modify: `src/install/tests/service_safety.rs`
- Modify: `src/install/tests/uninstall.rs`
- Modify: `src/install/tests/failure_retry.rs`

Each slice below must start RED, end GREEN, and commit before the next slice. Keep one review checkpoint after all three slices; do not add reviewer pairs inside them.

#### Slice 4A: Setup and the First Production Writer

- [ ] **Step 1: Write failing setup output, producer, parity, and privacy tests**

Add a table-driven `setup_guidance_precedence_is_exact` covering CLI running with Desktop unavailable, Desktop running, both running, changed-but-not-running Desktop, no duplicate guidance, and rejection of `changed + unavailable/could-not-verify`.

Add `setup_pure_failure_mappings_are_exact` for invocation, reconciliation-error class, and descriptor-residue mappings. Assert the remaining typed results in the existing setup, Desktop lifecycle, and retry tests that already own their filesystem/systemd/mutation evidence. Add `setup_default_and_verbose_are_behaviorally_identical`: run fresh equivalent fixtures with diagnostics off/on, remove only complete stderr lines beginning `[verbose] `, then compare stdout, remaining stderr, exit code, filesystem/native/systemctl markers, and manifest-last evidence.

Before production changes, extend the exhaustive diagnostic constructor inventory with every planned setup event; the privacy test must fail to compile until those typed constructors exist.

- [ ] **Step 2: Run setup RED**

```bash
cargo test --bin codex-session-control --locked install::tests::setup
cargo test --bin codex-session-control --locked install::tests::desktop_start_lifecycle
cargo test --bin codex-session-control --locked install::tests::normal_home_setup
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
```

Expected RED: typed setup outcomes/events are missing; current receipt/PATH/warning order is wrong; CLI facts are Desktop-gated; producer and parity assertions fail while existing mutation evidence remains available.

- [ ] **Step 3: Implement setup and the first human writer**

Replace `SetupReport`, arbitrary `PreflightFailure { cause, recovery }`, and stage-selected prose. Map every setup producer directly at its semantic call site, never via stage or displayed error. Use clean versus final/stage/old-path evidence to choose ordinary versus rollback-incomplete. Return `Result<UserSuccess, UserFailure>` and route it through `ProcessOutcome::Render`; on write/flush error exit `1` without a second friendly/raw write. Keep MCP separate and leave update/status arms on their existing internal paths until their tasks.

- [ ] **Step 4: Run setup GREEN/refactor and commit**

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked install::tests::setup
cargo test --bin codex-session-control --locked install::tests::desktop_start_lifecycle
cargo test --bin codex-session-control --locked install::tests::normal_home_setup
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
git add src/main.rs src/install.rs src/install/setup.rs src/install/tests/setup.rs src/install/tests/desktop_start_lifecycle.rs src/install/tests/normal_home_setup.rs src/cli_output.rs src/diagnostics.rs
git commit -m "feat(setup): render bounded setup outcomes"
```

Expected GREEN: setup copy/guidance/producer/parity/privacy tests and operational fixtures pass. Do not add temporary dead-code allowances or fake constructions for output variants that become production-reachable in later slices; the first warnings-denied Clippy gate is Task 6 after all human commands and `ProcessOutcome::Exit` are wired.

#### Slice 4B: Enable and Disable

- [ ] **Step 5: Write and run failing enable/disable producer, guidance, parity, and privacy tests**

Add `enable_disable_pure_failure_mappings_are_exact`, `enable_guidance_precedence_is_exact`, `enable_default_and_verbose_are_behaviorally_identical`, and `disable_default_and_verbose_are_behaviorally_identical`. Keep publication-residue and cleanup-state selection in the pure table; assert caller independence, service stop/state, pathless/exact-path partial disable, and post-cleanup outcomes in their existing operational tests. Extend exact diagnostic coverage before implementation.

```bash
cargo test --bin codex-session-control --locked install::tests::enable_disable
cargo test --bin codex-session-control --locked install::tests::desktop_stop_lifecycle
cargo test --bin codex-session-control --locked install::tests::service_safety
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
```

Expected RED: the new direct variants/guidance/parity/events are absent; existing service/caller/descriptor evidence still records the required truth.

- [ ] **Step 6: Implement enable/disable, run GREEN/refactor, and commit**

Enable selects the primary service-start versus service-state result before cleanup: no new descriptor -> `StopThenRetry`; changed descriptor with proven clean cleanup -> ordinary retry; remaining/unverified cleanup -> rollback-incomplete/check-status with exact path. Disable maps current caller/service evidence before mutation to `IndependentTerminal` or `StopThenRetry`; after authoritative stop, cleanup failure is truthful `PartialDisable`.

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked install::tests::enable_disable
cargo test --bin codex-session-control --locked install::tests::desktop_stop_lifecycle
cargo test --bin codex-session-control --locked install::tests::service_safety
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
git add src/main.rs src/install.rs src/install/enable_disable.rs src/install/tests/enable_disable.rs src/install/tests/desktop_stop_lifecycle.rs src/install/tests/service_safety.rs src/install/tests/failure_retry.rs src/cli_output.rs src/diagnostics.rs
git commit -m "feat(lifecycle): render enable and disable outcomes"
```

Expected GREEN: exact variants/output/parity pass and existing service safety, caller independence, descriptor ordering, cleanup, and retry evidence remains green in this slice before its commit.

#### Slice 4C: Uninstall

- [ ] **Step 7: Write and run failing uninstall producer, parity, manual-command, and privacy tests**

Add `uninstall_default_and_verbose_are_behaviorally_identical`; extend the existing service-first, retry, native-cleanup, filesystem-order, and terminal-partial tests with their exact typed outcomes. Keep the existing manual cleanup test covering validated Codex home, optional validated executable, and exact shell quoting. Cover rollback while identity survives, manifest rollback, and terminal partial uninstall with exact remaining paths/no rerun. Extend exact diagnostic coverage before implementation.

```bash
cargo test --bin codex-session-control --locked install::tests::uninstall
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
```

Expected RED: current manual cleanup is an arbitrary string; uninstall receipt/progress cannot express direct rollback/manual/terminal variants or parity.

- [ ] **Step 8: Implement uninstall, run GREEN/refactor, and commit**

Preserve service-first and manifest-last order. Make `shell_quote_path` crate-visible through an `install` re-export without changing quoting behavior. Replace `manual_native_removal() -> String` with typed command/home/executable data. Map exact rollback paths while identity remains; after manifest removal, data-root/binary failure is terminal partial with no retry.

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked install::tests::uninstall
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
git add src/main.rs src/install.rs src/install/uninstall.rs src/install/native.rs src/install/paths.rs src/install/tests/uninstall.rs src/install/tests/failure_retry.rs src/cli_output.rs src/diagnostics.rs
git commit -m "feat(uninstall): render terminal cleanup outcomes"
```

Expected GREEN: exact uninstall output/variants/parity/manual quoting pass and service-first, identity survival, retry convergence, manifest-last, and terminal no-rerun fixtures remain green.

- [ ] **Step 9: Run the single intermediate ordered review checkpoint**

Freshly rerun all Milestone A-B surfaces before review:

```bash
cargo test --test cli_contract --locked command_surface
cargo test --bin codex-session-control --locked cli::tests::verbose_placement_and_codex_passthrough_are_exact -- --exact
cargo test --bin codex-session-control --locked cli_output::tests
cargo test --bin codex-session-control --locked diagnostics::tests
cargo test --bin codex-session-control --locked tests::success_writer_flushes_stdout_before_default_visible_stderr -- --exact
cargo test --bin codex-session-control --locked tests::writer_failure_exits_one_without_a_second_friendly_or_raw_error -- --exact
cargo test --bin codex-session-control --locked desktop::tests::descriptor
cargo test --bin codex-session-control --locked desktop::tests::discovery
cargo test --bin codex-session-control --locked install::tests::setup
cargo test --bin codex-session-control --locked install::tests::enable_disable
cargo test --bin codex-session-control --locked install::tests::uninstall
cargo test --bin codex-session-control --locked install::tests::failure_retry
implementation_base_sha=$(git log --diff-filter=A --format=%H -1 -- docs/superpowers/plans/2026-08-10-user-facing-cli-output.md)
git diff --check "$implementation_base_sha"..HEAD
```

Request one spec-compliance review of Milestones A-B. Repair valid findings, commit them with the owning scope, rerun affected commands, and require empty `git status --short`. Only then request one code-quality review of the same milestone diff; handle valid repairs the same way and require another empty status. Do not run another review pair before final verification.

### Task 5: Preserve Update Gate and Candidate Ownership

**Files:**
- Modify: `src/main.rs`
- Modify: `src/install.rs`
- Modify: `src/install/update.rs`
- Modify: `src/install/tests/active_turn_gate.rs`
- Modify: `src/install/tests/update_matrix.rs`
- Modify: `src/install/tests/failure_retry.rs`
- Modify: `src/install/tests/systemd.rs`
- Modify: `tests/app_server_integration/normal_home.rs`
- Modify: `tests/app_server_integration/live_harness.rs`
- Modify: `tests/app_server_integration/cases.rs`

- [ ] **Step 1: Write the failing gate classification tests**

Use:

```rust
enum ActiveTurnGateFailure {
    Inspection,
    InteractiveRequired,
    Cancelled,
}
```

Convert existing gate tests to assert:

- initial/final list failure -> ordinary active-tasks/check failure with retry update;
- noninteractive terminal or prompt write/read failure -> `InteractiveTerminal`;
- non-yes response -> `Cancellation`;
- every failure leaves binary/manifest/reload/restart evidence unchanged.

Keep `expected_prompt()` byte-for-byte, including the final `[y/N]` without a newline. Keep all disclosed IDs, reapproval for new IDs, no thread interrupt, and goal semantics.

- [ ] **Step 2: Write the failing candidate spawn/wait table**

Add:

```rust
enum CandidateApplyResult {
    Exit0,
    Exit1,
    SpawnFailed,
    CompletionUnknown,
}

#[cfg(test)]
enum CandidateWaitHook {
    Real,
    FailAfterSuccessfulSpawn,
}

#[test]
fn candidate_wait_classifies_error_signal_and_normal_exit() {
    use std::os::unix::process::ExitStatusExt;

    assert_eq!(classify_candidate_wait(Ok(ExitStatus::from_raw(0))), Exit0);
    assert_eq!(classify_candidate_wait(Ok(ExitStatus::from_raw(1 << 8))), Exit1);
    assert_eq!(
        classify_candidate_wait(Err(io::Error::other("injected wait failure"))),
        CompletionUnknown
    );
    assert_eq!(classify_candidate_wait(Ok(ExitStatus::from_raw(9))), CompletionUnknown);
    assert_eq!(
        classify_candidate_wait(Ok(ExitStatus::from_raw(2 << 8))),
        CompletionUnknown
    );
}
```

Use a private pure `classify_candidate_wait(Result<ExitStatus, io::Error>)` for classification unit coverage, then bind it to the real production path with a private `run_candidate_apply_with_wait_hook`; production always passes `Real`, while the test-only hook returns an injected wait error only after `.spawn()` succeeds. Add `candidate_apply_routes_spawn_and_post_spawn_completion` for nonexistent executable/spawn failure, injected post-spawn wait failure, real exits `0`/`1`, signal, and another exit. Add `outer_candidate_ownership_is_exact` to prove outer/apply prefixes, flush before spawn, one candidate-owned friendly result, and no parent prose for `0`/`1`. Assert verbose argv is `["--verbose", "update"]` and the private marker remains `1`. Do not add a process trait/backend.

- [ ] **Step 2A: Write the failing exhaustive update producer and parity tables**

Add `update_producer_boundaries_select_complete_failures` before implementation. Enumerate every approved Update producer row and group only identical complete variants: unexpected/retry; installed-state/check-status; release/checksum/candidate; `UpdateCliIntegrationRetry` versus `UpdateCliIntegrationCheckStatus`; unit/service/restart; active tasks; spawn; completion-unknown; writes/projection; Desktop clean versus residue; service log recoveries; and final-manifest post-mutation check-status/no retry.

Add `update_default_and_verbose_are_behaviorally_identical` using equivalent fresh update fixtures. Remove only complete `[verbose] ` stderr lines, then compare stdout, remaining stderr, exit code, downloaded/candidate/manifest files, native/systemctl markers, descriptor state, active-task requests, and retry evidence. Extend the diagnostic inventory with every outer/apply event before production implementation.

- [ ] **Step 3: Run the RED commands**

```bash
cargo test --bin codex-session-control --locked install::tests::active_turn_gate::every_gate_failure_keeps_installed_state_unchanged -- --exact
cargo test --bin codex-session-control --locked install::tests::update_matrix::candidate_wait_classifies_error_signal_and_normal_exit -- --exact
cargo test --bin codex-session-control --locked install::tests::update_matrix::candidate_apply_routes_spawn_and_post_spawn_completion -- --exact
cargo test --bin codex-session-control --locked install::tests::update_matrix::outer_candidate_ownership_is_exact -- --exact
cargo test --bin codex-session-control --locked install::tests::update_matrix::update_producer_boundaries_select_complete_failures -- --exact
cargo test --bin codex-session-control --locked install::tests::update_matrix::update_default_and_verbose_are_behaviorally_identical -- --exact
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
cargo test --bin codex-session-control --locked install::tests::failure_retry
```

Expected RED: every exact filter reports one failing test (or a compile failure for the not-yet-defined typed seam); gate causes collapse to generic errors; production `.status()` cannot distinguish spawn/wait; update producer/parity/event tables are absent; child exit `1` and abnormal completion become retryable parent errors; child exit `0` creates a second outer receipt.

- [ ] **Step 4: Implement gate-local direct outcomes**

Keep `baseline_active_turn_gate`'s list/prompt/recheck algorithm and `restart_prompt_response` direct flushed stderr write. Change only its returned semantic failure. Map the three cases at the gate call site, never from `UpdateStage::ActiveTurnGate` or displayed error text.

- [ ] **Step 5: Implement explicit spawn then wait**

Use this narrow ownership:

```rust
enum CandidateExit { Zero, One }

enum UpdateExecution {
    Render(UpdateSuccess),
    PropagateCandidateExit(CandidateExit),
}
```

Return `Result<UpdateExecution, UserFailure>`. The candidate/apply success path returns `Render`; its failure returns `Err`. The outer process emits only `update/outer`, flushes diagnostics before spawn, inherits child stdio, and adds `--verbose` before `update` only when requested. Proven spawn failure selects ordinary installation-files/retry-update. After successful spawn:

- exit `0` or `1` -> propagate unchanged with no parent renderer;
- wait error, signal, or any other exit -> exact `UpdateCompletionUnknown`;
- no cross-process result serialization.

The candidate/apply process emits only `update/apply` and owns the sole friendly result.

- [ ] **Step 6: Map all remaining update producer rows directly**

At each approved producer boundary, make `update_producer_boundaries_select_complete_failures` green: release retrieval, checksum, candidate identity, CLI/unit/service preflight, restart uncertainty, active tasks, candidate spawn/wait, writes, projection, descriptor publication, service operations, and final manifest uncertainty. Foreign/unsafe projection/native pre-observation selects `UpdateCliIntegrationCheckStatus`; ordinary CLI render/reconcile failures select retry. Descriptor clean failure is ordinary; final/stage residue is rollback-incomplete. Final manifest uncertainty checks status and never recommends immediate retry.

Remove `candidate-apply` from generic retry-stage tables. Stages remain recorded diagnostics only.

- [ ] **Step 7: Replace operational stage-string parsers**

Run normal-home/cases/live-harness fixtures in verbose mode and assert only explicit `[verbose] setup:`, `[verbose] update/apply:`, or shutdown diagnostic lines. Apply the same receipt-to-diagnostic conversion in `src/install/tests/systemd.rs`; it is compiled locally but executed only through the ignored disposable entry point. Preserve the real shutdown-before-descriptor-removal/systemd ordering. The direct staged-candidate live helper proves apply ownership only; prove outer propagation in the update subprocess fixture.

- [ ] **Step 8: Run GREEN and refactor checks**

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked install::tests::active_turn_gate
cargo test --bin codex-session-control --locked install::tests::update_matrix
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
cargo test --bin codex-session-control --locked --no-run
cargo test --test app_server_integration --locked
```

Expected GREEN: exact prompt and mutation gates remain; normal `0`/`1` is candidate-owned; completion-unknown is exact/no-retry; descriptor evidence selects ordinary versus rollback truthfully; the binary test target including `systemd.rs` compiles; non-ignored integration fixtures use typed/prefixed diagnostics.

- [ ] **Step 9: Commit**

```bash
git add src/main.rs src/cli_output.rs src/diagnostics.rs src/install.rs src/install/update.rs src/install/tests/active_turn_gate.rs src/install/tests/update_matrix.rs src/install/tests/failure_retry.rs src/install/tests/systemd.rs tests/app_server_integration/normal_home.rs tests/app_server_integration/live_harness.rs tests/app_server_integration/cases.rs
git commit -m "feat(update): preserve staged result ownership"
```

### Task 6: Finish Typed Status, Wrapper Silence, and MCP Isolation

**Files:**
- Modify: `src/main.rs`
- Modify: `src/install.rs`
- Modify: `src/install/status.rs`
- Modify: `src/install/wrapper.rs`
- Modify: `src/install/tests/status.rs`
- Modify: `src/install/tests/selected_home_evidence.rs`
- Modify: `src/install/tests/failure_retry.rs`
- Modify: `src/install/tests/codex_wrapper.rs`
- Modify: `tests/mcp_contract.rs`

- [ ] **Step 1: Write failing typed status/evidence tests**

Use one table for the four top-level states and four integration labels without cross-producting irrelevant dimensions. Add focused cases:

```rust
#[test]
fn status_classification_and_exit_code_matrix_is_exact() {
    assert_eq!(exit_code(Healthy), 0);
    assert_eq!(exit_code(Disabled), 0);
    assert_eq!(exit_code(NotInstalled), 1);
    assert_eq!(exit_code(Unhealthy), 1);
}

#[test]
fn cli_and_desktop_integration_states_are_independent() {
    assert_eq!(classify(cli_ready_desktop_fault()).cli, Ready);
    assert_eq!(classify(cli_ready_desktop_fault()).desktop, Unhealthy);
}

#[test]
fn native_registration_distinguishes_fault_from_could_not_verify() {
    assert_eq!(trusted_registration_mismatch(), Fault);
    assert_eq!(malformed_or_failed_native_query(), CouldNotVerify);
}

#[test]
fn projection_distinguishes_fault_from_could_not_verify() {
    assert_eq!(trusted_projection_mismatch(), Fault);
    assert_eq!(projection_read_or_render_ambiguity(), CouldNotVerify);
}

#[test]
fn pre_context_failures_still_render_one_four_state_status_result() {
    assert_eq!(
        status_from_paths(Err(injected_context_error())).render(),
        CONTEXT_UNHEALTHY_RESULT
    );
}
```

Retain all existing read-only mutation logs, unsafe socket no-connect, systemd exit-4, descriptor precedence, selected-home ancestor, mode, and compatibility evidence.

`CONTEXT_UNHEALTHY_RESULT` is the minimal technical projection required by the approved four-state status contract: top-level `Status: unhealthy`; no version row; service `could not verify`; both integrations `could not verify`; the already-approved problem `The installed Codex Session Control state could not be verified.`; and the already-approved recovery block `Check what needs attention:` followed by `codex-session-control status`; exit `1`; no stderr. This selects existing closed wording and recovery behavior under the status composition rule; it does not introduce a fifth state, a retry recommendation, or new copy. Missing `PATH` uses the validated absolute CSC binary for later actions, and missing cwd/systemctl resolution becomes typed inconclusive service evidence instead of escaping the status result.

Add `status_default_and_verbose_are_behaviorally_identical`: run equivalent fresh status fixtures with diagnostics off/on, remove only complete `[verbose] ` lines, then compare stdout, remaining stderr, exit code, empty mutation logs, socket-connect evidence, and independent integration classification. Extend the exact static-stage and dynamic-field diagnostic coverage before implementation.

- [ ] **Step 2: Write failing wrapper and MCP tests**

Convert wrapper raw-string assertions to exact `WrapperUnavailable` rendering and raw-error sentinel absence. Reuse the existing preparation seam with a typed result and diagnostics parameter:

```rust
async fn prepare_codex_wrapper(
    paths: &ResolvedUserPaths,
    args: Vec<OsString>,
    diagnostics: &mut Diagnostics,
) -> Result<Command, UserFailure>;
```

Production resolves paths, calls this function, then immediately passes the prepared command to the real `exec_codex_wrapper_command`. Extend the existing exact argv/cwd/environment test to enable record-mode verbose diagnostics and assert that successful preparation records no CSC diagnostic. Add one bounded process test that runs the returned fixture-native command, asserts its exit status and exact fixed stdout/stderr sentinels, and rejects any additional CSC, friendly-result, or raw-error bytes. Do not re-execute the libtest binary, assert test-harness presentation, add a public option or production environment override, or introduce a process backend or helper protocol. Keep the existing exec-failure test and make it assert exact `WrapperUnavailable`, safe diagnostics, and raw-error sentinel absence. Combine these proofs with Task 1's parser assertion that global `--verbose` reaches `codex` without entering passthrough arguments. Successful wrapper preparation and dispatch emit no event.

Run `public_catalog_is_exact` through global verbosity and explicitly reject human/`[verbose]` stdout while retaining JSON-RPC parsing, exact catalog/schema, EOF exit, reaping, and no result/error frames on stderr.

- [ ] **Step 3: Run the RED commands**

```bash
cargo test --bin codex-session-control --locked install::tests::status
cargo test --bin codex-session-control --locked install::tests::status::pre_context_failures_still_render_one_four_state_status_result -- --exact
cargo test --bin codex-session-control --locked install::tests::status::status_default_and_verbose_are_behaviorally_identical -- --exact
cargo test --bin codex-session-control --locked install::tests::selected_home_evidence::status_does_not_probe_identity_from_an_unsafe_configuration_ancestor
cargo test --bin codex-session-control --locked install::tests::codex_wrapper
cargo test --bin codex-session-control --locked install::tests::codex_wrapper::successful_wrapper_preparation_runs_only_native_bytes_when_verbose -- --exact
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
cargo test --test mcp_contract --locked public_catalog_is_exact -- --exact
```

Expected RED: status still collapses to `StatusReport { stdout, healthy }`; native/projection errors collapse into booleans; wrapper exposes raw errors; verbose MCP is rejected or can enter human dispatch.

- [ ] **Step 4: Preserve minimum status evidence at current inspection sites**

Keep `InstalledStatusState`, service/socket/app-server evidence, and Desktop descriptor evidence. Add only:

```rust
enum ProjectionEvidence { Ready, Fault, CouldNotVerify }
enum NativeRegistrationEvidence { Ready, Fault, CouldNotVerify }
```

Return the `StatusResult` defined in Task 2 directly. The command boundary has no `UserFailure` escape:

```rust
pub(crate) async fn status_from_paths(
    paths: Result<ResolvedUserPaths, ControllerError>,
    diagnostics: &mut Diagnostics,
) -> StatusResult;

pub(super) async fn status_with_context(
    context: StatusContext,
    diagnostics: &mut Diagnostics,
) -> StatusResult;
```

Project a failed `ResolvedUserPaths::from_effective_user()` to the exact four-state unhealthy context result above. Represent missing `PATH`/cwd as optional context evidence, not early errors. `StatusProblem` is a closed enum with typed identical-action grouping. Delete arbitrary `StatusFailure { check, detail, action }` and `render_status_report`. Do not add a snapshot/view hierarchy or new evidence subsystem. Preserve the unsafe-socket gate before app-server connection. Same-version update health checks require `StatusState::Healthy`, not merely exit `0`.

- [ ] **Step 5: Make wrapper failure exact and success silent**

Keep all selected-home/manifest/socket/app-server/native argv/environment checks. Convert preflight or `exec` failure directly to `WrapperUnavailable` with safe diagnostic categories only. Never stringify the raw OS or controller error. Successful exec emits no CSC diagnostic or rendered bytes even when global verbose is enabled.

- [ ] **Step 6: Finalize main dispatch and MCP bypass**

`Command::McpServer` remains the first distinct branch and never constructs human diagnostics/rendering. Other human commands produce typed success/failure or delegated candidate exit. Parser errors remain Clap-owned. Remove `LifecycleReceipt`, `SetupReport`, `UninstallReceipt`, `StatusReport`, and raw `ControllerError` rendering from human dispatch.

- [ ] **Step 7: Run GREEN and refactor checks**

```bash
cargo fmt --all
cargo test --bin codex-session-control --locked install::tests::status
cargo test --bin codex-session-control --locked install::tests::selected_home_evidence
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked install::tests::codex_wrapper
cargo test --bin codex-session-control --locked diagnostics::tests::every_dynamic_diagnostic_field_is_exactly_rendered -- --exact
cargo test --test mcp_contract --locked
cargo clippy --bin codex-session-control --all-features --locked -- -D warnings
```

Expected GREEN: exact status vocabulary/exit matrix and read-only proofs pass; wrapper is silent/safe; every MCP stdout line remains protocol; no raw error or machine receipt reaches a human result.

- [ ] **Step 8: Commit**

```bash
git add src/main.rs src/cli_output.rs src/diagnostics.rs src/install.rs src/install/status.rs src/install/wrapper.rs src/install/tests/status.rs src/install/tests/selected_home_evidence.rs src/install/tests/failure_retry.rs src/install/tests/codex_wrapper.rs tests/mcp_contract.rs
git commit -m "feat(status): finish typed human process boundaries"
```

### Task 7: Update Public Docs and Produce Fresh Final Evidence

**Files:**
- Modify: `README.md`
- Modify: `docs/desktop.md`
- Verify: every file in the File Structure

- [ ] **Step 1: Update the two forced public documentation targets**

In `README.md`, list the seven exact human command descriptions and do not present `mcp-server` as an interactive command. Keep MCP tool/catalog documentation intact.

In `docs/desktop.md`, use `Codex Desktop integration` and only `ready`, `unavailable`, `unhealthy`, `could not verify`. Preserve the statement that status is read-only and does not claim a current live Desktop connection without evidence.

- [ ] **Step 2: Run documentation RED/GREEN boundary checks**

```bash
git diff --check -- README.md docs/desktop.md
rg -n 'not_ready|Desktop configuration|Unattached running clients|native results remain authoritative' README.md docs src/cli.rs src/install
```

Expected: `git diff --check` exits `0`. Every remaining search match is either an internal typed identifier that cannot reach human output or an intentional historical artifact under `docs/superpowers/`; record each exception in implementation review evidence. Any public/current match is a failure.

- [ ] **Step 3: Commit documentation**

```bash
git add README.md docs/desktop.md
git commit -m "docs(cli): document human output vocabulary"
```

- [ ] **Step 4: Review generated help byte-for-byte**

```bash
cargo run --locked -- --help
cargo run --locked -- setup --help
cargo run --locked -- update --help
cargo run --locked -- status --help
cargo run --locked -- enable --help
cargo run --locked -- disable --help
cargo run --locked -- uninstall --help
cargo run --locked -- codex --help
```

Expected: exact approved root/setup/codex text; shared detailed help for the other visible commands; no visible `mcp-server`; no CSC verbose option after the `codex` passthrough boundary.

- [ ] **Step 5: Run all focused aggregate verification**

```bash
cargo test --test cli_contract --locked command_surface
cargo test --bin codex-session-control --locked cli_output::tests
cargo test --bin codex-session-control --locked diagnostics::tests
cargo test --bin codex-session-control --locked install::tests::setup
cargo test --bin codex-session-control --locked install::tests::enable_disable
cargo test --bin codex-session-control --locked install::tests::status
cargo test --bin codex-session-control --locked install::tests::uninstall
cargo test --bin codex-session-control --locked install::tests::codex_wrapper
cargo test --bin codex-session-control --locked install::tests::active_turn_gate
cargo test --bin codex-session-control --locked install::tests::update_matrix
cargo test --bin codex-session-control --locked install::tests::failure_retry
cargo test --bin codex-session-control --locked desktop::tests::descriptor
cargo test --test app_server_integration --locked
cargo test --test mcp_contract --locked
```

Expected: every command exits `0`; output identifies only passing tests. The app-server command runs non-ignored cases only and does not claim disposable live coverage.

- [ ] **Step 6: Run fresh full verification**

```bash
./scripts/check.sh
```

Expected: exit `0` after formatting, ShellCheck, Actionlint 1.7.12, JSON/manifest validation, Clippy with warnings denied, and locked workspace tests.

- [ ] **Step 7: Run final ordered reviews**

First run a spec-compliance review against the approved spec, this plan, and the full implementation diff. It must verify AC1-AC20 and all preserved safety/ownership boundaries. Repair valid findings, commit each fix with the owning scope, and rerun affected focused tests plus `./scripts/check.sh`.

Only after spec compliance passes, run one code-quality review. It must reject stage/error-string mapping, generic output frameworks, duplicated status evidence, cross-process protocols, broad error rewrites, privacy leakage, and unnecessary abstractions. Repair valid findings, commit each fix with the owning scope, and rerun affected focused tests plus `./scripts/check.sh`.

No extra reviewer passes are required unless a valid repair changes behavior or causes fresh verification failure.

- [ ] **Step 8: Record the disposable integration gate**

Mark this exactly as pending until CI or a documented disposable environment runs it:

```bash
bash scripts/ci/disposable-systemd-user-contract.sh
```

Do not run ignored live cases on the normal workstation and do not describe local non-ignored integration success as equivalent evidence.

## Verification

Completion requires all Task 7 commands with fresh output, plus:

```bash
git status --short
implementation_base_sha=$(git log --diff-filter=A --format=%H -1 -- docs/superpowers/plans/2026-08-10-user-facing-cli-output.md)
git log --oneline --decorate "$implementation_base_sha"..HEAD
git diff --check "$implementation_base_sha"..HEAD
git diff --stat "$implementation_base_sha"..HEAD
```

Expected evidence:

- `git status --short` is empty.
- The implementation history contains only the coherent task commits (plus review-fix commits only when a review actually required them).
- `git diff --check` exits `0`.
- Net growth stays below the approved review thresholds or triggers the stop condition below.
- Focused tests and fresh `./scripts/check.sh` pass after the last code change.
- Spec-compliance review passes before code-quality review at the intermediate checkpoint and again after final fresh verification.
- **[MANUAL/CI]** disposable systemd-user verification is explicitly recorded as passed or pending; it is never fabricated.

## Stop Conditions

Stop immediately and return to the Operator if:

- the implementation needs different user-facing copy, exit/channel behavior, prompt semantics, retry advice, safety behavior, or lifecycle mutation ordering;
- a producer cannot select one approved complete bounded result without a new user-facing or safety decision;
- a new dependency, background/concurrent subsystem, cross-process serialization protocol, generic stage/message table, generic CLI/report framework, arbitrary error-string parsing, or broad `ControllerError` rewrite appears necessary;
- work expands into unrelated MCP, app-server, release, or domain behavior;
- actual net growth exceeds roughly 1,500 production lines or 1,800 test/documentation lines; the threshold requires Operator review, not automatic rejection;
- the source spec/review/plan boundary is missing, altered, contradicted, or not approved/passed;
- unrelated worktree dirt cannot be separated safely;
- a required focused or full verification failure reveals behavior outside this plan;
- three attempted fixes fail on the same issue; restart root-cause analysis instead of adding another workaround;
- the disposable live gate would have to run on a normal workstation.

Do not stop for a concrete in-scope compile/test consumer such as the seven-visible-command CI/release checks; update that bounded consumer and record the repository reason.
