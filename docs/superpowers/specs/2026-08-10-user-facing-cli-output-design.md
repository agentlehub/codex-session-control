# User-Facing CLI Output Design Specification

**Status:** approved
**Source:** [Approved brainstorming artifact](../brainstorming/2026-08-09-user-facing-cli-output.md)
**Review:** [Specification review trace](../reviews/2026-08-10-user-facing-cli-output-design-review.md)
**Next:** [Writing plans handoff](../../handoffs/2026-08-10-103026-user-facing-cli-output-writing-plans.md)

## Objective

Replace Codex Session Control's machine-oriented human CLI receipts with the approved concise user-facing output while preserving lifecycle safety, operational behavior, diagnostic usefulness, exit semantics, wrapper silence, staged-update ownership, the active-task update gate, and MCP stdio isolation.

This specification is the active implementation contract. The source brainstorming artifact and its [passed review trace](../reviews/2026-08-10-user-facing-cli-output-brainstorming-review.md) retain rationale and superseded history.

The source returned from the first specification review to close two material gaps. Its approved [Return From Spec Writing](../brainstorming/2026-08-09-user-facing-cli-output.md#return-from-spec-writing) addendum supplies the abnormal staged-candidate result and confirms producer-level failure mapping as technical specification work under Q23/Q27; this revision incorporates that return.

## Prerequisites

- Work from a dedicated Codex Session Control worktree whose `HEAD` contains approved source commit `3d73ce13eda5cf9052e817617d736faac48ca719` and whose base is `528ef31566048520617929a45ea446ff9af70559` (`origin/main` when this specification was written).
- Before implementation, confirm the approved spec and review trace are unchanged, the intended implementation branch/worktree is exact, and the worktree has no unrelated modifications.
- Use Rust `1.95.0` with `rustfmt` and `clippy`, plus the repository tools required by `./scripts/check.sh`: Bash, POSIX `sh`, `shellcheck`, `jq`, and `actionlint 1.7.12`.
- Preserve the existing selected-home, filesystem ownership, systemd-user, Desktop descriptor, release verification, active-task, and retry-safety contracts. This output refactor does not authorize weakening their tests or behavior.
- No new dependency, background task, concurrent diagnostic subsystem, or cross-process serialization protocol is approved.
- The Operator's 2026-08-10 unattended directive approves this specification after a passing repaired review gate; that condition is satisfied. **[MANUAL]** The later implementation plan still requires approval before production or test code changes begin.

## Scope

- Root and subcommand help for the seven human-facing commands.
- Default success, notice, warning, failure, refusal, cancellation, partial-outcome, and status output.
- A global human-readable `--verbose` diagnostic mode for user commands.
- One typed human rendering boundary and one concrete diagnostic emitter.
- Typed final status classification and the approved status exit-code matrix.
- Staged-update outer/apply diagnostic and result ownership.
- Successful and failed `codex` wrapper behavior.
- Preservation of the active-task update prompt and its gate semantics.
- Preservation of `mcp-server` stdout as protocol-only.
- Focused risk-based output, diagnostic, classification, subprocess, wrapper, protocol, and existing operational regression tests.
- Public documentation that directly describes the changed help or status vocabulary.

## Non-Goals

- No localization/message catalog, generic output AST, report framework, builder family, command-specific rendering trait hierarchy, or serialized presentation protocol.
- No rewrite of `ControllerError`, the lifecycle stage machines, status evidence collection, systemd operations, Desktop discovery, release mechanics, MCP/domain models, or app-server behavior beyond the narrow typed projections required at the CLI boundary.
- No guarantee of a total merged ordering after a user redirects stdout and stderr independently. The enforceable contract is program write/flush order and exact content within each channel.
- No machine-stable verbose schema. Verbose output is diagnostic prose and may evolve when a concrete support question justifies a new allowlisted event.
- No shell detection, shell-profile mutation, or supposedly universal persistent PATH command.
- No Cartesian test product across commands, problems, recoveries, verbosity, service state, and Desktop state.
- No change to the existing active-task confirmation prompt.
- No direct user workflow for `mcp-server`; it remains callable for the installed plugin but hidden from human help.

## Design Boundaries

### Human rendering

Create `src/cli_output.rs` as the sole owner of final human prose. It owns:

- one non-generic `UserSuccess` enum for the fixed command success/status outcomes;
- one bounded `UserFailure` enum with an ordinary form and explicit variants only for materially different safety, partial, manual-cleanup, verified-release, terminal-uninstall, staged-update completion-unknown, and cancellation behavior;
- typed notices and the independent client/process facts needed by the approved composition rules;
- one typed `StatusResult` produced from the existing status evidence before the current boolean/string collapse, with narrow evidence-strength projections retained at existing inspection sites that currently erase decisive-fault versus inconclusive-result distinctions;
- direct exhaustive rendering into one final `RenderedCli { stdout, stderr, exit_code }` value.

Dynamic values are typed and allowlisted: versions, command paths, managed paths, release/checksum URLs, Desktop/CLI facts, integration states, and known diagnostic categories. Arbitrary user-facing problem or recovery strings are not accepted by the renderer.

`src/main.rs` is the final writer for buffered human results. It writes and flushes the complete stdout result before default-visible stderr notices on success. On failure it writes one complete friendly block to stderr and returns its typed exit code. The active-task prompt remains the sole mid-operation direct write because it must flush before reading input.

### Diagnostics

Create `src/diagnostics.rs` with one concrete `Diagnostics` type and private off, stderr, and test-record modes. Do not add a production trait/backend hierarchy.

Command code emits owned typed events using existing command-local stages and evidence types. The concrete emitter projects events to allowlisted diagnostic fields and performs best-effort synchronous writes. Its first write failure disables further diagnostic output and must not change mutation state, the normal result, or the exit code.

Initial diagnostic events are demand-driven and limited to:

- command and update phase;
- controller or candidate version/target identity;
- selected Codex home;
- completed stage;
- failed stage and an allowlisted cause category;
- service or restart decisions only when they change execution;
- actionable allowlisted managed cleanup paths;
- final component evidence used by `status`.

Additional events require a named support/debugging question not answered by that set, structural privacy classification, and typed-event tests.

### Command ownership

The command modules continue to own operational decisions and mutation ordering:

- `setup`, `enable`, and `disable` produce the exact typed outcome, Desktop state, configuration-change state, independent running CLI fact, independent running Desktop fact, and applicable warnings.
- `update` produces applied/already-current state, version, service enablement, Desktop configuration-change state, and exceptional safety/cancellation outcomes.
- `uninstall` produces complete or terminal-partial outcomes and allowlisted remaining paths.
- `status` retains the current read-only operations and evidence sources, preserves narrow typed evidence strength at existing inspection sites, and replaces the final rendered/boolean collapse with one typed result and independent CLI/Desktop classifications. This does not authorize a parallel snapshot/view hierarchy or a new evidence subsystem.
- `codex` returns no success report because successful `exec` must remain silent.

Existing command-local stage and evidence types remain authoritative. Do not introduce a global stage enum, duplicate status evidence hierarchy, generic `CommandReport<T>`, or parallel snapshot/view model.

## Help Contract

Root help must render exactly:

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

`mcp-server` remains callable but hidden from root help. Each visible subcommand reuses its root-help description. `setup`, `update`, `status`, `enable`, `disable`, and `uninstall` expose the global `--verbose` option before or after the subcommand. `codex` preserves the passthrough boundary: CSC verbosity is accepted only before `codex`; every argument after `codex` belongs to native Codex.

The detailed `setup` help must include:

```text
Install Codex Session Control and start the shared app-server

Usage: codex-session-control setup [OPTIONS]

Options:
      --desktop-launcher <PATH>  Absolute path to the Codex Desktop executable when automatic discovery fails
      --verbose                  Show diagnostic details
  -h, --help                     Print help
```

The detailed `codex` help must include:

```text
Start Codex CLI through the shared app-server

Usage: codex-session-control codex [ARGS]...

Arguments:
  [ARGS]...  Arguments passed directly to Codex CLI

Options:
  -h, --help  Print help
```

The other visible subcommands use `Usage: codex-session-control <command> [OPTIONS]`, the shared description, `--verbose`, and `-h, --help`; they add no unapproved public controls.

## Default Success Contract

### Setup

The primary setup output is:

```text
Codex Session Control {version} is ready.
```

Successful stdout then composes non-empty blocks in this order with exactly one blank line between adjacent blocks:

1. Primary outcome.
2. CLI guidance.
3. Desktop guidance.

CLI guidance is independent of all Desktop facts:

| Running unattached CLI | CLI block |
| --- | --- |
| yes | Use the detected-running CLI block below instead of generic guidance. |
| no | Use the generic CLI block below. |

Generic CLI block:

```text
To use Codex Session Control with Codex CLI, start the CLI with:
  codex-session-control codex
```

Detected-running CLI block:

```text
Codex CLI is already running without Codex Session Control.
Exit it, then start it with:
  codex-session-control codex
```

Desktop guidance selects the first matching row:

| Desktop state | Running unattached Desktop | Configuration changed | Desktop block |
| --- | --- | --- | --- |
| unavailable or could not verify | either | no | No stdout restart block; keep the applicable default-visible Desktop warning. |
| available/configured | yes | either | Detected-running Desktop block. |
| available/configured | no | yes | Generic setup Desktop block. |
| available/configured | no | no | No Desktop block. |

Generic setup Desktop block:

```text
If Codex Desktop is already running, restart it to make Codex Session Control available there.
```

Detected-running Desktop block:

```text
Codex Desktop is already running without Codex Session Control.
Restart Codex Desktop to use Codex Session Control there.
```

`configuration changed + unavailable/could not verify` is an invalid typed state. Never render duplicate guidance for the same client.

### Update

Applied update while the service remains enabled:

```text
Codex Session Control was updated to {version}.

Start a new task to use the updated plugin.
```

Already current while the service remains enabled:

```text
Codex Session Control {version} is already up to date.
```

Applied update while the service remains disabled replaces the enabled-service follow-up with:

```text
Codex Session Control was updated to {version}.

The service remains disabled. Run `codex-session-control enable` when you want to use it.
```

Already-current update while the service remains disabled is:

```text
Codex Session Control {version} is already up to date.

The service remains disabled. Run `codex-session-control enable` when you want to use it.
```

An applied update whose Desktop configuration changed appends:

```text
If Codex Desktop is already running, restart it to use the updated version of Codex Session Control.
```

An already-current result has no plugin-restart sentence or Desktop-change sentence unless the operation actually made that change.

### Enable

Primary outcome:

```text
Codex Session Control is running and will start automatically.
```

Composition uses primary outcome, CLI guidance, then Desktop guidance with one blank line between blocks. CLI detection remains independent of Desktop state:

| Running unattached CLI | CLI block |
| --- | --- |
| yes | The detected-running CLI block from setup. |
| no | No CLI block. |

Desktop guidance selects the first matching row:

| Desktop state | Running unattached Desktop | Configuration changed | Desktop block |
| --- | --- | --- | --- |
| setup required | either | no | Setup-required block; never restart guidance. |
| unavailable or could not verify | either | no | No stdout restart block; keep the applicable default-visible Desktop warning. |
| available/configured | yes | either | Detected-running Desktop block. |
| available/configured | no | yes | Generic enable Desktop block. |
| available/configured | no | no | No Desktop block. |

Setup-required block:

```text
Codex Desktop integration is unavailable.
Run `codex-session-control setup` to set it up.
```

Generic enable Desktop block:

```text
If Codex Desktop is already running, restart it to make Codex Session Control available there.
```

When both detected-client blocks render, CLI precedes Desktop. The same order applies when detected CLI guidance precedes the setup-required block. Never render duplicate client actions.

### Disable

```text
Codex Session Control is stopped and will not start automatically.

Your Codex data is unchanged.
If Codex Desktop is already running, restart it to continue without Codex Session Control.
```

The Desktop sentence appears only when CSC Desktop configuration was removed. The Codex data statement always remains.

### Uninstall

```text
Codex Session Control was uninstalled.

Your Codex data is unchanged.
If Codex Desktop is already running, restart it to continue without Codex Session Control.
```

The Desktop sentence appears only when uninstall removed CSC Desktop configuration. The Codex data statement always remains.

### Codex wrapper

Successful wrapper preflight and `exec` emit no CSC stdout or stderr bytes, including with `codex-session-control --verbose codex`.

## Status Contract

`status` writes its inspection report to stdout and remains read-only. It must not create directories, publish/remove descriptors, start/restart clients, mutate systemd state, repair files, or connect to a socket that failed the existing ownership/type/mode safety checks.

Top-level output forms are:

Healthy:

```text
Status: healthy
Version: {version}
Service: running, starts automatically
Codex CLI integration: {cli_state}
Codex Desktop integration: {desktop_state}
```

Disabled:

```text
Status: disabled
Version: {version}
Service: stopped, automatic startup is off
Codex CLI integration: unavailable
Codex Desktop integration: unavailable

Run `codex-session-control enable` to start Codex Session Control.
```

Not installed:

```text
Status: not installed
Codex CLI integration: unavailable
Codex Desktop integration: unavailable

Install Codex Session Control by running:
  codex-session-control setup
```

Unhealthy retains the same summary rows where evidence is available, followed by `Problems:`. Each problem uses readable user prose and a verb-led action. Problems with the exact same action may be grouped only when the relationship remains unambiguous; different actions remain adjacent to their problem. A configured shared-service failure is:

```text
Status: unhealthy
Version: {version}
Service: stopped unexpectedly, automatic startup is on
Codex CLI integration: unhealthy
Codex Desktop integration: unhealthy

Problems:
- The service is configured to run but is stopped.
- The service connection is unavailable.

Check the service logs for both problems:
  journalctl --user -u codex-session-control.service
```

Integration states are mutually exclusive:

- `ready`: affirmative current evidence says the integration is usable.
- `unavailable`: CSC is not installed, intentionally disabled, or cannot provide an optional integration because no target was configured/discovered; it does not claim Desktop itself is absent.
- `unhealthy`: the integration is expected to work, but decisive evidence proves a fault prevents correct operation.
- `could not verify`: evidence is inconclusive.

CLI and Desktop are classified independently. CLI classification uses installation, native registration, service, socket, and app-server evidence. Desktop classification uses its configuration/descriptor evidence plus the shared service chain. Top-level `unhealthy` does not overwrite a valid per-integration fact: a Desktop-only problem may coexist with CLI `ready`, and optional unconfigured Desktop remains `unavailable`. Optional Desktop `unavailable` or `could not verify` may coexist with top-level `healthy`.

Inspection code that currently collapses `Result` failure, malformed output, absence, and decisive mismatch into one boolean must retain only the minimum typed evidence strength needed to classify `unhealthy` versus `could not verify`. That projection occurs at the existing inspection site and feeds the single `StatusResult`; it must not duplicate filesystem, native, service, socket, app-server, or Desktop evidence into a second hierarchy.

`ready` is an affirmative inspection snapshot, not a guarantee against a later race.

Status exit codes depend only on the final top-level state:

| Top-level status | Exit code |
| --- | ---: |
| `healthy` | `0` |
| `disabled` | `0` |
| `not installed` | `1` |
| `unhealthy` | `1` |

Warnings and per-integration states inform classification but do not independently override the final exit code. Verbose mode never changes it.

## Warnings and Notices

Default-visible success material is written and flushed to stdout before applicable warnings/notices are written to stderr.

Compatibility warning:

```text
Warning: Codex {codex_version} has not been tested with Codex Session Control {version}.
Some features may not work as expected.
```

Desktop discovery warning:

```text
Codex Desktop integration is unavailable because a compatible Desktop launcher was not found.
```

PATH notice:

```text
Note: `{local_bin}` is not on your PATH.
Add it to your PATH to use the short `codex-session-control` command.
```

Warnings, manual gates, recovery instructions, partial-cleanup warnings, and paths the user must inspect remain default-visible. Verbose diagnostics never replace them.

## Failure, Safety, and Partial-Outcome Contract

`setup`, `update`, `enable`, `disable`, and `uninstall` failures use command-specific headlines:

```text
Codex Session Control could not be installed.
Codex Session Control could not be updated.
Codex Session Control could not be started.
Codex Session Control could not be stopped.
Codex Session Control could not be uninstalled.
```

Each ordinary failure then selects exactly one problem from this closed catalog:

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

Each ordinary failure selects one closed recovery form:

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

Known safety-preserving branches use explicit typed recovery variants:

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

The enable line is present only when the failed recovery disabled a service that was enabled before the operation.

```text
Complete Codex CLI cleanup manually:
  {allowlisted_native_cleanup_command}
```

```text
Recover the existing installation with the verified release:
  Release: {allowlisted_release_url}
  Checksums: {allowlisted_checksum_url}
```

Rollback-incomplete failures append:

```text
Cleanup could not be completed safely.

Inspect these managed paths:
  {managed_paths}
```

Only allowlisted managed paths may render.

When `disable` authoritatively stopped the service and disabled automatic startup but Desktop cleanup remained incomplete, override the total-failure headline with:

```text
Codex Session Control is stopped and will not start automatically.

Codex Desktop integration could not be removed safely.
Your Codex data is unchanged.

Complete the remaining cleanup:
  codex-session-control disable
```

Append the following only when retry is insufficient and exact managed paths are safely known:

```text
Managed paths requiring attention:
  {managed_paths}
```

Terminal partial uninstall is:

```text
Codex Session Control was only partially uninstalled.

Cleanup could not be completed safely.

Inspect these remaining managed paths:
  {managed_paths}

Do not rerun `codex-session-control uninstall`; the installed identity has already been removed.
```

Declining the active-task update gate is a cancellation:

```text
Codex Session Control was not updated.

The update was canceled before installation files changed.
```

When a staged candidate was successfully started but the outer process cannot observe a normal exit `0` or `1`, the outer process selects the single `UpdateCompletionUnknown` result. It writes only this exact block to stderr, exits `1`, and never recommends immediate retry:

```text
Codex Session Control could not confirm that the update completed.

The installed Codex Session Control state could not be verified.

Check what needs attention:
  codex-session-control status
```

The fallback `The operation failed unexpectedly.` is allowed only when no known category applies. A retry-safe pre-mutation failure may use `Try again`; unknown post-mutation state must use a partial/safety variant; terminal uninstall must never suggest retry.

All ordinary failures, refusals, partial outcomes, noninteractive gates, and cancellations write no stdout, write the complete block to stderr, and exit `1`. Clap retains ownership of parser errors and their exit code.

Wrapper preflight/launch failure is separate and exact:

```text
Codex CLI could not start because Codex Session Control is unavailable.

Check what needs attention:
  codex-session-control status
```

Failed wrapper preflight may prepend/add safe verbose diagnostics on stderr; it must not leak the current raw `ControllerError` text into diagnostics.

## Producer-Boundary Failure Classification

The following tables exhaustively classify current lifecycle failure producers. A row may group branches only when they select the same complete bounded `UserFailure` variant. The producer selects that variant from its semantic operation and the safety evidence available at that boundary; the renderer does not combine independent problem, recovery, mutation, or retry axes. Command-local stages remain diagnostic facts and never select user copy. Raw error strings are never parsed.

### Setup producers

| Producer boundary | Direct bounded result |
| --- | --- |
| `setup` / `lifecycle_context`: effective-user paths, `PATH`, or current-directory refusal | `Ordinary` — `The operation could not safely continue from this terminal.` / `Try again: codex-session-control setup`. |
| `setup`: controller `current_exe` unavailable | `Ordinary` — `The operation failed unexpectedly.` / `Try again: codex-session-control setup`. |
| `setup_preflight` and `validate_manifestless_setup_artifacts`: selected-home, manifest, native evidence, or validation-race ambiguity | `Ordinary` — `The installed Codex Session Control state could not be verified.` / `Check what needs attention: codex-session-control status`. |
| `validate_manifestless_setup_artifacts`: coherent different release | `Ordinary` — `The installation files could not be updated.` / `Try again: codex-session-control update`. |
| `validate_manifestless_setup_artifacts`: exact older release with a validated same-release installed binary | `Ordinary` — `The installed Codex Session Control state could not be verified.` / `Repair Codex Session Control:` with that validated binary's `setup` command. |
| `validate_manifestless_setup_artifacts`: exact older release without a safe matching installed binary | `VerifiedRelease` — installed state unverifiable plus the allowlisted release and checksum URLs. |
| `resolve_codex_executable`, `read_codex_version`, `render_projection`, `reconcile_projection`, `reconcile_marketplace`, and `reconcile_plugin` | `Ordinary` — `Codex CLI integration could not be updated.` / `Try again: codex-session-control setup`; a foreign or unsafe native registration instead uses `Check what needs attention: codex-session-control status`. |
| Candidate read/identity, configuration render, binary/configuration reconciliation, and final manifest serialization/write | `Ordinary` — `The installation files could not be updated.` / `Try again: codex-session-control setup`. |
| `resolve_named_executable("systemctl")` and `render_unit` | `Ordinary` — `The service could not be configured.` / `Try again: codex-session-control setup`. |
| `resolve_setup_desktop`, descriptor preflight, and foreign/unsafe descriptor inspection | `Ordinary` — `Codex Desktop integration could not be updated.` / retry `setup` for render/probe failures, or check `status` for foreign/unsafe evidence. |
| `publish_descriptor` and old-descriptor switch/removal | Proven no managed residue: the ordinary Desktop-integration failure with retry `setup`. Final publication, stage-file residue, old-path state, or cleanup remaining/unverifiable: `RollbackIncomplete` with the same primary failure and only the exact affected managed descriptor/stage paths. |
| Service-unit reconciliation, `daemon-reload`, `systemctl enable --now`, and `verify_setup_service` | When descriptor cleanup is unnecessary or succeeds under the existing inactive-service/absent-socket proof: `Ordinary` with respectively service-configured, service-started, or service-state problem and existing retry (`setup`, or `update` for verification). When changed-descriptor cleanup remains/unverifiable: the corresponding `RollbackIncomplete` result with the exact descriptor path. |
| Test-only completed-stage injections | The matching concrete producer family above; an injection with no production cause uses `Ordinary` — `The operation failed unexpectedly.` / retry `setup`. The stage name remains diagnostic-only. |

### Update producers

| Producer boundary | Direct bounded result |
| --- | --- |
| `main::run`, `update`, or `lifecycle_context`: invalid private marker, context construction, `current_exe`, or installed-executable staged-apply refusal | `Ordinary` — `The operation failed unexpectedly.` / `Try again: codex-session-control update`. |
| `load_update_manifest` and same-version health inspection failure | `Ordinary` — installed state unverifiable / check `status`. |
| Release-target/client construction, release discovery, exact asset selection, temporary download destination, or transfer | `Ordinary` — `The latest release could not be retrieved.` / retry `update`. |
| Checksum parsing/integrity verification | `Ordinary` — `The downloaded release could not be verified.` / retry `update`. |
| `inspect_candidate`, release identity/target/version/hash/downgrade checks, and candidate chmod/read | `Ordinary` — downloaded release unverifiable / retry `update`. |
| Codex executable/version resolution and configuration/projection rendering in outer or apply preflight | `Ordinary` — CLI integration could not be updated / retry `update`. |
| Unit rendering in outer or apply preflight | `Ordinary` — service could not be configured / retry `update`. |
| `service_snapshot` or systemctl resolution with unproven/contradictory service evidence | `Ordinary` — service state unverifiable / check `status`. |
| `inspect_restart` returns unknown | `StopThenRetry` — service state unverifiable; from an independent terminal run `disable`, `update`, then `enable`. The enable line is present because the accepted active snapshot is enabled. |
| `guard_restart_required_update`: self-hosting or caller independence unproven | `IndependentTerminal` — operation cannot safely continue / run `update` independently. |
| `list_active_threads` on the initial check or final recheck | `Ordinary` — active tasks could not be checked safely / retry `update`. |
| `require_restart_approval`: noninteractive terminal or prompt write/read failure | `InteractiveTerminal` — operation cannot safely continue / run `update` interactively. |
| `require_restart_approval`: deliberate non-yes response | `Cancellation` with the exact approved before-mutation copy. |
| `run_candidate_apply`: proven `spawn` failure | `Ordinary` — installation files could not be updated / retry `update`. |
| `run_candidate_apply`: `wait` failure, signal termination, or exit outside `0`/`1` after successful spawn | `UpdateCompletionUnknown` with the exact no-retry block above. Exit `0` or `1` creates no parent `UserFailure`; it is propagated unchanged. |
| Binary/configuration writes | `Ordinary` — installation files could not be updated / retry `update`. |
| Projection/native reconciliation | `Ordinary` — CLI integration could not be updated / retry `update`; foreign/unsafe pre-observation uses check `status`. |
| Persisted Desktop capability, descriptor render/inspect, or foreign descriptor | `Ordinary` — Desktop integration could not be updated / check `status`. |
| `publish_descriptor` | Proven clean publication failure: the ordinary Desktop-integration failure / retry `update`. Final descriptor or stage-file residue remaining/unverifiable: `RollbackIncomplete` with only the exact affected managed paths. |
| Service-unit write or `daemon-reload`; service restart/enable; service verification | `Ordinary` with respectively service-configured, service-started, or service-state problem / check the service logs. |
| Final manifest serialization/write after apply | `Ordinary` — installed state unverifiable / check `status`; never recommend immediate retry because durability is uncertain. |
| Test-only completed-stage injections | Candidate-side injections inherit the matching concrete family. The outer post-candidate injection cannot create a second friendly result after normal candidate exit and is limited to diagnostic/exit-ownership testing. |

### Enable producers

| Producer boundary | Direct bounded result |
| --- | --- |
| `enable` / `lifecycle_context` context failure | `Ordinary` — unexpected failure / retry `enable`. |
| Selected-home/configuration/manifest validation or configured Codex-version failure | `Ordinary` — installed state unverifiable / repair with `setup`. |
| Unit read/render/content validation | `Ordinary` — service could not be configured / repair with `setup`. |
| `resolve_enable_desktop`, descriptor render/preflight, or foreign descriptor | `Ordinary` — Desktop integration could not be updated / check `status`. |
| Systemctl resolution | `Ordinary` — service could not be started / retry `enable`. |
| `publish_descriptor` | Proven no managed residue: ordinary Desktop-integration failure / retry `enable`. Final descriptor or stage-file residue remaining/unverifiable: `RollbackIncomplete` with only the exact affected paths. |
| `systemctl enable --now` or `verify_enabled_service`, when no descriptor was newly published | `StopThenRetry` — respectively service-start or service-state problem; from an independent terminal stop the service and retry `enable`. |
| The same service-start/verify producers after exact changed-descriptor cleanup proves inactive service and absent socket | `Ordinary` with the corresponding service-start or service-state problem / retry `enable`. |
| The same service-start/verify producers when descriptor cleanup remains/unverifiable | `RollbackIncomplete` — service state unverifiable / check `status`, with only the exact descriptor/residue paths. |
| Test-only completed-stage injections | Inherit the matching producer family; after descriptor only, unexpected/retry `enable`; after unverified service enable, `StopThenRetry`; after verified completion, unexpected/check `status`. |

### Disable producers

| Producer boundary | Direct bounded result |
| --- | --- |
| `disable` / `lifecycle_context` context failure, or systemctl resolution | `Ordinary` — respectively unexpected failure or service-stop failure / retry `disable`. |
| Managed self-hosting proved before mutation | `IndependentTerminal` — operation cannot safely continue / run `disable` independently. |
| Caller independence or service activity unproven | `StopThenRetry` — operation cannot safely continue; independently stop the service, then run `disable`. |
| `systemctl disable --now` failure, post-disable injection before verification, or `verify_disabled_service` failure | `StopThenRetry` — service-stop or service-state problem; independently stop the service, then run `disable`. |
| After service stop/disable is authoritative, descriptor identity/inspection/removal/sync failure | `PartialDisable` with the exact Q25 copy; include the exact descriptor path only when safely known and retry alone is insufficient. |
| Test-only injection after authoritative service verification but before descriptor removal | `PartialDisable` without a path. |
| Test-only injection after complete descriptor removal | `Ordinary` — unexpected failure / check `status`. |

### Uninstall producers

| Producer boundary | Direct bounded result |
| --- | --- |
| `uninstall` / `lifecycle_context` context failure, or systemctl resolution | `Ordinary` — respectively unexpected failure or service-stop failure / retry `uninstall`. |
| Managed self-hosting proved before mutation | `IndependentTerminal` — operation cannot safely continue / run `uninstall` independently. |
| Caller independence or service activity unproven | `StopThenRetry` — `The operation could not safely continue from this terminal.`; from an independent terminal run `systemctl --user stop codex-session-control.service`, then `codex-session-control uninstall`. |
| `disable --now`/absent-unit proof failure or service-stop verification failure | `StopThenRetry` — `The service state could not be verified.`; from an independent terminal run `systemctl --user stop codex-session-control.service`, then `codex-session-control uninstall`. |
| Selected-home or manifest identity evidence failure before descriptor removal | `RollbackIncomplete` — `The installed Codex Session Control state could not be verified.` / check `status`, with only exact known managed paths. |
| Descriptor render, inspection, removal, or sync failure after service stop | `RollbackIncomplete` — `Codex Desktop integration could not be updated.` / check `status`, with only exact known managed paths. |
| Unit, projection, or configuration cleanup failure while installed identity remains | `RollbackIncomplete` — cleanup could not be completed safely / retry `uninstall`, with exact affected managed paths. |
| Native executable selection failure or native plugin/marketplace removal/verification failure | `ManualCleanup` — `Codex CLI integration could not be updated.` / complete cleanup manually with only the exact allowlisted plugin or marketplace removal command. |
| Manifest removal failure while manifest and binary identity remain | `RollbackIncomplete` — cleanup could not be completed safely / retry `uninstall`, with the manifest path. |
| Manifest removed, then binary or data-root removal fails | `TerminalPartialUninstall` with the exact remaining managed paths and the no-rerun instruction. |
| Test-only completed-stage injections before identity removal | The matching producer family; an injection with no production cause uses `Ordinary` — unexpected failure / retry `uninstall`. |

No classified producer requires user-facing behavior beyond Q23, Q25, Q27, and `UpdateCompletionUnknown`. Implement only two narrow evidence repairs needed to make those direct selections truthful: split candidate creation from waiting, and make descriptor publication report whether final/stage residue remains or cleanup is unverified. Do not create a generic mutation-state model, stage-to-message table, cross-process protocol, or broader error-system rewrite.

## Active-Task Update Gate

The existing prompt remains byte-for-byte unchanged and is written to flushed stderr before input:

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

The list contains every disclosed active task. Rechecks continue to prevent undisclosed active tasks from being interrupted. Noninteractive refusal uses the interactive-terminal safety recovery. `yes`/`y` continues; every other response uses the cancellation outcome. None of these paths changes goal pause/clear behavior.

## Verbose Contract

Verbose diagnostics are chronological CSC emission order on stderr. Each line starts with:

```text
[verbose] <command>[/<phase>]:
```

Representative lines are:

```text
[verbose] setup: controller {version} ({target})
[verbose] setup: selected Codex home {codex_home}
[verbose] setup: completed preflight
[verbose] setup: completed binary
[verbose] setup: failed service-verify (service state could not be verified)
```

Staged update uses explicit phases:

```text
[verbose] update/outer: candidate {version} verified
[verbose] update/outer: starting staged candidate
[verbose] update/apply: staged marker accepted
[verbose] update/apply: completed service-restart
[verbose] update/apply: completed manifest
[verbose] update/outer: staged candidate exited successfully
```

Verbose mode is additive:

- stdout is byte-for-byte identical to default stdout;
- removing only `[verbose] ` lines from verbose stderr leaves default-visible stderr byte-for-byte identical;
- exit codes and mutation behavior are identical;
- default-visible warnings, recovery, manual gates, cleanup actions, and required paths remain present;
- successful `codex` remains completely silent;
- `mcp-server` stdout remains protocol-only.

The privacy boundary is absolute. Diagnostic construction and rendering must structurally prevent credentials, environment dumps, configuration contents, task/rollout data, full process command lines, timestamps, PIDs, and telemetry identifiers from entering events. The original technical error may remain only in the in-memory source chain; it cannot be stringified into a diagnostic event. Legacy errors without a safe typed projection use a generic allowlisted category.

## Staged Update Ownership

The public `--verbose` state is propagated to the candidate as a normal CLI flag before `update`; `CODEX_SESSION_CONTROL_STAGED_UPDATE=1` remains the private staged marker. No typed result is serialized between versions.

The outer process:

- emits only `update/outer` diagnostics;
- flushes diagnostics before spawning the candidate;
- inherits the candidate's stdout/stderr;
- emits no friendly success/failure for a normal candidate exit `0` or `1`;
- propagates that normal candidate exit code unchanged.

The candidate/apply process:

- emits `update/apply` diagnostics;
- owns the single friendly update success, cancellation, refusal, partial, or failure result;
- is the only process that renders apply-stage user prose.

`run_candidate_apply` must create the child and wait for it as distinguishable operations. A proven spawn failure means the candidate never began; the outer process selects the ordinary installation-files failure with `Try again: codex-session-control update`. After a successful spawn, wait failure, signal termination, or an exit outside `0`/`1` selects the exact `UpdateCompletionUnknown` result. Normal exit `0` or `1` remains candidate-owned and is propagated unchanged without a second friendly result. There is no private JSON or cross-version report protocol.

## MCP Stdout Isolation

`Command::McpServer` bypasses human result rendering and verbose handling. Its stdout is exclusively JSON-RPC/MCP framing produced by `rmcp::transport::stdio`; human warnings, diagnostics, lifecycle reports, and panic-like wrappers must never be written there. Stderr must not contain MCP result/error frames. Existing EOF/reaping and protocol catalog behavior remains unchanged.

## Expected File Targets

### Create

- `src/cli_output.rs` — closed human success/failure/status types, exact renderers, composition, channels, and exit codes.
- `src/diagnostics.rs` — concrete off/stderr/test-record diagnostic emitter, typed allowlist, prefix/phase rendering, and privacy boundary.

### Modify

- `src/main.rs` — global render/write boundary, typed exit propagation, staged-update ownership, wrapper failure handling, and MCP bypass.
- `src/cli.rs` — exact root/subcommand help, hidden `mcp-server`, global `--verbose`, and `codex` passthrough boundary.
- `src/install.rs` — replace string receipts with shared typed output/diagnostic plumbing while retaining lifecycle behavior.
- `src/install/setup.rs` — setup outcome, warning, independent process facts, stage diagnostics, and closed failure classification.
- `src/install/update.rs` — update outcomes, active-task exceptional outcomes, outer/apply diagnostics and ownership, service-enabled fact, and candidate exit propagation.
- `src/install/enable_disable.rs` — enable/disable outcomes, independent process facts, Desktop precedence, safety refusals, and partial-disable truthfulness.
- `src/install/uninstall.rs` — complete/partial outcomes, manual cleanup, terminal retry prohibition, and allowlisted paths.
- `src/desktop/descriptor.rs` — preserve narrow publication/cleanup evidence needed to choose an existing ordinary or rollback-incomplete result; do not add a generic transaction abstraction.
- `src/install/status.rs` — typed final status result, independent integration classification, readable problems/recoveries, and exit matrix without changing read-only evidence collection.
- `src/install/wrapper.rs` — exact friendly failure and silent-success diagnostic boundary.
- `src/install/service.rs` — return independent typed running-client facts instead of appending user prose.
- `tests/cli_contract/command_surface.rs` — exact help, hidden command, global option placement, and passthrough tests.
- `src/install/tests/setup.rs`, `src/install/tests/enable_disable.rs`, `src/install/tests/uninstall.rs`, `src/install/tests/status.rs`, `src/install/tests/codex_wrapper.rs`, `src/install/tests/active_turn_gate.rs`, `src/install/tests/update_matrix.rs`, and `src/install/tests/failure_retry.rs` — focused renderer/classification/ownership assertions while preserving operational fixtures and mutation-safety coverage.
- `src/desktop/tests/descriptor.rs` — direct ordinary-versus-rollback-incomplete descriptor publication evidence.
- `src/install/tests/normal_home_setup.rs` and `src/install/tests/systemd.rs` — replace retained completed-stage/user-receipt assertions with typed test-record diagnostics or explicitly prefixed diagnostic evidence.
- `src/install/tests/selected_home_evidence.rs` — adapt retained assertions to the typed final status result without weakening evidence-strength coverage.
- `src/install/tests/desktop_start_lifecycle.rs` and `src/install/tests/desktop_stop_lifecycle.rs` — independent running-client facts and restart/cleanup presentation where existing fixtures already prove the behavior.
- `tests/app_server_integration/normal_home.rs` — replace rendered stage-order parsing with typed or explicitly diagnostic evidence and prove staged update ownership where the existing integration harness is authoritative.
- `tests/app_server_integration/live_harness.rs` and `tests/app_server_integration/cases.rs` — replace the shared shutdown-stage string parser and its assertions with explicitly prefixed subprocess diagnostics while preserving the live systemd boundary.
- `tests/mcp_contract.rs` — explicit stdout-isolation regression for human/verbose output.
- `README.md` — human command list/descriptions; do not present hidden `mcp-server` as a normal interactive command.
- `docs/desktop.md` — `Codex Desktop integration` terminology and the approved `ready`/`unavailable`/`unhealthy`/`could not verify` vocabulary.

The implementation plan may narrow a listed test file when an existing adjacent test proves the exact requirement, but it must not move production work outside these responsibilities without showing the concrete repository reason. New focused modules are allowed only when they create a clearer responsibility boundary; module count alone is not a stop trigger.

## Verification Guidance

Use TDD for each behavioral slice: add or update the narrow failing assertion first, observe the relevant failure, implement the minimum typed/rendering change, then rerun the focused test.

Required focused proof:

- Exact root/subcommand help snapshots, hidden `mcp-server`, `--verbose` placement, parser exit `2`, and `codex` passthrough.
- One exact renderer case per distinct output block and per materially distinct `UserFailure` variant, including byte-for-byte `UpdateCompletionUnknown` stderr with exit `1` and no retry line.
- Focused producer-boundary tables covering every row in `Producer-Boundary Failure Classification`; exercise grouped branches only when they genuinely share the same complete variant, and never map from stage labels or parsed error text.
- One table-driven setup/enable guidance-precedence test, including independent CLI detection and invalid typed combinations.
- One table-driven status classification/exit test covering all four top-level states and four integration labels without cross-producting irrelevant dimensions.
- One table-driven privacy sentinel that exercises every diagnostic constructor against every prohibited data class.
- Default/verbose parity assertions for each human command: identical stdout, identical non-verbose stderr after diagnostic-line filtering, identical exit code, and identical mutation evidence.
- One staged-update subprocess test covering `update/outer` and `update/apply` labels, flush/ownership behavior, one friendly result, normal exit `0`/`1` propagation, proven spawn failure, wait failure, signal termination, and an exit outside `0`/`1`.
- Focused descriptor-publication tests proving a clean pre-publication failure selects the ordinary result while final/stage residue or unverified cleanup selects the existing rollback-incomplete result with only exact managed paths.
- One successful-wrapper test proving zero CSC bytes, including global verbosity, plus the exact failed-wrapper block.
- One MCP stdio test proving every stdout line remains a valid JSON-RPC/MCP frame and no human or verbose line reaches stdout.
- Existing filesystem mutation, systemd argv, service-state, selected-home, descriptor safety, manifest-last, cleanup, retry, active-task recheck, and failure-injection tests remain authoritative; update only their presentation/stage-recording assertions.

Suggested focused commands for the plan to refine into red/green steps:

```bash
cargo test --test cli_contract --locked command_surface
cargo test --bin codex-session-control --locked install::tests::setup
cargo test --bin codex-session-control --locked install::tests::enable_disable
cargo test --bin codex-session-control --locked install::tests::status
cargo test --bin codex-session-control --locked install::tests::uninstall
cargo test --bin codex-session-control --locked install::tests::codex_wrapper
cargo test --bin codex-session-control --locked install::tests::active_turn_gate
cargo test --bin codex-session-control --locked install::tests::update_matrix
cargo test --test app_server_integration --locked
cargo test --test mcp_contract --locked
```

The ignored `live_normal_home_*` cases in `tests/app_server_integration.rs` are authoritative for the disposable systemd-user staged-update boundary. The local command above compiles the integration target and runs only non-ignored cases; it does not execute those live cases.

- **[MANUAL/CI]** Run the repository's disposable systemd-user CI job after the implementation branch is published for CI, or run `bash scripts/ci/disposable-systemd-user-contract.sh` only inside the documented disposable environment. Do not run the ignored live systemd cases directly on a normal development workstation.

Review the generated help exactly as required by `CONTRIBUTING.md`:

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

Before completion, run fresh full verification:

```bash
./scripts/check.sh
```

For the documentation boundary, also run:

```bash
git diff --check -- README.md docs/desktop.md
rg -n 'not_ready|Desktop configuration|Unattached running clients|native results remain authoritative' README.md docs src/cli.rs src/install
```

Any remaining match must be either an internal typed identifier that cannot reach human output or an intentional historical artifact under `docs/superpowers/`; the implementation review must inspect and explain it.

## Acceptance Criteria

1. Root help and detailed help match the approved copy, expose no new public controls, hide `mcp-server`, and preserve the `codex` argument boundary.
2. Default successful `setup`, `update`, `enable`, `disable`, and `uninstall` output contains only the approved outcome and applicable guidance blocks with exact blank-line/order rules.
3. Running CLI and Desktop facts are produced independently; Desktop availability cannot suppress CLI guidance, and specific detected-client guidance replaces generic duplicate guidance.
4. On successful commands, the compatibility warning, Desktop-discovery warning, and PATH notice use the approved copy and are written after successful stdout; manual-gate, failure, refusal, recovery, and partial-outcome stderr retains its separately specified ordering.
5. `status` renders only `healthy`, `disabled`, `not installed`, or `unhealthy`; integration rows render only `ready`, `unavailable`, `unhealthy`, or `could not verify`.
6. CLI and Desktop status classifications are independent and derived from current evidence without changing the read-only safety boundary.
7. Status returns `0` for `healthy`/`disabled` and `1` for `not installed`/`unhealthy`; verbosity and optional integration detail do not override that matrix.
8. Every concrete lifecycle failure producer listed in the producer-boundary tables directly selects one closed `UserFailure` variant; no stage label, independent failure axis, or arbitrary low-level error string selects user-facing prose.
9. Disable-after-stop cleanup failure, terminal partial uninstall, rollback-incomplete cleanup, manual native cleanup, verified-release recovery, independent-terminal refusal, interactive-terminal refusal, and cancellation each preserve their distinct approved behavior and retry safety.
10. The active-task prompt is byte-for-byte unchanged, remains flushed before input, retains task identifiers and goal semantics, and the final recheck still prevents undisclosed active work from being interrupted.
11. Default and verbose invocations have identical stdout, default-visible stderr, exit code, and mutations after verbose diagnostic lines are removed.
12. Verbose lines are chronological within CSC emission order, use `[verbose] <command>[/<phase>]:`, and cover only the approved demand-driven event set.
13. Diagnostic events structurally cannot contain credentials, environment dumps, configuration contents, task/rollout data, full process command lines, timestamps, PIDs, or telemetry identifiers; sentinel tests cover every constructor and prohibited class.
14. The update outer process emits only outer diagnostics and propagates normal candidate exit `0`/`1`; the candidate owns the only friendly apply result; proven spawn failure receives the ordinary retry-safe result, while wait failure, signal termination, or exit outside `0`/`1` receives the exact stderr-only, exit-`1`, no-retry `UpdateCompletionUnknown` result without a serialization protocol.
15. Successful `codex` emits no CSC bytes even under global verbosity; failed wrapper launch renders the exact friendly block and safe diagnostics only.
16. `mcp-server` stdout remains protocol-only, its command stays callable but hidden, and existing EOF/reaping/catalog behavior continues to pass.
17. Descriptor publication preserves only the narrow final/stage residue and cleanup evidence needed for an existing ordinary or rollback-incomplete selection; existing operational safety tests continue to prove filesystem ownership, selected-home validation, systemd sequencing, Desktop descriptor safety, mutation ordering, cleanup, retries, and manifest-last behavior without relying on human prose.
18. `README.md` and `docs/desktop.md` describe the active human command/status vocabulary and contain no superseded public labels.
19. No new dependency, background/concurrent subsystem, cross-process result protocol, generic CLI framework, or unrelated MCP/app-server/domain change is introduced.
20. Fresh `./scripts/check.sh` passes after the implementation.

## Escalation Boundary

Stop and return to the operator before implementation proceeds if approved UX, safety, privacy, lifecycle behavior, or output contracts must change; if a new dependency, background or concurrent subsystem, or cross-process serialization protocol is needed; if work expands into unrelated MCP, app-server, or domain models; or if actual net growth exceeds roughly 1,500 production lines or 1,800 test/documentation lines. Line-count thresholds trigger review, not automatic rejection.

## Gap Resolutions

- Current `docs/desktop.md` still documents `Desktop configuration` with `not_ready` and `unverified`. Updating that exact public documentation target is forced by the approved four-state integration vocabulary.
- Current `README.md` lists `mcp-server` among normal available commands. Removing it from the human command table, while leaving the callable plugin transport unchanged, is forced by the approved hidden-help boundary.
