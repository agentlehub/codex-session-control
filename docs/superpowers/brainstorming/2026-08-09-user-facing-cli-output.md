# Brainstorming: User-Facing CLI Output

**Status:** approved
**Review:** [Brainstorming review trace](../reviews/2026-08-10-user-facing-cli-output-brainstorming-review.md)
**Next:** [Writing specs handoff](../../handoffs/2026-08-10-040021-user-facing-cli-output-writing-specs.md)

## Context

The feature branch `codex/user-facing-cli-output` starts from merged `main` at `528ef31566048520617929a45ea446ff9af70559`.

The current human CLI exposes internal lifecycle artifacts directly: successful lifecycle commands print completed stage journals such as `completed: candidate-preflight`, while stdout prints machine-oriented postcondition receipts such as `Durable plugin state: current` and `Loaded task state: may_be_stale`. Root help also leaves most commands undescribed. Exact-output and stage-order tests currently couple lifecycle correctness to these rendered strings.

The operator has confirmed that default output should answer only what happened, whether the user must do anything, and—on failure—what to do next. The final wording has been reviewed interactively one command and exceptional outcome at a time. A global `--verbose` mode will retain diagnostic stage evidence without polluting default output.

No output implementation begins until this brainstorming artifact is complete, reviewed, and approved.

## Current Decisions

- Default output is concise and user-facing; lifecycle failures use a closed typed problem/recovery catalog; and `--verbose` adds a safe chronological, demand-driven diagnostic trace without changing the normal result or exit behavior. ([Q1](#q1-should-diagnostic-lifecycle-details-remain-available-through-a-verbose-mode), [Q14](#q14-what-shared-structure-should-lifecycle-command-failures-use), [Q15](#q15-how-should-warnings-and-partial-success-notices-be-written), [Q16](#q16-what-exact-contract-should-global---verbose-provide-for-debugging-and-testing), [Q23](#q23-how-should-lifecycle-failures-safety-refusals-and-partial-outcomes-be-bounded), [Q28](#q28-should-the-initial-verbose-contract-cover-every-internal-evidence-category))
- Root and subcommand help use shared human-facing descriptions, document meaningful arguments, hide the internal `mcp-server` command, and preserve the `codex` passthrough boundary. ([Q2](#q2-what-should-the-root-help-present-to-users), [Q19](#q19-what-should-each-subcommands-help-surface-say))
- `setup` and `update` report the versioned outcome and show only the CLI, Desktop, or disabled-service guidance relevant to that run; `setup` also identifies already-running clients that did not switch to CSC. ([Q3](#q3-what-should-a-successful-setup-print-by-default), [Q4](#q4-what-should-a-successful-update-print-by-default), [Q20](#q20-should-setup-and-enable-preserve-guidance-for-already-running-unattached-clients), [Q21](#q21-what-should-update-report-when-the-installed-version-is-current-but-the-service-is-disabled))
- `status` keeps the readable list format, gives direct recovery actions, uses independently derived integration states, and returns an exit code based only on the top-level state. ([Q5](#q5-what-should-a-healthy-status-print-by-default), [Q6](#q6-what-should-status-print-for-an-intentionally-disabled-installation), [Q7](#q7-how-should-an-unhealthy-status-present-problems-and-recovery-actions), [Q8](#q8-what-should-status-print-when-codex-session-control-is-not-installed), [Q17](#q17-what-mutually-exclusive-vocabulary-should-client-integration-states-use), [Q22](#q22-what-exit-code-should-each-top-level-status-state-return))
- `enable`, `disable`, and `uninstall` report the lifecycle outcome, preserve Codex data, and show Desktop restart, setup, already-running-client, or truthful partial-cleanup guidance only when relevant. ([Q10](#q10-what-should-a-successful-enable-print-by-default), [Q11](#q11-what-should-a-successful-disable-print-by-default), [Q12](#q12-what-should-a-successful-uninstall-print-by-default), [Q20](#q20-should-setup-and-enable-preserve-guidance-for-already-running-unattached-clients), [Q25](#q25-what-should-disable-report-when-the-service-stopped-but-desktop-cleanup-failed))
- The `codex` wrapper is silent on success; launch failures state the outcome and route users to `status`. ([Q13](#q13-what-should-the-codex-wrapper-print))
- Implementation uses one CLI rendering boundary, one concrete diagnostic emitter, one non-generic success enum, one bounded failure enum, and one typed final status result while retaining existing command-local stage/evidence types. ([Q18](#q18-which-implementation-architecture-should-produce-the-approved-default-and-verbose-output), [Q27](#q27-should-the-implementation-architecture-be-simplified-without-changing-user-facing-output))
- The existing active-task update confirmation prompt remains unchanged; deliberate decline uses the cancellation outcome from Q23. ([Q24](#q24-should-the-active-task-update-confirmation-prompt-be-rewritten))
- Successful `setup` and `enable` compose primary outcome, CLI guidance, then Desktop guidance from independent typed facts; more specific detected-client guidance replaces generic duplicate actions. ([Q26](#q26-how-should-setup-and-enable-compose-generic-and-detected-client-guidance))
- Tests cover each distinct output/safety contract with focused tables and existing operational fixtures; they do not cross-product independent command, state, failure, recovery, and verbosity dimensions. ([Q29](#q29-how-should-tests-cover-the-output-refactor-without-a-cartesian-explosion))
- Complexity preferences are enforced through ordinary review; implementation pauses only for changed approved behavior, material scope/runtime expansion, or substantial measured growth. ([Q30](#q30-what-complexity-guardrails-should-pause-implementation))
- The next specification synthesizes only active requirements and constraints; this artifact retains superseded decisions solely as clearly marked provenance. ([Q31](#q31-should-the-specification-carry-the-full-brainstorming-chronology))

## Supporting Material

- [Rust architecture review](../reviews/2026-08-10-user-facing-cli-output-rust-architecture-review.md)

## Q&A

### Q1: Should diagnostic lifecycle details remain available through a verbose mode?

**Recommended**

Use a global `--verbose` option. Default output should remain strictly user-facing, while verbose output adds the internal completed-stage history and technical diagnostics needed for support. The alternatives were removing diagnostic stages from the CLI entirely, which would force every investigation into service logs, or limiting verbose mode to lifecycle commands, which would make the CLI inconsistent and prevent the same diagnostic contract from extending cleanly to `status`.

**Resolved**

The operator selected the recommended global verbose mode. Concise output remains the default across the CLI, and `--verbose` is the explicit diagnostic surface.

**Rationale**

The existing stage journal contains useful failure-localization evidence, but printing it unconditionally is the core usability defect. An explicit global mode preserves supportability without making ordinary users interpret internal transaction stages, and one global convention is simpler than command-specific verbosity rules.

### Q2: What should the root help present to users?

**Recommended**

Describe the product directly as `Manage Codex Session Control`, give every human-facing command one concise purpose statement, and expose the global diagnostic option without explaining internal lifecycle architecture. Keep `mcp-server` callable but hidden because the installed plugin starts this stdio-to-socket bridge automatically and manual invocation provides no useful interactive workflow. Use Clap's conventional `[OPTIONS] <COMMAND>` usage ordering while allowing the global `--verbose` option on either side of the command.

**Resolved**

The operator approved this exact root-help contract:

```text
Manage Codex Session Control

Usage: codex-session-control [OPTIONS] <COMMAND>

Commands:
  setup      Install Codex Session Control and start the shared app-server
  update     Install the latest release
  status     Check whether Codex Session Control is ready
  enable     Start the service and turn on automatic startup
  disable    Stop the service and turn off automatic startup
  uninstall  Remove the service while keeping your Codex data
  codex      Start Codex CLI through the shared app-server

Options:
      --verbose  Show diagnostic details
  -h, --help     Print help
  -V, --version  Print version
```

The internal `mcp-server` command remains callable but is omitted from root help.

**Rationale**

The command list should explain user outcomes rather than implementation stages. Repeating the product name in every row creates noise, but using it for `setup` and `status` removes ambiguity where the subject matters. `Enable` and `disable` describe automatic startup rather than the less natural phrase “at login.” Hiding `mcp-server` prevents users from launching a non-interactive transport process that appears to hang, without changing the plugin's executable contract.

### Q3: What should a successful `setup` print by default?

**Recommended**

Replace the component receipt and diagnostic state labels with one outcome sentence followed by client-specific activation guidance. The success line should render the actual candidate version at runtime. CLI guidance should name the wrapper command explicitly, while Desktop guidance should explain that only an already-running Desktop needs restarting and that the restart makes Codex Session Control available inside Desktop; it must not imply Desktop itself is otherwise unusable.

**Resolved**

The operator approved this exact default shape, with `{version}` dynamically rendered rather than hardcoded:

```text
Codex Session Control {version} is ready.

To use Codex Session Control with Codex CLI, start the CLI with:
  codex-session-control codex

If Codex Desktop is already running, restart it to make Codex Session Control available there.
```

The Desktop paragraph appears only when `setup` changed Desktop configuration. Paths, component receipts, projection state, and completed stages remain available through `--verbose`. Relevant warnings and recovery instructions remain visible in default output.

**Rationale**

The previous `Desktop restart required` flag represented a descriptor change, not trustworthy proof that Desktop was running or disconnected. Process inspection can observe a likely Desktop process but cannot prove which descriptor it loaded. Conditioning the paragraph on a configuration change and phrasing it around an already-running Desktop communicates the required behavior without overstating runtime knowledge. The approved wording frames both instructions around making Codex Session Control available in each client rather than around generic client startup.

### Q4: What should a successful `update` print by default?

**Recommended**

Replace the postcondition receipt with a direct update outcome and only the action that matters afterward. Distinguish a newly applied update from an already-current healthy installation. If the managed service remains disabled, say so and provide the exact enable command. If the update changes Desktop configuration, explain that an already-running Desktop must restart specifically to use the updated Codex Session Control version.

**Resolved**

The operator approved these default variants, with `{version}` dynamically rendered:

Applied update:

```text
Codex Session Control was updated to {version}.

Start a new task to use the updated plugin.
```

Already current:

```text
Codex Session Control {version} is already up to date.
```

Applied update while the service remains disabled:

```text
Codex Session Control was updated to {version}.

The service remains disabled. Run `codex-session-control enable` when you want to use it.
```

When Desktop configuration changed, append:

```text
If Codex Desktop is already running, restart it to use the updated version of Codex Session Control.
```

Technical component receipts, paths, and completed stages remain available through `--verbose`.

**Rationale**

Users need to know whether an update actually occurred and whether any action is required, not how each durable artifact reconciled. An already-current installation requires no follow-up. A disabled service is materially different because updating it does not enable it. Setup introduces Codex Session Control to Desktop, while update replaces an existing version, so the Desktop guidance must describe using the updated version rather than making the feature available for the first time.

### Q5: What should a healthy `status` print by default?

**Recommended**

Keep the compact list format because `status` is an inspection command, but translate internal receipt terminology into stable user-facing concepts. Preserve `healthy` for overall system health and use `ready` for each client integration. Remove the wrapper command from the state row because usage guidance is not status, and omit the unverifiable loaded-task field from default output.

**Resolved**

The operator approved this exact healthy status shape:

```text
Status: healthy
Version: {version}
Service: running, starts automatically
Codex CLI integration: ready
Codex Desktop integration: ready
```

The runtime version is dynamically rendered. Detailed component evidence remains available through `--verbose`.

**Rationale**

The one-line proposal removed too much information and made `status` less useful. A compact list remains easy to scan while explaining service behavior and client readiness in user terms. `Healthy` and `ready` intentionally describe different levels: the first summarizes the installation as a whole, while the latter reports whether each supported client integration is prepared. Naming the rows as integrations avoids claiming that either client is currently connected.

### Q6: What should `status` print for an intentionally disabled installation?

**Recommended**

Treat intentional disablement as its own user-facing state rather than reporting the internally coherent installation as healthy. Keep the compact list, state that the service is stopped and automatic startup is off, mark both client integrations unavailable, and give the exact enable command.

**Resolved**

The operator approved this exact disabled status shape:

```text
Status: disabled
Version: {version}
Service: stopped, automatic startup is off
Codex CLI integration: unavailable
Codex Desktop integration: unavailable

Run `codex-session-control enable` to start Codex Session Control.
```

**Rationale**

The internal health check considers a deliberately disabled and inactive service coherent, but `Status: healthy` would mislead users who are checking whether the product is available. `Disabled` accurately names the intentional lifecycle state while preserving a successful command result. Both integrations depend on the stopped shared service, so they are unavailable until the user explicitly enables it.

### Q7: How should an unhealthy `status` present problems and recovery actions?

**Status:** Partially superseded by [Q17](#q17-what-mutually-exclusive-vocabulary-should-client-integration-states-use). The problem and recovery structure remains active; the example's integration labels do not.

**Recommended**

Use `Status: unhealthy` instead of the internal term `drifted`, preserve the compact summary list, and translate failed checks into readable problem statements. Do not place recovery commands in a detached `Next` section because multiple failures can require different actions. Attach a specific verb-led action to each problem; when several problems share one action, group those problems and show the action once. Keep raw check identifiers and technical evidence in `--verbose`.

**Resolved**

The operator approved this structure. A representative unhealthy report with one shared action is:

```text
Status: unhealthy
Version: {version}
Service: stopped unexpectedly, automatic startup is on
Codex CLI integration: unavailable
Codex Desktop integration: unavailable

Problems:
- The service is configured to run but is stopped.
- The service connection is unavailable.

Check the service logs for both problems:
  journalctl --user -u codex-session-control.service
```

**Superseded by Q17:** In this configured shared-service-failure example, both integration lines render `unhealthy`, not `unavailable`. An optional Desktop integration that was never configured remains `unavailable`.

When actions differ, keep each instruction with its problem:

```text
Problems:
- The service file is missing.
  Restore it by running:
  codex-session-control setup

- The service is configured to run but is stopped.
  Check the service logs:
  journalctl --user -u codex-session-control.service
```

**Rationale**

The old check names and `action:` labels expose implementation vocabulary, while a single detached recovery section loses the relationship between a failure and its remedy. Verb-led instructions state what the user should do without a vague `Next` heading. Grouping identical actions avoids repetition only when it preserves an unambiguous mapping; distinct actions remain adjacent to their corresponding problems.

### Q8: What should `status` print when Codex Session Control is not installed?

**Recommended**

Recognize a clean first-install state as `not installed` instead of rendering expected missing files as drift. Omit a nonexistent version and service receipt, mark both integrations unavailable, and collapse the missing manifest and configuration actions into one direct setup instruction.

**Resolved**

The operator approved this exact not-installed shape:

```text
Status: not installed
Codex CLI integration: unavailable
Codex Desktop integration: unavailable

Install Codex Session Control by running:
  codex-session-control setup
```

**Rationale**

A clean absence is not corruption and should not be described as `drifted`. Missing product artifacts are implementation evidence for the single user-level fact that setup has not run. One installation instruction is clearer than repeating the same action under multiple expected missing-file checks.

### Q9: How should `status` describe a healthy installation whose Desktop integration is not ready?

**Status:** Superseded by [Q17](#q17-what-mutually-exclusive-vocabulary-should-client-integration-states-use).

**Recommended**

Keep overall `Status: healthy` when the installed service and CLI integration are healthy because Desktop integration is optional. Do not expose the internal `unavailable` label: it means CSC has no stored Desktop attachment and discovery found no supported configuration location, not that Desktop itself is absent. Collapse internal Desktop `unavailable` and `not_ready` into user-facing `not ready`; known actionable failures still appear below with recovery instructions. Retain `could not verify` when evidence is insufficient.

**Resolved**

The operator approved this representative output:

```text
Status: healthy
Version: {version}
Service: running, starts automatically
Codex CLI integration: ready
Codex Desktop integration: not ready
```

**Superseded by Q17:** `not ready` is removed from the user-facing vocabulary. This optional integration renders `unavailable` because CSC has no configured/discovered Desktop target.

The original user-facing integration vocabulary was `ready`, `not ready`, and `could not verify`. Q17 supersedes it with four mutually exclusive states after review exposed ambiguity between readiness and availability. `--verbose` preserves the underlying evidence.

**Rationale**

This rationale is historical and superseded by Q17. The original decision treated `unavailable` as if it claimed that the Desktop application itself was absent. Q17 corrects that interpretation: the label describes CSC's ability to provide the integration, while CSC still does not claim whether Desktop is installed, open, or connected. The original collapse to `not ready` remains above only as historical decision context.

### Q10: What should a successful `enable` print by default?

**Recommended**

Replace service and attachment receipts with one sentence that states the two outcomes users care about: CSC is running and will start automatically. When enabling publishes Desktop configuration, reuse the setup-specific instruction for an already-running Desktop. When Desktop integration cannot be enabled because no configured integration target exists, state that it is unavailable and provide the setup command instead of using internal attachment terminology.

**Resolved**

The operator approved the normal success form:

```text
Codex Session Control is running and will start automatically.

If Codex Desktop is already running, restart it to make Codex Session Control available there.
```

The Desktop paragraph appears only when enabling changed Desktop configuration. When Desktop integration remains unavailable to CSC, use:

```text
Codex Session Control is running and will start automatically.

Codex Desktop integration is unavailable.
Run `codex-session-control setup` to set it up.
```

**Rationale**

The command's primary user-level outcomes are immediate service availability and automatic startup. CLI attachment is implied by the successful running state and does not require a separate receipt. Desktop guidance remains conditional because it either requires restarting an already-running app after newly published configuration or running setup when no usable integration target is recorded.

### Q11: What should a successful `disable` print by default?

**Recommended**

Replace service-state receipts with a direct outcome stating that CSC is stopped and automatic startup is off. Reassure the user that Codex data is unchanged. When disabling removed Desktop configuration, explain that only an already-running Desktop needs restarting and that the restart continues Desktop operation without CSC.

**Resolved**

The operator approved this default form:

```text
Codex Session Control is stopped and will not start automatically.

Your Codex data is unchanged.
If Codex Desktop is already running, restart it to continue without Codex Session Control.
```

The Desktop sentence appears only when disabling removed its CSC configuration.

**Rationale**

Users need confirmation of both immediate and automatic-startup state, plus assurance that disabling the service does not delete their Codex data. The previous “ordinary mode” phrase exposed product terminology without explaining the outcome. The approved wording makes clear that Desktop itself remains usable and that restart is only needed to stop using CSC there.

### Q12: What should a successful `uninstall` print by default?

**Recommended**

Replace the per-artifact removal receipt with one direct uninstall outcome and one clear preservation guarantee. Keep paths and removed-component details in `--verbose`. When uninstall removed Desktop configuration, reuse the approved disable wording for an already-running Desktop.

**Resolved**

The operator approved this default form:

```text
Codex Session Control was uninstalled.

Your Codex data is unchanged.
If Codex Desktop is already running, restart it to continue without Codex Session Control.
```

The Desktop sentence appears only when uninstall removed its CSC configuration.

**Rationale**

The old receipt forced users to interpret five internal product artifacts and four preservation fields. The user-level outcome is simply that CSC was removed while Codex data remained intact. A conditional Desktop instruction covers the only required follow-up without exposing descriptor terminology.

### Q13: What should the `codex` wrapper print?

**Recommended**

Keep the successful path silent because the wrapper replaces itself with Codex CLI and must not add noise to the interactive application. On preflight failure, replace the internal “Codex authority” reason and branching status-or-enable guidance with one user-level launch failure and one `status` command. Let `status` report the actual state and specific recovery action. Preserve the technical preflight cause in `--verbose`.

**Resolved**

The operator approved no wrapper output before a successful Codex CLI start. The approved default failure is:

```text
Codex CLI could not start because Codex Session Control is unavailable.

Check what needs attention:
  codex-session-control status
```

**Rationale**

Successful wrapper output would pollute the Codex terminal interface. The old error combined an internal authority abstraction with conditional recovery that duplicated `status` logic. One stable status entry point keeps the wrapper message concise and ensures the user receives recovery guidance based on the current installation state rather than a generic guess.

### Q14: What shared structure should lifecycle command failures use?

**Recommended**

Replace completed-stage journals and `failed at <internal-stage>` output with three user-facing elements: a natural command-level outcome, a readable problem statement, and a verb-led recovery instruction. Use command-specific outcome verbs for install, update, start, stop, and uninstall. Keep completed stages, internal stage names, diagnostic-safe causes, paths, and retry mechanics in `--verbose`.

**Resolved**

The operator approved this representative setup failure:

```text
Codex Session Control could not be installed.

The service state could not be verified.

Try again:
  codex-session-control setup
```

Other lifecycle outcomes use:

```text
Codex Session Control could not be updated.
Codex Session Control could not be started.
Codex Session Control could not be stopped.
Codex Session Control could not be uninstalled.
```

The concrete problem follows the outcome. Recovery uses a specific instruction such as `Try again`, `Check the service logs`, or `Repair the installation`; it does not use a generic `Next` heading.

**Rationale**

Stage journals are valuable implementation evidence but make ordinary failures read like transaction logs. The approved structure answers what failed, why in user terms, and what action to take. Command-specific verbs remain clear without exposing the lifecycle stage machine, while verbose mode preserves the evidence needed for diagnosis and support.

### Q15: How should warnings and partial-success notices be written?

**Recommended**

Print the primary outcome first, then show only notices that affect user expectations or require action. Compatibility warnings should name the untested versions and likely impact without the internal phrase “native results remain authoritative.” Desktop discovery notices should describe integration readiness rather than attachment availability. PATH notices should keep the existing clear explanation instead of introducing a shell-specific command or profile-edit policy.

**Resolved**

The operator approved these compatibility and Desktop rewrites:

```text
Warning: Codex {codex_version} has not been tested with Codex Session Control {version}.
Some features may not work as expected.
```

```text
Codex Desktop integration is unavailable because a compatible Desktop launcher was not found.
```

The PATH notice remains intentionally generic and uses:

```text
Note: `{local_bin}` is not on your PATH.
Add it to your PATH to use the short `codex-session-control` command.
```

The design does not add shell detection, modify shell profiles, or print a supposedly universal persistent PATH command.

**Rationale**

Warnings should explain user impact, not implementation authority or descriptor discovery. A persistent PATH command is shell- and startup-file-specific; printing one generic command would either be temporary or wrong for some users. The approved notice remains accurate across shells and leaves profile policy outside CSC.

### Q16: What exact contract should global `--verbose` provide for debugging and testing?

**Status:** Partially superseded by [Q28](#q28-should-the-initial-verbose-contract-cover-every-internal-evidence-category). The trace format, chronology, privacy, phases, channels, and parity rules remain active; the exhaustive evidence breadth does not.

**Recommended**

Reject a generic buffered `Diagnostics:` dump. Instead, add a chronological line-oriented diagnostic trace on stderr with every line prefixed `[verbose] <command>[/<phase>]:`. Preserve default stdout, default-visible stderr, and exit codes exactly when verbosity is enabled. Stream lifecycle stages as they complete, append failed-stage and diagnostic-safe cause evidence on failure, and render a buffered read-only evidence snapshot for `status`. Identify staged update output as `update/outer` or `update/apply` so diagnostics remain truthful across the self-exec boundary. Keep diagnostic prose human-oriented and explicitly non-machine-stable; test lifecycle correctness through typed events rather than parsing verbose strings.

**Resolved**

The operator approved the prefixed streaming-trace contract. Representative diagnostics are:

```text
[verbose] setup: controller {version} ({target})
[verbose] setup: selected Codex home {codex_home}
[verbose] setup: completed preflight
[verbose] setup: completed binary
[verbose] setup: completed configuration
...
[verbose] setup: service enabled and active
[verbose] setup: plugin current at {plugin_path}
[verbose] setup: Desktop descriptor published at {descriptor_path}
```

Staged update uses explicit process phases:

```text
[verbose] update/outer: candidate {version} verified
[verbose] update/outer: starting staged candidate
[verbose] update/apply: staged marker accepted
[verbose] update/apply: completed service-restart
[verbose] update/apply: completed manifest
[verbose] update/outer: staged candidate exited successfully
```

Verbose diagnostics include controller/candidate identity, completed and failed stages, diagnostic-safe failure categories and evidence, managed evidence paths, service and restart decisions, projection/plugin state, Desktop internal evidence, and partial-cleanup facts. They never include credentials, environment dumps, configuration contents, task or rollout data, full process command lines, timestamps, PIDs, or telemetry identifiers.

Warnings, recovery instructions, manual gates, partial-cleanup warnings, and any path the user must inspect remain default-visible. Successful `codex` remains silent even under verbosity; failed wrapper preflight may add technical diagnostics. `mcp-server` stdout remains protocol-only. CSC verbosity must precede the passthrough command (`codex-session-control --verbose codex`); arguments after `codex` belong to native Codex. Other subcommands may accept the global option before or after the subcommand.

**Rationale**

Streaming evidence localizes hangs and failures, while a buffered dump would appear too late and cannot truthfully span update's outer and candidate processes. A stable prefix separates diagnostics from warnings without creating a versioned machine schema. Additive stderr preserves the approved user-facing output and makes default-versus-verbose behavior testable. Typed diagnostic events decouple lifecycle tests from prose while still allowing small rendering-contract tests for the prefix, channel, and phase labels.

### Q17: What mutually exclusive vocabulary should client integration states use?

**Recommended**

Replace the ambiguous `not ready` label with a four-state vocabulary based on the kind and strength of evidence. Use `unavailable` only when CSC cannot provide the integration because it is absent, intentionally offline, or no optional integration target was configured/discovered. Use `unhealthy` when the integration is expected to work in the current state but decisive evidence proves a fault. Preserve `could not verify` for inconclusive evidence and `ready` for affirmative evidence. Derive CLI and Desktop states independently rather than copying top-level status.

**Resolved**

The operator approved these definitions after a Sol/xhigh review and correction:

- `ready` — verified and usable.
- `unavailable` — CSC is not installed, is intentionally disabled, or the optional integration was not configured/discovered; this never claims the Desktop application itself is absent.
- `unhealthy` — the integration is expected to be ready in the current state, but decisive evidence shows a problem prevents correct operation.
- `could not verify` — evidence is inconclusive.

The corrected configured shared-service-failure example is:

```text
Status: unhealthy
Version: {version}
Service: stopped unexpectedly, automatic startup is on
Codex CLI integration: unhealthy
Codex Desktop integration: unhealthy
```

If Desktop integration was never configured or discovered, its line remains:

```text
Codex Desktop integration: unavailable
```

Approved impacts on earlier decisions are:

- Q6 disabled remains unchanged: both integrations are `unavailable`.
- Q7's configured shared-service-failure example changes both integration lines to `unhealthy`; problem/action grouping is unchanged.
- Q8 not installed remains unchanged: both integrations are `unavailable`.
- Q9's optional Desktop line changes from `not ready` to `unavailable`.

Desktop evidence maps as follows: `DesktopConfigurationState::Ready` maps to `ready`; `Unavailable` maps to `unavailable`; and `Unverified` maps to `could not verify`. `NotReady` maps to `unavailable` when the installation is intentionally disabled, and to `unhealthy` when the integration is expected to work but decisive evidence proves a fault. A clean not-installed state overrides both integrations to `unavailable`. A healthy installation may report Desktop `unavailable` or `could not verify` because optional Desktop evidence does not by itself make the installation unhealthy.

Implementation must derive CLI state explicitly from installation, native registration, service, socket, and app-server evidence: intentional absence/offline maps to `unavailable`, decisive operational faults to `unhealthy`, inconclusive evidence to `could not verify`, and affirmative evidence to `ready`. Top-level `Status: unhealthy` does not force both integrations unhealthy; a Desktop-only problem can coexist with CLI `ready`, and an optional unconfigured Desktop integration remains `unavailable`.

**Rationale**

The earlier `not ready` and `unavailable` definitions overlapped, while the initial unhealthy example labeled the same shared-service failure differently for CLI and Desktop. The four-state vocabulary separates intentional or optional absence from proven faults and uncertainty. Independent evidence mapping prevents a global failure from overwriting valid per-integration facts and keeps `unavailable` scoped to CSC's ability to provide an integration rather than the presence, runtime state, or connection state of the Desktop application.

### Q18: Which implementation architecture should produce the approved default and verbose output?

**Status:** Partially superseded by [Q27](#q27-should-the-implementation-architecture-be-simplified-without-changing-user-facing-output). The typed rendering and diagnostic boundaries remain active; the generic reports, sink trait, and parallel status snapshot/view model do not.

**Recommended**

Use a narrow typed presentation boundary. Command modules should return command-specific typed outcomes, notices, structured failures, and owned diagnostic events. A dedicated CLI-output module should be the only owner of user-facing prose, while a small synchronous diagnostic sink should stream prefixed verbose events or record them in tests. Status should retain a typed evidence snapshot and derive a separate user-facing view. Keep the existing low-level `ControllerError` as the technical source rather than expanding this feature into a complete error-system rewrite.

**Resolved**

The operator approved approach A after an independent Sol/xhigh architecture review:

- Add `src/cli_output.rs` for typed reports, failures, notices, renderers, and final channel/exit representation.
- Add `src/diagnostics.rs` for diagnostic scopes and events plus only three sinks: no-op, stderr, and a test recorder.
- Use owned event data and a synchronous best-effort `DiagnosticSink`; do not add channels, background tasks, global mutable state, `tracing`, JSON, callbacks, or a trait hierarchy per command.
- Let command modules own operational decisions and return narrow command-specific outcomes.
- Preserve status inspection evidence in a `StatusSnapshot`, derive a pure `StatusView`, render the normal view through `cli_output`, and render verbose evidence from the snapshot.
- Wrap technical `ControllerError` once at the command boundary with command outcome, readable problem, recovery, failed stage, and cleanup evidence; do not retype every low-level error.
- Keep `ControllerError` only as the in-memory technical source. Never stringify an unrestricted error into a diagnostic event; project it into an allowlisted typed diagnostic cause, and use a generic category when legacy errors have no safe typed projection.
- Let the staged update candidate own the single friendly result. The outer process emits only `update/outer` diagnostics and propagates the candidate exit code; no private serialized result protocol is introduced.
- Migrate tests from rendered stage strings to typed diagnostic-event assertions while retaining small exact-output snapshots for approved prose.

The supporting review is preserved in full at [`docs/superpowers/reviews/2026-08-10-user-facing-cli-output-rust-architecture-review.md`](../reviews/2026-08-10-user-facing-cli-output-rust-architecture-review.md).

Three contract clarifications are part of the approved approach:

- Chronological diagnostics mean CSC emission order, including the serialized outer/candidate handoff, not a total merged ordering after stdout and stderr are redirected independently.
- The primary-outcome-first rule applies to default-visible success material; opted-in streaming verbose stages can precede the final success outcome.
- `ready` is a snapshot based on affirmative evidence at inspection time, not a guarantee against a later race.
- Diagnostic privacy is enforced structurally, not through best-effort redaction: renderers accept only typed allowlisted diagnostic fields, while the original error remains available solely through the in-memory source chain. Sentinel tests cover every prohibited data class from Q16.

These artifact decisions are authoritative over the supporting architecture sketch where later review exposed a narrower contract. In particular, the sketch's unrestricted `DiagnosticEvent::Failure { cause: String }` is superseded by the diagnostic-safe typed projection above; Q20 adds independently typed already-running-client facts; Q21 adds service enablement to `AlreadyCurrent`; Q22 fixes the status exit matrix; Q23 replaces free-form problem/recovery fragments with the closed typed catalog; and Q27 removes the sink trait, generic report family, parallel status snapshot/view hierarchy, and orthogonal failure-state axes.

**Rationale**

The code already owns typed stages, evidence cases, service states, Desktop states, and restart reasons, but currently discards that information into strings immediately before presentation. Extending string buffers would preserve the coupling that caused the UX problem, while post-processing those strings would make correctness depend on private prose. The narrow typed design separates operational facts from rendering without inventing a generic UI framework or retyping the entire error stack. A synchronous event sink matches the sequential async lifecycle, provides chronological debugging evidence, and remains easy to record in tests.

**Approaches Considered**

1. **Typed outcomes, structured failures, and diagnostic events — selected.** This requires the largest deliberate refactor, but it creates the clean boundary needed by the approved output, independent status classification, and update phase diagnostics. The review narrowed it to two modules and one small sink trait to control complexity.
2. **Extend existing stdout/stderr string reports — rejected.** This would reduce the initial diff but preserve business logic that constructs presentation strings, continue coupling tests to wording, and make the staged update boundary awkward.
3. **Post-process existing output strings — rejected.** This appears smallest but would parse private prose such as lifecycle stage errors, duplicate semantic knowledge, and create the most brittle implementation.

### Q19: What should each subcommand's help surface say?

**Recommended**

Reuse the approved root-help description for every subcommand so the command list and detailed help cannot drift. Document only arguments whose behavior is not obvious. Lifecycle and inspection commands should expose `--verbose` both globally and after the subcommand, while `codex` must keep the passthrough boundary explicit: CSC verbosity appears before `codex`, and every argument after `codex` belongs to Codex CLI.

The approved detailed help examples are:

```text
Install Codex Session Control and start the shared app-server

Usage: codex-session-control setup [OPTIONS]

Options:
      --desktop-launcher <PATH>  Absolute path to the Codex Desktop executable when automatic discovery fails
      --verbose                  Show diagnostic details
  -h, --help                     Print help
```

```text
Start Codex CLI through the shared app-server

Usage: codex-session-control codex [ARGS]...

Arguments:
  [ARGS]...  Arguments passed directly to Codex CLI

Options:
  -h, --help  Print help
```

**Resolved**

The operator approved the proposed subcommand-help contract. Every subcommand reuses its root-help description, meaningful arguments receive concise user-facing descriptions, and lifecycle or inspection commands show `--verbose` after the subcommand. The `codex` command does not advertise or accept CSC verbosity after its passthrough boundary; users invoke `codex-session-control --verbose codex` when they need CSC diagnostics.

**Rationale**

The current subcommand help is incomplete: most commands lack descriptions, `--desktop-launcher` and `[ARGS]...` are unexplained, and the old uninstall wording exposes implementation detail. Reusing one description per command produces a consistent surface without introducing another copy of the prose. Keeping `codex` passthrough unambiguous prevents native Codex arguments from being consumed by CSC.

**Approaches Considered**

1. **Shared descriptions with targeted argument documentation — selected.** This keeps root and subcommand help consistent, explains only the non-obvious inputs, and preserves the `codex` passthrough boundary.
2. **Independent long-form help for every subcommand — rejected.** This would add prose without improving the simple command surface and create wording that can drift from root help.
3. **Keep generated sparse help — rejected.** This is the smallest change, but it leaves users guessing about Desktop launcher discovery, argument forwarding, and the purpose of most commands.

### Q20: Should `setup` and `enable` preserve guidance for already-running unattached clients?

**Recommended**

Preserve the existing same-user process detection because an already-running CLI or Desktop process does not switch to CSC automatically. Replace its internal attachment/migration terminology with independent conditional notices, rendered only for the detected client:

```text
Codex CLI is already running without Codex Session Control.
Exit it, then start it with:
  codex-session-control codex
```

```text
Codex Desktop is already running without Codex Session Control.
Restart Codex Desktop to use Codex Session Control there.
```

The alternatives were removing the notice, which would hide a required action, or moving it to `--verbose`, which would violate the rule that actionable guidance remains default-visible.

**Resolved**

The operator selected option A: preserve the detection and replace the current `Unattached running clients` receipt with the concise conditional notices above. CLI and Desktop remain independent facts so either or both notices may appear.

**Rationale**

The approved setup and enable outcomes describe the newly configured service, but they cannot make an already-running client adopt that configuration. Users must be told when their current client remains outside CSC. Keeping this as a typed outcome fact preserves the behavior without leaking attachment or migration terminology into the user-facing surface.

### Q21: What should `update` report when the installed version is current but the service is disabled?

**Recommended**

Preserve the already-current result and append the same disabled-service guidance used after an applied update:

```text
Codex Session Control {version} is already up to date.

The service remains disabled. Run `codex-session-control enable` when you want to use it.
```

The alternative was keeping the shorter already-current output, which is version-correct but hides why CSC remains unavailable and what restores it.

**Resolved**

The operator selected option A: an already-current update reports the disabled state and exact enable command. The typed `AlreadyCurrent` outcome therefore retains service enablement rather than carrying only the version.

**Rationale**

Update answers both whether the release changed and whether the installed product is usable afterward. A disabled service is intentional, but it remains relevant to the user's next action regardless of whether the binary was newly installed or already current. Reusing one guidance paragraph keeps both update paths consistent.

### Q22: What exit code should each top-level `status` state return?

**Recommended**

Use an explicit top-level matrix:

| Top-level status | Exit code |
| --- | ---: |
| `healthy` | `0` |
| `disabled` | `0` |
| `not installed` | `1` |
| `unhealthy` | `1` |

Integration details and warnings contribute to classification but do not independently override the resulting exit code. An optional Desktop integration may therefore be `unavailable` or `could not verify` while top-level status remains `healthy` with exit `0`. Verbose mode never changes the code.

The alternatives were returning `1` for intentional disablement, which would turn a coherent operator-selected state into failure, or returning `0` whenever inspection completes, which would make `status` useless as a health check.

**Resolved**

The operator selected option A and approved the matrix above. Status exit behavior is derived only from the top-level `OverallStatus` after evidence classification; warnings and per-integration rendering do not create a second exit-code policy.

**Rationale**

The matrix preserves current automation semantics: coherent enabled and intentionally disabled installations succeed, while absence and operational faults fail. Making the contract explicit prevents the richer integration vocabulary from accidentally changing scripts, and it guarantees default and verbose invocations return the same code.

### Q23: How should lifecycle failures, safety refusals, and partial outcomes be bounded?

**Status:** Partially superseded by [Q27](#q27-should-the-implementation-architecture-be-simplified-without-changing-user-facing-output). The closed copy, safety, partial-outcome, retry, channel, and exit contracts remain active; the original orthogonal implementation axes do not.

**Recommended**

Use a closed typed catalog for `setup`, `update`, `enable`, `disable`, and `uninstall` command-boundary outcomes. Do not create one variant per low-level Rust error, but do not accept arbitrary user-facing strings either. The ordinary problem catalog is:

- `The latest release could not be retrieved.`
- `The downloaded release could not be verified.`
- `The installed Codex Session Control state could not be verified.`
- `The installation files could not be updated.`
- `The service could not be configured.`
- `The service could not be started.`
- `The service could not be stopped.`
- `The service state could not be verified.`
- `Codex CLI integration could not be updated.`
- `Codex Desktop integration could not be updated.`
- `Active tasks could not be checked safely.`
- `The operation could not safely continue from this terminal.`
- `Cleanup could not be completed safely.`
- `The operation failed unexpectedly.`

Ordinary recovery uses one exact form:

```text
Try again:
  {command}
```

```text
Check the service logs:
  journalctl --user -u codex-session-control.service
```

```text
Repair Codex Session Control:
  {setup_command}
```

```text
Check what needs attention:
  {status_command}
```

Safety-preserving recovery uses closed typed forms rather than being collapsed into retry or status:

```text
Run the command from an independent terminal:
  {command}
```

```text
Run the update from an interactive terminal:
  {update_command}
```

```text
From an independent terminal, stop the service and try again:
  {stop_or_disable_command}
  {command}
  {conditional_enable_command}
```

The enable command and its line are rendered only when this recovery sequence disabled a service that was enabled before the failed operation. If the service was already disabled, the line is omitted entirely.

```text
Complete Codex CLI cleanup manually:
  {allowlisted_native_cleanup_command}
```

```text
Recover the existing installation with the verified release:
  Release: {allowlisted_release_url}
  Checksums: {allowlisted_checksum_url}
```

Rollback-incomplete failures append the cleanup problem and only the allowlisted managed paths that need inspection:

```text
Cleanup could not be completed safely.

Inspect these managed paths:
  {managed_paths}
```

Terminal partial uninstall is a separate primary outcome:

```text
Codex Session Control was only partially uninstalled.

Cleanup could not be completed safely.

Inspect these remaining managed paths:
  {managed_paths}

Do not rerun `codex-session-control uninstall`; the installed identity has already been removed.
```

Declining the active-task update gate is a cancellation rather than an unexpected failure:

```text
Codex Session Control was not updated.

The update was canceled before installation files changed.
```

Every current lifecycle failure producer must select one closed `UserFailure` variant that fully determines its problem, recovery, partial-state semantics, and retry safety. Ordinary cases infer the approved problem from the existing command-local stage and share one ordinary retry form; only genuinely different safety, manual, partial, terminal-uninstall, and cancellation branches receive explicit variants. The unexpected fallback is allowed only when no known category applies: retry-safe pre-mutation failures may use `Try again`, unknown post-mutation state must use a partial/safety variant, and terminal uninstall can never suggest retry. Dynamic commands, managed paths, versions, targets, and release URLs are allowlisted typed values rather than arbitrary strings.

All ordinary failures, safety refusals, partial outcomes, noninteractive gates, and declined gates write no stdout, render on stderr, and exit `1`. Verbose output remains additive and never changes the code. Q7 continues to govern status findings, Q13 governs the wrapper, Clap owns parser errors, and `mcp-server` remains a protocol surface.

**Resolved**

The operator approved the revised bounded design after a Sol/xhigh review found that the initial four recovery forms erased safety-critical independent-terminal, interactive-terminal, manual-cleanup, partial-completion, and deliberate-cancellation semantics. The approved catalog and templates above incorporate those corrections without turning every technical producer into user-facing prose.

The active-task confirmation prompt itself remains a separate copy decision; this entry governs its noninteractive refusal and deliberate-decline outcomes.

**Rationale**

The correct abstraction boundary is user action and mutation safety, not lifecycle stage and not arbitrary error text. Most failures fit a small ordinary catalog, but current code deliberately uses specialized recovery to avoid self-termination, unsafe service mutation, lost manual cleanup, and unrecoverable retry loops. A bounded `UserFailure` enum with explicit exceptional variants preserves those protections without creating independent type axes or invalid combinations.

### Q24: Should the active-task update confirmation prompt be rewritten?

**Recommended**

The agent proposed shortening the existing prompt and removing internal thread identifiers:

```text
Installing this update requires restarting the shared app-server.

The following active tasks will be interrupted:
- {title}

Running turns will stop and be marked as interrupted.
Goals will remain active and may continue when these tasks reconnect, including immediately for tasks already open in a Codex client.

Pause any goal you do not want to continue before updating.

Continue with the update? [y/N]
```

The alternative was preserving the existing prompt exactly.

**Resolved**

The operator selected option B: keep the existing prompt unchanged because it was explicitly designed and reviewed in an earlier agent session:

```text
Codex session control must restart its app-server to install this update.

This will interrupt {count} active tasks:
- {title} ({thread_id})

Their running turns will stop and be recorded as interrupted.

Goals will not be paused or cleared. Restart alone will not continue them, but
an active goal will start a new turn when a Codex client resumes its task.
This can happen immediately if the task is already open.

Pause any goal you do not want to continue before updating.

Continue and interrupt active work? [y/N]
```

Q23 still governs the surrounding exceptional outcomes: noninteractive execution explains that approval requires an interactive terminal, while a deliberate `no` renders the approved cancellation message.

**Rationale**

The current prompt is intentionally more explicit than the proposed rewrite about task identity, interruption recording, goal persistence, restart behavior, and the possibility of immediate continuation. Those details were previously designed as safety-critical copy. Rewriting an already-approved manual gate would create churn and risk weakening a deliberate warning without advancing the CLI-output goal.

### Q25: What should `disable` report when the service stopped but Desktop cleanup failed?

**Recommended**

Do not use Q14's `Codex Session Control could not be stopped.` headline after the service was successfully stopped, automatic startup was disabled, and only later Desktop cleanup failed. Use this partial outcome instead:

```text
Codex Session Control is stopped and will not start automatically.

Codex Desktop integration could not be removed safely.
Your Codex data is unchanged.

Complete the remaining cleanup:
  codex-session-control disable
```

If retry is insufficient and exact managed paths are safely known, append:

```text
Managed paths requiring attention:
  {managed_paths}
```

This block overrides Q14 for the stopped-with-Desktop-cleanup-incomplete state, writes no stdout, renders on stderr, and exits `1`.

**Resolved**

The operator selected option A and approved the truthful partial outcome above. The generic `could not be stopped` failure remains valid only when stopping or verifying the stopped state did not complete.

**Rationale**

The command has two sequential responsibilities: stop and disable the service, then remove Desktop integration state. When the first responsibility is authoritatively complete, reporting total stop failure is false and can prompt unnecessary service troubleshooting. The partial outcome preserves the successful state change, names the remaining problem, and gives an idempotent cleanup command without implying that Codex data was removed.

### Q26: How should `setup` and `enable` compose generic and detected-client guidance?

**Recommended**

Avoid duplicate CLI and Desktop actions by composing successful stdout from non-empty blocks in this order, with exactly one blank line between adjacent blocks:

1. Primary command outcome.
2. CLI guidance.
3. Desktop guidance.

The initial proposal let Q20's detected-client guidance replace the matching generic Q3/Q10 paragraph. A Sol/xhigh review found two missing constraints: Desktop availability must be classified before restart guidance, and CLI detection must be independent of Desktop availability.

**Resolved**

The operator approved the reviewed matrix.

CLI guidance:

| Command | Running unattached CLI detected | CLI block |
| --- | --- | --- |
| `setup` | yes | Q20 detected-running CLI block; replace Q3's generic CLI paragraph |
| `setup` | no | Q3 generic CLI paragraph |
| `enable` | yes | Q20 detected-running CLI block |
| `enable` | no | no CLI block |

Running-CLI detection is independent of every Desktop fact. The implementation must move it outside the current shared Desktop-availability condition or otherwise produce two independent typed facts.

Desktop guidance selects the first matching row:

| Desktop state | Running unattached Desktop detected | Configuration changed | Desktop block |
| --- | --- | --- | --- |
| setup required (`enable` only) | either | no | Q10 unavailable/setup block; never render restart guidance |
| unavailable or unverified | either | no | no stdout restart block; retain the applicable Q15 Desktop warning on stderr |
| available/configured | yes | either | Q20 detected-running Desktop block; replace the generic restart paragraph |
| available/configured | no | yes | Q3 generic restart paragraph for `setup` or Q10 generic restart paragraph for `enable` |
| available/configured | no | no | no Desktop block |

`changed + unavailable/setup-required` is an invalid typed state. When both detected-client blocks render, CLI appears first and Desktop second. The same order applies when detected CLI guidance is followed by Q10's Desktop setup-required block. Never render two blocks that prescribe the same client action.

Q20 blocks are stdout client guidance, not Q15 notices. Write and flush the complete stdout composition first; then render applicable Q15 compatibility, Desktop-discovery, and PATH warnings/notices on stderr. Those warnings remain additive and are never replaced or reordered by this matrix.

**Rationale**

Specific detected-client guidance explains why an existing process did not switch, while generic guidance remains correct when no such process was found. Desktop restart is valid only after CSC has a usable configured target; when setup is required or evidence is unavailable, restart guidance would be false and could suppress the real recovery action. Independent CLI detection prevents an unrelated Desktop state from hiding required CLI guidance, and fixed block order makes exact-output tests deterministic.

### Q27: Should the implementation architecture be simplified without changing user-facing output?

**Recommended**

Accept the fresh Sol/xhigh YAGNI review's smallest clean architecture while preserving every approved default message, warning, status state, exit code, safety refusal, partial outcome, active-task prompt, channel, and ordering rule:

- Start with the two focused output responsibilities in `src/cli_output.rs` and `src/diagnostics.rs`; split further only when a concrete responsibility boundary makes the code clearer, under Q30's ordinary-review policy.
- Let `cli_output` own one non-generic `UserSuccess` enum, one bounded `UserFailure` enum, direct exhaustive renderers, and one final `RenderedCli { stdout, stderr, exit_code }` value.
- Give `UserFailure` one ordinary form plus explicit variants only for genuinely different independent-terminal, interactive-terminal, stop-then-retry, manual-cleanup, verified-release, rollback-incomplete, partial-disable, terminal-partial-uninstall, and cancellation behavior. Do not model problem, recovery, partial state, and retry safety as four independent axes.
- Use one concrete `Diagnostics` type with private off, stderr, and test-record modes. Do not add a `DiagnosticSink` trait or production backend hierarchy.
- Retain existing command-local stage and evidence types rather than introducing a global stage enum or retyping every error.
- Keep current internal status inspection states and replace only the final rendered/boolean collapse with one typed `StatusResult`. Do not add parallel `StatusSnapshot` and `StatusView` evidence hierarchies.
- Render directly from fixed enums. Do not add generic `CommandReport<T>`, command-specific report traits, builders, fragment trees, message IDs, localization catalogs, serialization, callbacks, channels, background tasks, `tracing`, or a general UI framework.
- Keep the original `ControllerError` only as an in-memory technical source and project it into allowlisted diagnostic categories at the command boundary.

The alternative was retaining Q18/Q23's broader typed architecture, which a fresh review estimated could add roughly 1,400–2,530 production lines and 1,600–3,100 test/documentation lines before gross migration churn.

**Resolved**

The operator approved the minimal architecture enthusiastically after confirming that it preserves essentially the same user-facing messages. The architecture changes only internal Rust structure. Q28 separately resolves the breadth of initial verbose evidence.

**Rationale**

The CLI has seven fixed commands and one human renderer. Typed boundaries are justified where they protect exact copy, safety decisions, diagnostic privacy, status classification, and update process ownership, but generic report families and polymorphic sink infrastructure provide no second consumer or production backend. One renderer, one concrete diagnostic emitter, one final status result, and one bounded failure enum retain the useful separation while preventing a copy refactor from becoming a framework.

### Q28: Should the initial verbose contract cover every internal evidence category?

**Recommended**

Narrow Q16's broad evidence promise to the smallest set that answers concrete debugging questions without creating a second product surface:

- command and update phase;
- controller or candidate identity;
- selected Codex home;
- completed stage;
- failed stage plus an allowlisted cause category;
- service or restart decisions only when they change execution;
- actionable managed cleanup paths;
- final component evidence for `status`.

Representative output remains:

```text
[verbose] setup: controller {version} ({target})
[verbose] setup: selected Codex home {codex_home}
[verbose] setup: completed preflight
[verbose] setup: completed binary
[verbose] setup: completed service-enable
[verbose] setup: failed service-verify (service state could not be verified)
```

Additional diagnostic events require a named support or debugging question that the existing set cannot answer, plus the same privacy classification and typed-event tests. The alternative was retaining Q16's preemptive promise to emit projection, plugin, Desktop, service, and other evidence on every applicable path, which would make verbose output a second large rendering and test system.

**Resolved**

The operator approved option A after confirming that this is the fresh Sol/xhigh simplicity reviewer's recommendation. The initial diagnostic set is demand-driven rather than exhaustive.

The following Q16 contracts remain unchanged:

- chronological streaming with `[verbose] <command>[/<phase>]:` prefixes;
- explicit `update/outer` and `update/apply` phases;
- typed allowlisted evidence and the absolute privacy deny-list;
- identical default stdout, default-visible stderr, and exit behavior;
- default-visible warnings, recovery, manual gates, and cleanup actions;
- successful wrapper silence and MCP protocol stdout isolation.

**Rationale**

Stage localization, safe cause classification, execution-changing decisions, cleanup paths, and final status evidence cover the demonstrated support needs. Preemptively modeling every internal projection or plugin fact would add event variants, emit calls, and tests without a concrete consumer. Demand-driven expansion preserves debugging value while keeping the diagnostic module proportional.

### Q29: How should tests cover the output refactor without a Cartesian explosion?

**Recommended**

Use risk-based table coverage:

- one exact renderer case per distinct output block or `UserFailure` variant;
- one table-driven Q26 precedence test;
- one table-driven status classification and exit-code test;
- one table-driven privacy sentinel test that exercises every diagnostic constructor;
- one staged-update subprocess test for phase labels, ownership, and exit propagation;
- one successful-wrapper silence test;
- one MCP stdout-isolation test;
- existing operational failure-injection loops for stage ordering, mutation safety, cleanup, and retry behavior, changing only their presentation assertions where necessary.

Add interaction cases only where two dimensions actually change each other's behavior. Explicitly prohibit a command × problem × recovery × verbosity × service state × Desktop state test product, and do not duplicate filesystem, systemd, mutation-order, or retry fixtures solely to reassert rendering.

The alternative was exhaustive combination coverage, which would mostly test the presentation model against itself and obscure the stronger existing operational safety tests.

**Resolved**

The operator selected option A with the explicit instruction not to be excessive with testing. Tests must prove every distinct user-facing and safety behavior, but line count, scenario count, or combinatorial completeness is not a quality goal.

**Rationale**

The repository already has thousands of lines of lifecycle tests covering real mutations and failure injection. Renderer tables should verify exact copy and selection logic once, while existing tests continue proving operational semantics. Cross-products of independent dimensions add maintenance cost without discovering proportionate interaction risk.

### Q30: What complexity guardrails should pause implementation?

**Recommended**

Do not use module count, type count, file length, or named patterns such as builders, traits, or serialization as automatic stop triggers. Focused module splitting can improve clarity, and any pattern can be proportionate when it solves a concrete need.

Use these as design preferences evaluated through ordinary code review:

- prefer direct enums and exhaustive matches;
- add abstractions only for a concrete repeated need;
- split code into focused modules when that improves responsibility boundaries;
- keep tests risk-based under Q29;
- challenge disproportionate structure during normal spec, plan, and code review.

Require an operator pause only when:

- approved UX, safety, privacy, lifecycle behavior, or output contracts must change;
- actual net growth exceeds roughly 1,500 production lines or 1,800 test/documentation lines;
- implementation needs a new dependency, background or concurrent subsystem, or cross-process serialization protocol;
- work expands into unrelated MCP, app-server, or domain models.

Crossing a line-count threshold is a review trigger rather than automatic rejection. Implementation does not stop repeatedly for projections, individual modules, types, file sizes, or pattern choices.

**Resolved**

The operator approved the narrower guardrails after rejecting the reviewer's blunt third-module trigger and the agent's initial broad list of prohibited implementation patterns. Focused clean modules are welcome; the goal is to stop scope creep and disproportionate systems, not force code into two files or ban tools by name.

**Rationale**

Mechanical architecture limits can make code worse by encouraging oversized files, hidden coupling, or avoidance of an otherwise appropriate local pattern. Actual behavioral change, measured growth, runtime machinery, new dependencies, cross-process protocols, and unrelated subsystem expansion are stronger signals of material scope. This policy provides an exceptional escalation gate without interrupting routine implementation decisions.

### Q31: Should the specification carry the full brainstorming chronology?

**Recommended**

Keep this brainstorming artifact intact as the decision history, but make the next specification synthesize only active requirements:

- exact user-facing copy;
- status and exit-code matrix;
- setup/enable guidance composition;
- lifecycle safety, refusal, cancellation, and partial outcomes;
- verbose scope, channels, phases, privacy, and parity;
- minimal architecture from Q27;
- risk-based testing from Q29;
- narrow complexity escalation policy from Q30.

Do not copy superseded Q7, Q9, Q16, Q18, or Q23 material into the implementation specification. Keep architecture constraints short and link this artifact and its review trace for provenance instead of reproducing the full debate.

The alternative was carrying the complete chronological Q&A into the specification, which would make obsolete alternatives look active and turn implementation into document archaeology.

**Resolved**

The operator selected option A and explicitly confirmed that superseded decisions must be clearly marked under the brainstorming skill's cleanup rule. The brainstorming artifact remains the audit trail; the specification becomes the concise active source for implementation.

**Rationale**

Brainstorming preserves why decisions changed, while a specification must state only what to build. Mixing those roles makes superseded vocabulary and provisional architecture easy to reintroduce. Clear inline supersession markers plus an active-only current-decision index preserve provenance without forcing later agents to infer which historical text still governs.
