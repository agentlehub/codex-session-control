# User-Facing CLI Output Rust Architecture Review

## 1. Verdict

Choose A, but keep it narrow: typed command outcomes/failures plus typed diagnostic events rendered at the CLI boundary.

B preserves the defect. Current `SetupProgress`, `UpdateProgress`, `LifecycleProgress`, and `UninstallProgress` build stage journals as strings and splice them into `ControllerError::Operational`; this prevents friendly rendering without parsing or duplicating semantic decisions.

C is categorically wrong. Post-processing strings such as `failed at descriptor-remove:` would make correctness depend on private prose, destroy error-source structure, and reproduce the brittle exact-string tests the refactor is meant to remove.

A is best because the code already has real typed state—stage enums, service state, evidence cases, Desktop state, restart reasons—but throws that type information away just before reporting.

## 2. Smallest clean boundary

Add two focused modules:

- `src/cli_output.rs`: user-facing reports, failures, notices, renderers, and final channel/exit representation.
- `src/diagnostics.rs`: diagnostic scope, events, sink, stderr renderer, and test recorder.

Do not reuse `src/install/render.rs`; that module renders installed systemd/plugin assets, not CLI output.

Command modules continue to own operational decisions and return command-specific typed outcomes:

```rust
struct CommandReport<T> {
    outcome: T,
    notices: Vec<Notice>,
}

struct SetupOutcome {
    version: String,
    desktop_configuration_changed: bool,
}

enum UpdateOutcome {
    Applied {
        version: String,
        service_enabled: bool,
        desktop_configuration_changed: bool,
    },
    AlreadyCurrent {
        version: String,
    },
}

struct EnableOutcome {
    desktop: EnableDesktopOutcome,
}

struct DisableOutcome {
    desktop_configuration_removed: bool,
}

struct UninstallOutcome {
    desktop_configuration_removed: bool,
}
```

`status` is different: it should return a retained evidence snapshot and derived view, not a pre-rendered string.

The shared boundary should be approximately:

```rust
enum CommandName {
    Setup,
    Update,
    Status,
    Enable,
    Disable,
    Uninstall,
    Codex,
}

struct CommandFailure {
    command: CommandName,
    problem: String,
    recovery: Recovery,
    diagnostic: FailureDiagnostic,
}

struct Recovery {
    lead: String,
    commands: Vec<String>,
    notes: Vec<String>,
}

struct FailureDiagnostic {
    stage: Option<Stage>,
    cause: ControllerError,
    cleanup: Vec<CleanupFact>,
}

struct RenderedOutput {
    stdout: String,
    stderr: String,
    exit_code: u8,
}
```

The headline—“could not be installed/updated/started…”—comes from `CommandName`, not call-site strings. `problem` and `Recovery` remain structured fragments because forcing every specialized safety/recovery case into a giant enum would be abstraction theater.

## 3. Diagnostic mechanism

Use one synchronous `DiagnosticSink` trait carrying an owned `DiagnosticEvent` enum:

```rust
trait DiagnosticSink: Send {
    fn emit(&mut self, scope: DiagnosticScope, event: DiagnosticEvent);
}

enum DiagnosticScope {
    Command(CommandName),
    UpdateOuter,
    UpdateApply,
}

enum DiagnosticEvent {
    StageCompleted(Stage),
    ControllerIdentity { version: String, target: String },
    CandidateVerified { version: String, target: String },
    SelectedCodexHome(PathBuf),
    ServiceEvidence(ServiceDiagnostic),
    PluginEvidence(PluginDiagnostic),
    DesktopEvidence(DesktopDiagnostic),
    Cleanup(CleanupFact),
    Failure { stage: Option<Stage>, cause: String },
}
```

Implement only:

- `NoopDiagnostics`
- `StderrDiagnostics`
- `RecordingDiagnostics` under tests

Do not use:

- a channel: operations are sequential; it adds a task, shutdown protocol, buffering, and ordering hazards;
- raw writers in command code: that leaks prose and I/O failures into lifecycle logic;
- callbacks: awkward ownership across async calls and worse test ergonomics;
- `tracing`: unnecessary dependency and policy surface;
- serialized events: verbose output is explicitly not a stable machine protocol.

The sink method should not return an error. Verbose-output failure must not alter lifecycle state or exit behavior. `StderrDiagnostics` should disable itself after its first write failure.

Use owned `String`/`PathBuf` fields. Do not retain borrowed paths, `StderrLock`, or closures across `.await`.

A single typed `Stage` enum is sufficient; `DiagnosticScope` disambiguates shared names such as `configuration` and `manifest`. Do not introduce a trait per command-stage enum.

## 4. Error/report model

Keep low-level `ControllerError` as the technical source for now. Wrap it once at a command boundary:

```rust
impl std::error::Error for CommandFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.diagnostic.cause)
    }
}
```

This preserves four distinct concerns:

- command-level failed outcome;
- readable user problem;
- actionable recovery;
- raw typed stage/cause/cleanup evidence.

Do not add user-facing prose to `ControllerError::Operational`. Existing low-level errors are heavily flattened strings already; repairing the entire source chain is a separate refactor. New code should avoid further flattening, but this feature does not justify retyping every filesystem, systemd, release, and native error.

`Progress::complete(stage)` emits `StageCompleted(stage)` immediately. It no longer stores completed stages or renders stderr. `Progress::fail(...)` constructs `CommandFailure`; the top-level command boundary emits its `Failure` diagnostic before rendering the friendly error.

## 5. Status evidence retention

Current status inspection discards too much information: it accumulates `StatusFailure`, collapses app-server health, then renders directly. Replace that final collapse with:

```rust
struct StatusSnapshot {
    installation: InstallationEvidence,
    native: NativeIntegrationEvidence,
    service: ServiceEvidence,
    socket: SocketEvidence,
    app_server: AppServerEvidence,
    desktop: DesktopIntegrationEvidence,
    findings: Vec<StatusFinding>,
}

struct StatusView {
    overall: OverallStatus,
    version: Option<String>,
    service: ServiceSummary,
    cli: IntegrationState,
    desktop: IntegrationState,
    problems: Vec<UserProblem>,
}

enum OverallStatus {
    Healthy,
    Disabled,
    NotInstalled,
    Unhealthy,
}

enum IntegrationState {
    Ready,
    Unavailable,
    Unhealthy,
    CouldNotVerify,
}
```

Inspection populates `StatusSnapshot` without rendering. One pure classifier derives `StatusView`. The normal renderer consumes the view; verbose rendering consumes the original snapshot.

This is essential for Q17:

- clean absence or intentional disablement → `unavailable`;
- decisive fault while expected to work → `unhealthy`;
- incomplete/raced evidence → `could not verify`;
- affirmative full chain → `ready`.

CLI and Desktop classifiers share service/socket/app-server evidence but run independently. Top-level unhealthy must not overwrite valid integration evidence.

Preserve the current read-only safety boundary: status must not launch Desktop, publish descriptors, create parents, or dial an unsafe socket.

## 6. Rendering and warning order

Only `cli_output` creates final human prose. Only `main` writes final buffers.

On success:

1. write and flush stdout outcome/guidance;
2. write and flush default-visible stderr warnings/notices.

On failure:

1. emit the final verbose failure event, if enabled;
2. write the entire friendly outcome/problem/recovery block once to stderr;
3. return its exit code.

`status` remains an inspection report on stdout. `codex` success writes nothing. `mcp-server` bypasses user-facing rendering entirely; stdout remains protocol-only.

The interactive active-task update prompt is the sole justified exception because it occurs mid-operation and must flush before blocking for input. Keep it command-specific; do not invent a general interaction framework during this refactor.

Absolute stdout/stderr merge order cannot be guaranteed after users redirect the descriptors separately. The enforceable contract is program write/flush order and exact per-channel content. The artifact should state that rather than promise a globally ordered merged stream.

## 7. Update outer/apply boundary

Use the public CLI flag for propagation:

```text
downloaded-candidate --verbose update
```

and retain the private staged marker:

```text
CODEX_SESSION_CONTROL_STAGED_UPDATE=1
```

The old process uses `UpdateOuter`; the candidate uses `UpdateApply`. Outer diagnostics must be flushed before spawn. The child inherits stdout/stderr and owns the single friendly update success or failure because it is the version that actually applied and understands its own report types.

Return a distinct disposition:

```rust
enum UpdateExecution {
    Local(CommandReport<UpdateOutcome>),
    ReportedByCandidate(u8),
}
```

For candidate exit 0 or 1, the parent emits only its outer diagnostic and propagates the code. It must not wrap the status in another `CommandFailure`. Spawn failure, signal termination, or an abnormal exit code gets one parent-owned friendly failure because the child did not complete the normal reporting contract.

Do not send typed reports back to the outer process. Outer and downloaded candidate are different versions; private serialized result types would require a versioned compatibility protocol for no user benefit.

## 8. Incremental migration and tests

Use this sequence:

1. Add output and diagnostic types plus renderer unit tests.
2. Migrate setup.
3. Migrate enable/disable and uninstall.
4. Refactor status into snapshot → view → rendering.
5. Migrate wrapper failure; retain silent successful `exec`.
6. Migrate update last because it spans two processes.
7. Switch `main` to one final writer path and hide `mcp-server`.

Test migration:

- Replace parsing of `completed: ...` strings with exact `Vec<(DiagnosticScope, DiagnosticEvent)>` assertions.
- Retain filesystem mutation, service argv, safety, retry, and manifest-last assertions unchanged.
- Keep small rendering snapshots for exact approved user prose.
- For every command, assert verbose and default have identical exit code/stdout and that filtering `[verbose] ` lines from verbose stderr yields default stderr.
- Add update subprocess tests for outer/apply labels, streaming order, one friendly success, one friendly failure, and parent exit propagation.
- Add status classifier truth tables independent of live inspection.
- Keep end-to-end MCP protocol tests proving stdout isolation.
- Add a successful wrapper test proving no CSC bytes are emitted even with `--verbose`.

## 9. Rust-specific hazards

- `&mut dyn DiagnosticSink` across `.await` is safe only while operations remain sequential. Require `Send`; do not share it through `Arc<Mutex<_>>`.
- Do not retain `StderrLock` across `.await`; lock, write one complete line, flush, and release inside `emit`.
- Events and reports should own data. Borrowed `&Path`/`&str` in async results will create needless lifetime coupling.
- Diagnostic I/O must be best-effort or verbosity changes exit semantics.
- The current `ControllerError` loses many source chains. Do not pretend raw display text is structured classification.
- Update outer and candidate share descriptors but are serialized, so explicit flushes preserve practical chronological order without concurrent writers.
- Wrapper `exec()` never returns on success. Do not buffer success diagnostics that could be lost; the approved contract requires silence anyway.
- Do not add an async channel, background renderer, generic output AST, logging framework, private JSON protocol, or trait hierarchy for every report fragment.

## 10. Artifact adjustments

No approved decision is infeasible.

Three points should be clarified:

- “Chronological” means emission order within CSC and across the serialized outer/child handoff, not a total ordering after stdout and stderr are redirected independently.
- “Primary outcome first” applies to default-visible success material. Verbose stage lines necessarily precede a final success outcome.
- `ready` is a status snapshot based on affirmative evidence at inspection time, not a guarantee against a race immediately afterward.

The repository remained untouched. Branch is `codex/user-facing-cli-output` at `528ef31566048520617929a45ea446ff9af70559`; the pre-existing `docs/superpowers/` tree remains untracked.
