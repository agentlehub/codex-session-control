# Desktop-Owned Session Control Phase 1 Implementation Plan

**Status:** approved
**Source:** `docs/superpowers/specs/2026-08-29-desktop-owned-session-control-design.md`

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` to implement this plan task-by-task. One primary orchestrator owns integration and evidence; isolated workers receive the non-overlapping ownership slices defined below. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor Codex Session Control into one stateless stdio MCP server that preserves the exact thirteen-tool contract while using only the Desktop-owned shared app-server authority, then deliver the reviewed feature branch as an internal CSC pull request.

**Architecture:** `SessionControlMcp` creates a fresh `AppServerClient` boundary for every operation; every new connection resolves and validates the deterministic Desktop endpoint, opens the Unix WebSocket at exact `/rpc`, initializes, and then reuses that connection only for the current operation. The retained protocol, mapping, wait, reconciliation, and error core stays intact; service ownership, saved installation state, lifecycle commands, release distribution, and downstream Desktop attachment code are deleted. A checkout-local legacy plugin contains one native binary built and atomically staged by one installer.

**Tech Stack:** Rust 1.95, Tokio, rmcp stdio transport, tokio-tungstenite over Unix sockets, rustix filesystem/process metadata, serde/serde_json, semver 1.0.28, Bash, GitHub Actions native x86-64/AArch64 runners, Codex CLI 0.149.1 legacy plugins, upstream Codex Desktop shared socket.

---

## Prerequisites

### Repository and authority gate

- Work only in `/home/korty/dev/agentlehub/codex-session-control` on `feat/desktop-owned-session-control`.
- Before dispatching any production worker, run:

  ```bash
  cd /home/korty/dev/agentlehub/codex-session-control
  test "$(git branch --show-current)" = feat/desktop-owned-session-control
  test "$(git merge-base HEAD origin/main)" = 88f2ac5b124abaa2a355ca88304e9439c692eb0a
  git merge-base --is-ancestor da06acf2c229b5e0aa68ff45ab3aa87037aa4d52 HEAD
  git merge-base --is-ancestor b7b9d94 HEAD
  test -z "$(git status --short)"
  ```

  Expected: every command exits `0`; the status output is empty.
- If the branch, merge-base, approved design commit, or clean-state assertion fails, stop. Inventory the drift, repair or re-review the specification and this plan, and obtain a coherent approved boundary before deleting code.
- Preserve `src/mcp/contract.rs` and `tests/mcp_contract.rs` as the public contract authorities. No tool name, order, title, description, annotation, schema, default, result, error, stage, dispatch evidence, or cross-thread policy may drift.
- Source readiness is anchored by `docs/superpowers/reviews/2026-08-29-desktop-owned-session-control-design-review.md`, whose final state is `passed` after the `b7b9d94` support-module repair. Any new `BLOCKER`/`MAJOR` material routes back to specification repair before implementation.
- The implementation base for every review is `88f2ac5b124abaa2a355ca88304e9439c692eb0a`; documentation commits `da06acf2c229b5e0aa68ff45ab3aa87037aa4d52`, `109adc71ede97073751c4a1693c51652a10d5c90`, and `b7b9d94` are source artifacts, not behavior to reimplement.

### Tools and runtime data

- Verify Rust and repository tools:

  ```bash
  rustc --version
  cargo --version
  shellcheck --version
  jq --version
  actionlint -version
  readelf --version
  codex --version
  gh auth status --hostname github.com
  ```

  Expected: Rust reports `1.95.x`; `actionlint` reports `1.7.12`; Codex CLI reports `0.149.1`; `gh` is authenticated to the internal `agentlehub` repository.
- Use committed `Cargo.lock`. Dependency changes are limited to the full-SemVer build edge and deletion-driven pruning proven by whole-tree usage checks.
- Verify `/opt/codex-desktop/resources/codex --version` reports `codex-cli 0.150.0-alpha.12.2` before fixture recapture.
- Native Linux x86-64 (`ubuntu-24.04`) and AArch64 (`ubuntu-24.04-arm`) runners must be available. Cross-compilation is not acceptance evidence.
- The local plugin compatibility contract uses isolated `HOME` and `CODEX_HOME`; never point packaging tests at the operator's normal Codex state.
- Live validation may create and mutate only tasks represented by test-only `OwnedThreadId` values already persisted in that run's private ledger.

### Execution authority

- The approved source authorizes uninterrupted execution through the internal CSC pull request. Do not insert routine approval pauses between tasks or passed review milestones.
- Stop only for a failed prerequisite that invalidates the approved design, a product contradiction, unavailable credentials, a marked human-only interaction, new external authority, or scope outside this plan. Diagnose test failures and valid review findings inside the owning task, then rerun the required gate chain.

### Human-only actions

- **[MANUAL]** Start the unmodified upstream Codex Desktop build with `shared-app-server-socket` enabled and verify its same-user private Unix socket exists before the fixture and live gates.
- **[MANUAL]** In a new normal Codex CLI session, verify install visibility, exact thirteen-tool presence, `/plugins` disable absence, `/plugins` re-enable presence, same-version refresh, version-bump refresh, native removal absence, and the new-session boundary.
- **[MANUAL]** In Desktop Plugins UI and newly created Desktop tasks, verify the same install, disable, re-enable, refresh, removal, exact catalog, and new-task boundaries. Do not alter any pre-existing task.
- **[MANUAL]** Verify the Desktop-launched plugin process receives exactly `XDG_RUNTIME_DIR`, `CODEX_LINUX_APP_ID`, and `CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET`; do not record their values in repository artifacts.
- **[MANUAL]** Prove the old installed 0.3.x cleanup commands against the actual installed state before publishing them in `docs/upgrading.md`: native plugin removal, old marketplace removal, and `systemctl --user disable --now codex-session-control.service`. If the installed state contradicts those commands, stop and repair the documentation from observed state; do not add migration code.
- **[MANUAL]** Relaunch Desktop and normal CLI tasks after staging so they load the new executable. Do not claim hot-loading or termination of a process already owned by an existing task.

### Delivery prohibitions

- Push only a normal update of `feat/desktop-owned-session-control` to internal `origin`; never force-push.
- Do not merge the CSC pull request, create or move a tag, publish a release/package/installer/checksum, touch a registry, or contact official Desktop upstream.
- Phase 1 contains no attached-CLI code. Phase 2 begins only after the internal CSC pull request exists and must use a separate Desktop specification and plan.

## Acceptance Criteria

| Spec AC | Measurable evidence | Owning tasks |
| --- | --- | --- |
| 1 | The no-argument binary speaks MCP on stdout, exits at stdin EOF, spawns no child, and lists 13 tools. | 6, 17 |
| 2 | `public_catalog_is_exact` and retained mapping/error suites pass unchanged except invocation plumbing. | 3, 6, 17 |
| 3 | All service, authority, socket/lock/descriptor, wrapper, updater, and saved-target code and artifacts are absent. | 11, 17 |
| 4 | Each independent operation and each reconciliation/descendant connection resolves and validates fresh metadata. | 2, 3, 7 |
| 5 | Explicit bridge socket precedence is table-tested and does not read XDG/app-ID inputs. | 2 |
| 6 | Missing explicit socket and missing/empty XDG maps to `target_unavailable` without fallback or scanning. | 2 |
| 7 | Lexical normalization, app ID, ownership, symlink, type, exact directory mode, and socket `0600`/`0700` predicates are exhaustive tests. | 2 |
| 8 | Unsafe metadata and validation races fail `authority_transport_failure`; public errors contain no selected path or environment value. | 2, 7 |
| 9 | Production and retained live harnesses use `ws://localhost/rpc`; `/`, query strings, and other paths fail against a strict fake server. | 3 |
| 10 | The same stdio process succeeds on the next independent call after socket inode replacement. | 7 |
| 11 | A disconnected in-flight `threads_wait` returns the retained transport error and opens no replacement connection. | 7 |
| 12 | Every ambiguous mutation writes at most once and remains `outcome_unknown` with dispatch evidence; reconciliation is read-only. | 5, 7 |
| 13 | `thread_message_send` resumes the exact persisted `notLoaded` thread on the same connection before `turn/start`. | 5 |
| 14 | Resume error, malformed result, ID drift, still-unloaded state, and active race send zero prompts. | 5 |
| 15 | Successful persisted messaging sends the original prompt exactly once through `turn/start`. | 5 |
| 16 | Canonical full SemVer pins `0.150.0-alpha.12.2`, fixture/schema evidence is recaptured from the exact Desktop binary, and mismatch warnings remain. | 1 |
| 17 | Legacy manifests are exact, forward only the three approved variables, and retain a Codex 0.149.1 v1 negative control. | 9 |
| 18 | One installer performs a locked release build and atomically stages one regular mode-0755 current-host executable. | 9 |
| 19 | Static and runtime packaging tests prove no download, checksum catalog, release selector, standalone install, or architecture dispatcher. | 9, 11 |
| 20 | Both native CI architectures build, inspect, stage, execute, and list tools from their native ELF. | 12 |
| 21 | New CLI sessions and Desktop tasks expose the identical 13 tools. | 9, 14 |
| 22 | A generic client initializes and lists the exact catalog through the stable checkout-relative binary from another cwd, with no Hermes mode. | 9 |
| 23 | Same-version and version-bump restaging refresh cache content without updater or legacy-state mutation. | 9, 14 |
| 24 | Manual CLI/Desktop evidence covers install, disable, re-enable, update, removal, and session/task boundaries. | 14 |
| 25 | Native removal removes registration/tools while retaining the clone and staged binary and making no old-process termination claim. | 9, 14 |
| 26 | `docs/upgrading.md` contains the implementation-proven five-step 0.3.x cutover and no migration-code claim. | 13, 14 |
| 27 | The sole ignored live gate exercises all 13 tools only on ledger-owned disposable tasks and archives exactly those IDs after success or recoverable failure. | 15 |
| 28 | A dedicated whole-implementation inventory justifies or deletes every added module, helper, dependency, installer branch, compatibility path, and test layer. | 19 |
| 29 | Reader-facing files and bug form advertise only current plugin/stdio evidence and no removed CSC command. | 13, 17 |
| 30 | Fresh `./scripts/check.sh`, specification-compliance, DRY/YAGNI, and code-quality gates pass in that order on the final tree. | 17-20 |
| 31 | The branch is pushed and an internal CSC pull request is open; no merge/tag/release/publish occurs. | 21 |
| 32 | A fresh resumable Phase 2 creates and reviews its separate Desktop spec/plan, executes it through verification and ordered reviews, and opens an internal-fork PR without official-upstream action. | 22 |

## File Structure and Worker Ownership

The orchestrator dispatches only the named ownership slice. Workers are not alone in the repository: they must not revert another worker's changes, sweep unrelated dirt, or modify paths outside their slice. Implementation workers run one at a time so each starts from the preceding committed state; waves describe dependency groupings, not parallel dispatch.

Route architecture-sensitive endpoint/version discovery and every review to `gpt-5.6-sol` / `xhigh`; route bounded runtime, packaging, CI, documentation, and live-harness implementation to `gpt-5.6-terra` / `xhigh`; route only the mechanical bulk-deletion slice to `gpt-5.6-terra` / `high`. Do not use `max` or `ultra`. Every worker writes large command/review output to a task-specific `/tmp/csc-phase1-*` artifact and returns only a concise result to the orchestrator.

| Slice | Files and responsibility | Tasks |
| --- | --- | --- |
| Version/fixture worker | `Cargo.toml`, `Cargo.lock`, `build.rs`, `supported-codex-version.txt`, `scripts/set-supported-codex-version.sh`, version assertions in `tests/workflow_contract.rs`, generated native-version block in `README.md`, `tests/fixtures/app-server-contract.json`, and fixture capture in `src/app_server/tests/live_capture.rs`. Own canonical full SemVer, exact version pin, transactional setter, separate Codex 0.149.1 plugin-host copy, and recaptured native evidence. | 1, then dependency pruning support in 11 |
| Endpoint/transport worker | Create `src/app_server/endpoint.rs`; modify `src/app_server.rs` only to declare the endpoint module in Task 2, then own endpoint/restart follow-up in Task 7. Own environment resolution and security predicates. | 2, 7 |
| Runtime migration worker | Modify `src/main.rs`, `src/app_server.rs`, `src/app_server/tests.rs`, `src/app_server/tests/transport.rs`, `src/app_server/tests/live_capture.rs`, `src/mcp.rs`, `src/mcp/operations.rs`, the named `src/mcp/tests/` files, and the exact URL in `tests/app_server_integration/live_harness.rs`; create `tests/desktop_shared_socket_contract.rs`. Own one atomic compile-safe lifecycle detachment, exact `/rpc`, client injection/call-site migration, and focused integration evidence. | 3 |
| MCP seam worker | Modify `src/mcp/operations.rs`, all named files under `src/mcp/tests/`, and `src/mcp/tests/support.rs`. Own persisted resume, reconciliation, descendant connections, waits, and no replay after the atomic migration. | 5, 7 |
| Process/contract worker | Modify `src/main.rs` and `tests/mcp_contract.rs`. Own no-argument rejection, stdout/stderr separation, no children, EOF exit, and exact public catalog after Task 3's direct stdio migration. | 6 |
| Packaging worker | Create `.agents/plugins/marketplace.json`, `plugins/codex-session-control/.codex-plugin/plugin.json`, `plugins/codex-session-control/.mcp.json`, `plugins/codex-session-control/bin/.gitignore`, `scripts/install-local-plugin.sh`, and `tests/plugin_packaging_contract.rs`. Own the local legacy plugin, build/stage/register installer, isolated host/cache/generic-client contracts, and removal retention. | 9 |
| Deletion worker | Delete `src/install.rs`, `src/install/`, `src/desktop.rs`, `src/desktop/`, `src/cli.rs`, `src/cli_output.rs`, `src/diagnostics.rs`, old service/marketplace assets, `install.sh`, old systemd contract, old CLI tests, and release/publish workflows; modify `src/model.rs` and `src/error.rs`. Provide whole-tree dependency-usage evidence to the version/dependency owner before `Cargo.toml`/`Cargo.lock` pruning. | 11 |
| CI/check worker | Modify `scripts/check.sh`, `.github/workflows/ci.yml`, and non-version CI/check assertions in `tests/workflow_contract.rs`. Own exactly two native architecture gates and deletion-aware local checks. | 12 |
| Documentation worker | Create `docs/upgrading.md`; modify `.github/ISSUE_TEMPLATE/bug.yml`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `docs/architecture.md`, `docs/desktop.md`, `docs/security.md`, and `docs/troubleshooting.md`. Own current plugin/stdio guidance and the evidence-backed cutover. | 13 |
| Live-validation worker | Modify `tests/app_server_integration.rs`, `tests/app_server_integration/cases.rs`, and `tests/app_server_integration/live_harness.rs`; delete `tests/app_server_integration/normal_home.rs`, `tests/app_server_integration/normal_home_paths.rs`, and `tests/app_server_integration/protocol_support.rs` after moving only its retained `NativeConnection` seam into `live_harness.rs`. Own the sole ignored all-tool gate, durable ownership ledger, hard-kill recovery, and cleanup proof. | 15 |

`tests/app_server_integration/protocol_support.rs` is an exact deletion target. Move only the existing `NativeConnection` type and correlated request loop into `live_harness.rs`; keep `connect`, `initialize`, and `codexHome` ownership handling implemented in `live_harness.rs` itself.

### Execution waves

1. Wave A: Task 1, then Task 2, then Task 3. Task 4 reviews the combined foundation.
2. Wave B: Tasks 5-7 are sequential because they share the client/MCP/process test seam. Task 8 reviews the retained runtime.
3. Wave C: Task 9 is isolated packaging work. Task 10 reviews it before any old installer or lifecycle evidence is deleted.
4. Wave D: Task 15 replaces the old live integration root while its source assets still exist; Task 16 reviews that coherent live-safety slice.
5. Wave E: Tasks 11-13 then delete obsolete source/assets and update checks/docs; Task 14 reviews the coherent subtractive product and reader surface.
6. Final: Tasks 17-20 are one ordered whole-tree verification/review chain. Task 21 delivers Phase 1. Task 22 crosses only the phase boundary.

## Review Milestones

- **M1 — Foundation:** Task 4 covers Tasks 1-3 from implementation base through canonical versioning, endpoint security, `/rpc`, and the complete client seam migration. One combined reviewer checks specification compliance first, then DRY/YAGNI pressure, then code quality.
- **M2 — Retained runtime:** Task 8 covers Tasks 5-7 since M1: persisted resume, direct stdio, restart recovery, wait disconnect, and no replay. One combined reviewer uses the same order.
- **M3 — Packaging:** Task 10 covers Task 9 only, before old packaging is deleted. It must prove the replacement covers the retained contracts rather than merely matching file shape.
- **M4 — Live safety:** Task 16 covers Task 15 and its interaction with the retained runtime and package-built executable. The execution waves deliberately run this before lifecycle assets disappear.
- **M5 — Subtractive product:** Task 14 covers Tasks 11-13 after M4: lifecycle deletion, dependency pruning, native CI/checks, documentation, and issue form.
- **Final — Whole implementation:** Tasks 18, 19, and 20 are one mandatory three-pass review over `88f2ac5b124abaa2a355ca88304e9439c692eb0a..HEAD`: dedicated specification compliance, then dedicated DRY/YAGNI, then dedicated code quality. A code change in any final pass resets the chain to Task 17 so the final recorded evidence is ordered `check.sh` -> specification compliance -> DRY/YAGNI -> code quality.

Intermediate reviews are dependency gates, not operator pauses. The orchestrator fixes valid findings, reruns affected evidence, repeats a material review after Critical/Important findings, and continues directly when no material issue remains.

## Tasks

### Task 1: Canonical Full-SemVer Pipeline and Desktop Fixture

**Owner:** Version/fixture worker
**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `build.rs`
- Modify: `supported-codex-version.txt`
- Modify: `scripts/set-supported-codex-version.sh`
- Modify: `tests/workflow_contract.rs`
- Modify: `README.md` generated native-version line and adjacent static Codex CLI 0.149.1 plugin-host line only
- Modify: `src/app_server/tests/live_capture.rs`
- Modify: `tests/fixtures/app-server-contract.json` only through the capture command

- [ ] **Step 1: Write the failing canonical-version contract**

  Extend `tested_codex_version_has_one_canonical_source` and `tested_codex_version_setter_updates_only_generated_version_data` with table cases equivalent to:

  ```rust
  let accepted = [
      "0.150.0-alpha.12.2",
      "1.2.3+build.7",
      "1.2.3-alpha.1+linux.x86-64",
  ];
  let rejected = [
      "1.2.3-01", "1.2.3-alpha..1", "1.2.3-alpha/1",
      "1.2.3-alpha\\1", "1.2.3 ", "1.2.3\nnext", "../1.2.3",
  ];
  for version in accepted {
      assert!(Command::new(&setter).arg(version).output().unwrap().status.success(), "{version}");
  }
  for version in rejected {
      let snapshot = || (
          fs::read(root.path().join("supported-codex-version.txt")).unwrap(),
          fs::read(root.path().join("README.md")).unwrap(),
          fs::read(root.path().join("tests/fixtures/app-server-contract.json")).unwrap(),
      );
      let before = snapshot();
      assert!(!Command::new(&setter).arg(version).output().unwrap().status.success(), "{version}");
      assert_eq!(snapshot(), before, "{version}");
  }
  ```

  Assert the README marker line is exactly `- Native app-server protocol validated against Codex \`<version>\`. <!-- generated: supported-codex-version -->`; assert plugin-host guidance separately names Codex CLI `0.149.1`.

  Delete the current `tested_codex_version_has_one_canonical_source` assertion that reads `scripts/ci/disposable-systemd-user-contract.sh`. That script is obsolete and deleted in Task 11; the retained version contract is the canonical file, build bridge, generated README line, setter behavior, and captured fixture.

- [ ] **Step 2: Run RED**

  Run: `cargo test --locked --test workflow_contract tested_codex_version -- --nocapture`
  Expected: FAIL because prerelease/build versions are rejected and the README still conflates the native protocol pin with a CLI-on-PATH requirement.

- [ ] **Step 3: Implement canonical parsing and transactional shell validation**

  Add the existing semver crate as a build dependency and replace the stable-only build parser with exact round-trip validation:

  ```toml
  [build-dependencies]
  semver = "1.0.28"
  ```

  ```rust
  let parsed = semver::Version::parse(version)
      .unwrap_or_else(|error| panic!("supported Codex version must be canonical SemVer: {error}"));
  assert_eq!(
      parsed.to_string(),
      version,
      "supported Codex version must be canonical SemVer"
  );
  println!("cargo::rustc-env={VERSION_ENV}={version}");
  ```

  In `scripts/set-supported-codex-version.sh`, keep the existing atomic staging/rollback and validate complete ASCII SemVer 2.0 grammar:

  ```bash
  core='(0|[1-9][0-9]*)'
  prerelease_identifier='(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
  prerelease="${prerelease_identifier}(\\.${prerelease_identifier})*"
  build='[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*'
  semver="^${core}\\.${core}\\.${core}(-${prerelease})?(\\+${build})?$"
  if ! [[ "$version" =~ $semver ]]; then
    printf 'VERSION must be canonical SemVer: %s\n' "$version" >&2
    exit 2
  fi
  replacement="- Native app-server protocol validated against Codex \`$version\`. $marker"
  ```

  Refresh `Cargo.lock` once with `cargo check`, then require `cargo check --locked`.

- [ ] **Step 4: Run the focused GREEN/PASS command for the version pipeline**

  Run: `cargo test --locked --test workflow_contract tested_codex_version -- --nocapture`
  Expected: PASS for canonical prerelease/build acceptance, unsafe/noncanonical rejection, exact non-mutation on rejection, and transactional rollback.

- [ ] **Step 5: Pin and recapture; never hand-edit evidence**

  Run:

  ```bash
  ./scripts/set-supported-codex-version.sh 0.150.0-alpha.12.2
  env \
    PATH="/opt/codex-desktop/resources:$PATH" \
    CODEX_SESSION_CONTROL_FIXTURE_OUT="$PWD/tests/fixtures/app-server-contract.json" \
    cargo test --locked --bin codex-session-control \
      app_server::tests::live_capture::capture_protocol_fixture -- \
      --exact --ignored --nocapture
  ```

  Expected: setter reports `0.150.0-alpha.12.2`; ignored capture PASS; fixture `codexVersion`, initialize `userAgent`, schema digest, and exemplars are produced by `/opt/codex-desktop/resources/codex` and match the new pin.

- [ ] **Step 6: Commit the version slice**

  ```bash
  git add Cargo.toml Cargo.lock build.rs supported-codex-version.txt \
    scripts/set-supported-codex-version.sh tests/workflow_contract.rs README.md \
    src/app_server/tests/live_capture.rs tests/fixtures/app-server-contract.json
  git commit -m "feat(protocol): support canonical prerelease versions"
  ```

### Task 2: Deterministic Desktop Endpoint and Security Predicates

**Owner:** Endpoint/transport worker
**Files:**
- Create: `src/app_server/endpoint.rs`
- Modify: `src/app_server.rs` module declaration only until Task 3

- [ ] **Step 1: Write exhaustive RED tables at the narrow resolver/validator boundary**

  Define unit tables around an injected environment lookup rather than mutating process-global variables in parallel tests:

  ```rust
  #[test]
  fn explicit_socket_precedes_all_derived_inputs() {
      let endpoint = resolve_with(|name| match name {
          EXPLICIT_SOCKET => Some(OsString::from("/run/user/1000/custom/app.sock")),
          XDG_RUNTIME_DIR => panic!("explicit endpoint must short-circuit XDG"),
          APP_ID => panic!("explicit endpoint must short-circuit app ID"),
          _ => None,
      }).unwrap();
      assert_eq!(endpoint.socket_path(), Path::new("/run/user/1000/custom/app.sock"));
  }

  #[test]
  fn empty_explicit_socket_falls_through_to_derived_resolution() {
      let endpoint = resolve_with(|name| match name {
          EXPLICIT_SOCKET => Some(OsString::new()),
          XDG_RUNTIME_DIR => Some(OsString::from("/run/user/1000")),
          APP_ID => Some(OsString::from("codex-desktop")),
          _ => None,
      }).unwrap();
      assert_eq!(
          endpoint.socket_path(),
          Path::new("/run/user/1000/codex-desktop/app-server-bridge/app-server.sock")
      );
  }

  #[test]
  fn derived_endpoint_uses_default_and_validated_app_ids() {
      for app_id in [None, Some(OsString::new())] {
          let endpoint = resolve_with(|name| match name {
              XDG_RUNTIME_DIR => Some(OsString::from("/run/user/1000")),
              APP_ID => app_id.clone(),
              _ => None,
          }).unwrap();
          assert_eq!(
              endpoint.socket_path(),
              Path::new("/run/user/1000/codex-desktop/app-server-bridge/app-server.sock")
          );
      }
      for rejected in [".", "..", "bad/id", "bad id", "é"] {
          let result = resolve_with(|name| match name {
              XDG_RUNTIME_DIR => Some(OsString::from("/run/user/1000")),
              APP_ID => Some(OsString::from(rejected)),
              _ => None,
          });
          assert!(result.is_err(), "accepted app ID {rejected:?}");
      }
  }
  ```

  Add an invalid-UTF-8 app-ID case with `OsStringExt::from_vec(vec![0xff])`. Table-test raw metadata predicates with a synthetic foreign UID so ownership coverage never depends on permission to call `lchown`. Add real filesystem tables for relative paths, `.`, `..`, repeated/trailing separators, `/` runtime, symlink at every selected component, wrong types, directory modes other than exact `(mode & 0o7777) == 0o0700`, socket modes other than exact `(mode & 0o7777) ∈ {0o0600, 0o0700}`, missing parent/socket, and parent canonicalization replacement.

- [ ] **Step 2: Run RED**

  Run: `cargo test --locked --bin codex-session-control app_server::endpoint::tests::`
  Expected: FAIL because `app_server::endpoint` and its resolver/validator do not exist.

- [ ] **Step 3: Implement the single endpoint seam**

  Use real current error types and keep test injection private:

  ```rust
  pub(crate) const EXPLICIT_SOCKET: &str = "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET";
  pub(crate) const XDG_RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";
  pub(crate) const APP_ID: &str = "CODEX_LINUX_APP_ID";

  #[derive(Clone, Debug, Eq, PartialEq)]
  enum EndpointKind {
      Explicit,
      Derived {
          runtime_dir: PathBuf,
          app_dir: PathBuf,
          bridge_dir: PathBuf,
      },
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub(crate) struct DesktopEndpoint {
      socket_path: PathBuf,
      kind: EndpointKind,
  }

  impl DesktopEndpoint {
      pub(super) fn explicit(socket_path: PathBuf) -> Self {
          Self {
              socket_path,
              kind: EndpointKind::Explicit,
          }
      }

      pub(crate) fn resolve() -> Result<Self, ToolErrorData> {
          resolve_with(std::env::var_os)
      }

      pub(crate) fn socket_path(&self) -> &Path {
          &self.socket_path
      }

      pub(crate) fn validate(&self) -> Result<(), ToolErrorData> {
          validate_selected_tree(self)
      }
  }

  fn owned_directory_is_private(st_uid: u32, st_mode: u32, euid: u32) -> bool {
      FileType::from_raw_mode(st_mode) == FileType::Directory
          && st_uid == euid
          && st_mode & 0o7777 == 0o0700
  }

  fn owned_socket_is_private(st_uid: u32, st_mode: u32, euid: u32) -> bool {
      FileType::from_raw_mode(st_mode) == FileType::Socket
          && st_uid == euid
          && matches!(st_mode & 0o7777, 0o0600 | 0o0700)
  }

  ```

  Define `fn resolve_with(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Result<DesktopEndpoint, ToolErrorData>` and `fn validate_selected_tree(endpoint: &DesktopEndpoint) -> Result<(), ToolErrorData>` directly. The production resolver calls `DesktopEndpoint::explicit` after normalizing a selected explicit path; the same narrow constructor lets the parent module's existing fake client provide a fixed test path without exposing fields or mutating process-global environment. It has no test-only behavior and always represents `EndpointKind::Explicit`. `resolve_with` reads `OsString`; a zero-length explicit value falls through to derivation, a missing or zero-length XDG value returns `TargetUnavailable`, and a missing or zero-length app ID defaults to `codex-desktop`. It normalizes by rebuilding root plus only `Component::Normal`, requires byte-for-byte equality with the original `OsString`, and validates nonempty app-ID bytes without adding a regex dependency. `validate_selected_tree` uses `rustix::fs::lstat`, `std::fs::canonicalize`, and `rustix::process::geteuid`; it maps missing selected endpoint/parent to `TargetUnavailable` and all malformed, unsafe, or raced metadata to `AuthorityTransportFailure` at `socket_validation`. Error messages contain no path or environment value.

- [ ] **Step 4: Run the focused GREEN/PASS command**

  Run:

  ```bash
  cargo test --locked --bin codex-session-control app_server::endpoint::tests::
  ```

  Expected: PASS across precedence, derivation, lexical normalization, app-ID, owner/mode/type/symlink/absence/replacement, and redaction cases.

- [ ] **Step 5: Commit the endpoint slice**

  ```bash
  git add src/app_server.rs src/app_server/endpoint.rs
  git commit -m "feat(runtime): resolve the Desktop shared socket"
  ```

### Task 3: Exact `/rpc` Transport and Complete Client-Seam Migration

**Owner:** One runtime migration worker owns the complete atomic slice. It may reason through process, transport, and MCP substeps sequentially, but it does not commit or hand back an uncompilable intermediate tree.
**Files:**
- Modify: `src/main.rs`
- Modify: `src/app_server.rs`
- Modify: `src/app_server/tests.rs`
- Modify: `src/app_server/tests/transport.rs`
- Modify: `src/app_server/tests/live_capture.rs`
- Modify: `src/mcp.rs`
- Modify: `src/mcp/operations.rs`
- Modify: `src/mcp/tests/support.rs`
- Modify: `src/mcp/tests/descendant_interrupt.rs`
- Modify: `src/mcp/tests/goal_matrix.rs`
- Modify: `src/mcp/tests/mutation_mapping.rs`
- Modify: `src/mcp/tests/outcome_unknown.rs`
- Modify: `src/mcp/tests/read_tools.rs`
- Modify: `src/mcp/tests/threads_wait.rs`
- Modify: `src/mcp/tests/timeout.rs`
- Modify: `tests/app_server_integration/live_harness.rs` exact URL only; Task 15 owns the later rewrite
- Create: `tests/desktop_shared_socket_contract.rs`

- [ ] **Step 1: Write RED transport and seam tests**

  Make the fake server inspect the HTTP upgrade and record the target:

  ```rust
  let websocket = accept_hdr_async(stream, |request: &Request, response: Response| {
      assert_eq!(request.uri().path_and_query().map(|value| value.as_str()), Some("/rpc"));
      Ok(response)
  }).await.unwrap();
  ```

  Add `websocket_upgrade_uses_exact_rpc_path`, with negative controls that a fake strict server rejects `/`, `/rpc?x=1`, and `/other`. In MCP tests, replace every `ProductConfig` call site with a concrete fake-server client and assert `execute_tool` receives `&AppServerClient` directly. Process-isolated cases in `tests/desktop_shared_socket_contract.rs` launch the direct no-argument binary with controlled environment, then assert explicit/derived precedence and public error redaction.

- [ ] **Step 2: Run RED**

  Run:

  ```bash
  cargo test --locked --bin codex-session-control \
    app_server::tests::transport::websocket_upgrade_uses_exact_rpc_path -- --exact
  cargo test --locked --bin codex-session-control mcp::tests::read_tools::
  ```

  Expected: FAIL because production upgrades `/`, `execute_tool` still requires `ProductConfig`, and tests still construct clients through `harness.config`.

- [ ] **Step 3: Make `AppServerClient` resolve on every connection**

  First replace `src/main.rs` with this direct entrypoint; it deliberately leaves argument rejection for Task 6's RED process contract. This stops compiling obsolete install/desktop/CLI call sites before the client constructor changes:

  ```rust
  mod app_server;
  mod error;
  mod mcp;
  mod model;
  #[cfg(test)]
  mod test_support;

  #[tokio::main]
  async fn main() {
      if let Err(error) = run_mcp_server().await {
          eprintln!("{error}");
          std::process::exit(1);
      }
  }

  async fn run_mcp_server() -> Result<(), String> {
      let running = rmcp::serve_server(
          crate::mcp::SessionControlMcp::new(),
          rmcp::transport::stdio(),
      ).await.map_err(|error| error.to_string())?;
      running.waiting().await.map(|_| ()).map_err(|error| error.to_string())
  }
  ```

  Preserve the public client/connection method surface used by retained operations while replacing saved config with a production endpoint source and a test-only fixed source:

  ```rust
  #[derive(Clone, Debug)]
  pub struct AppServerClient {
      endpoint_source: EndpointSource,
      product_version: String,
      tested_codex_version: String,
      #[cfg(test)]
      failure_point: FailurePoint,
  }

  #[derive(Clone, Debug)]
  enum EndpointSource {
      Desktop,
      #[cfg(test)]
      Fixed(DesktopEndpoint),
  }

  impl EndpointSource {
      fn resolve(&self) -> Result<DesktopEndpoint, ToolErrorData> {
          match self {
              Self::Desktop => DesktopEndpoint::resolve(),
              #[cfg(test)]
              Self::Fixed(endpoint) => Ok(endpoint.clone()),
          }
      }
  }

  impl AppServerClient {
      fn new(
          endpoint_source: EndpointSource,
          product_version: &str,
          tested_codex_version: &str,
      ) -> Self {
          Self {
              endpoint_source,
              product_version: product_version.to_owned(),
              tested_codex_version: tested_codex_version.to_owned(),
              #[cfg(test)]
              failure_point: FailurePoint::Never,
          }
      }

      pub fn desktop() -> Self {
          Self::new(EndpointSource::Desktop, env!("CARGO_PKG_VERSION"), TESTED_CODEX_VERSION)
      }

      #[cfg(test)]
      pub(crate) fn for_test(socket_path: PathBuf, tested: &str) -> Self {
          Self::new(
              EndpointSource::Fixed(DesktopEndpoint::explicit(socket_path)),
              env!("CARGO_PKG_VERSION"),
              tested,
          )
      }

      pub async fn connect_initialized(&self) -> Result<AppServerConnection, ToolErrorData> {
          let endpoint = self.endpoint_source.resolve()?;
          endpoint.validate()?;
          let (websocket, _) = with_native_stage_timeout("connect", async {
              let stream = UnixStream::connect(endpoint.socket_path())
                  .await
                  .map_err(|_| transport_error("connect"))?;
              client_async("ws://localhost/rpc", stream)
                  .await
                  .map_err(|_| transport_error("connect"))
          }).await?;
          let mut connection = AppServerConnection::new(websocket);
          #[cfg(test)]
          {
              connection.failure_point = self.failure_point;
          }
          with_native_stage_timeout("initialize", async {
              connection.initialize(&self.product_version, &self.tested_codex_version).await
          }).await?;
          Ok(connection)
      }
  }
  ```

  Define the private real signatures `fn AppServerConnection::new(websocket: ClientWebSocket) -> Self` and `async fn AppServerConnection::initialize(&mut self, product_version: &str, tested_codex_version: &str) -> Result<(), ToolErrorData>` by moving the existing inline connection construction and initialize/initialized exchange without semantic changes. The production `EndpointSource::Desktop` calls `DesktopEndpoint::resolve()` every time. `Fixed` is compiled only for unit tests and still calls fresh metadata validation before every connection. Keep request correlation, timeouts, dispatch-state-before-write, native error mapping, warning text, and existing connection methods unchanged. Replace exact saved-home equality with a requirement that initialize contains a nonempty absolute `codexHome`; malformed or missing `codexHome` remains `TargetUnavailable` at `initialize`. Preserve current user-agent version extraction: an absent/unparseable version becomes the existing `unknown` mismatch warning rather than a new hard failure.

- [ ] **Step 4: Migrate production dispatch and every test call site**

  Remove `install::load_installed_config` and `model::ProductConfig` from `src/mcp.rs`:

  ```rust
  async fn execute_validated(
      tool: &str,
      validated: ValidatedInput,
  ) -> Result<CallToolResult, McpError> {
      let client = AppServerClient::desktop();
      execute_tool(tool, validated, &client).await
  }

  ```

  Change the existing private executor signature to `async fn execute_tool(tool: &str, validated: ValidatedInput, client: &AppServerClient) -> Result<CallToolResult, McpError>`, replace only its config/client opening with `let mut connection = client.connect_initialized().await.map_err(|error| response_error(tool, error, None))?;`, and keep all thirteen existing `match validated` arms inline and contract-identical. Give `FakeAppServer` `pub(super) fn client(&self) -> AppServerClient` and migrate all compiled direct constructors in the files listed above. Outcome reconciliation and descendant fan-out continue to call `client.connect_initialized()` so they get freshly resolved/validated connections. `threads_wait` retains one connection for the in-flight wait. Change retained native/live connection URL to `ws://localhost/rpc`.

- [ ] **Step 5: Run the complete focused GREEN/PASS commands**

  Run:

  ```bash
  cargo test --locked --bin codex-session-control app_server::tests::transport::
  cargo test --locked --bin codex-session-control mcp::tests::
  cargo test --locked --test desktop_shared_socket_contract
  rg -n 'ProductConfig|from_config|load_installed_config|ws://localhost/' src/mcp.rs src/mcp src/app_server.rs src/app_server tests/app_server_integration
  ```

  Expected: tests PASS; the search returns no legacy config/client construction and every remaining WebSocket literal is exactly `ws://localhost/rpc`.

- [ ] **Step 6: Commit the complete compile-safe migration atomically**

  ```bash
  git add src/main.rs src/app_server.rs src/app_server/tests.rs src/app_server/tests/transport.rs \
    src/app_server/tests/live_capture.rs \
    src/mcp.rs src/mcp/operations.rs src/mcp/tests \
    tests/app_server_integration/live_harness.rs tests/desktop_shared_socket_contract.rs
  git commit -m "refactor(runtime): use the Desktop-owned rpc client"
  ```

### Task 4: Review Milestone M1 — Foundation

**Covers:** Tasks 1-3
**Review contract:** `## Review Milestones` -> `M1`

- [ ] Confirm branch/base, path ownership, commits, RED/GREEN evidence, exact fixture provenance, and focused tests are complete.
- [ ] Dispatch one combined reviewer over `88f2ac5b124abaa2a355ca88304e9439c692eb0a..HEAD`; review specification compliance first, then inventory new version/endpoint/client constructs for DRY/YAGNI, then code quality.
- [ ] Require explicit checks for explicit-socket short-circuiting, lexical byte equality, every derived directory predicate, error redaction, exact `/rpc`, initialize identity, all test call sites, and fresh resolution in reconciliation/descendant paths.
- [ ] Fix valid findings, rerun affected focused gates, and repeat the review after Critical/Important findings. Minor-only fixes require affected verification but no repeat.
- [ ] Mark M1 complete only when no material finding remains; continue directly to Task 5.

### Task 5: Persisted `notLoaded` Resume Before Message Start

**Owner:** MCP seam worker
**Files:**
- Modify: `src/mcp/operations.rs`
- Modify: `src/mcp/tests/mutation_mapping.rs`
- Modify: `src/mcp/tests/outcome_unknown.rs`
- Modify: `src/mcp/tests/support.rs` only when a response helper is required

- [ ] **Step 1: Write the full RED resume matrix**

  Add exact scripted tests named:

  ```rust
  not_loaded_message_resumes_before_starting
  not_loaded_message_rejects_a_different_resumed_thread_without_starting
  not_loaded_message_rejects_a_resume_that_remains_not_loaded
  not_loaded_message_reports_an_active_resume_as_a_native_conflict
  not_loaded_message_propagates_resume_failure_without_starting
  not_loaded_message_resume_transport_failure_after_write_sends_no_prompt
  not_loaded_message_rejects_malformed_resume_without_starting
  system_error_message_reads_once_then_starts_with_overrides
  ```

  The success script must assert one connection and this order:

  ```rust
  [
      FakeStep::result("thread/read", json!({"threadId": id, "includeTurns": false}), compact_not_loaded),
      FakeStep::result("thread/turns/list", json!({"threadId": id, "limit": 1, "itemsView": "notLoaded"}), turns),
      FakeStep::result("thread/resume", json!({"threadId": id, "excludeTurns": true}), json!({"thread": native_thread(id, json!({"type": "idle"}), 2)})),
      FakeStep::result("turn/start", expected_original_prompt_params, turn_started),
  ]
  ```

  Every rejection asserts the fake log contains no `turn/start` or `turn/steer`. The transport-after-write case must accept the exact `thread/resume` request and then close before sending a correlated response; it asserts transport failure at `thread/resume` and zero prompt dispatch. The `SystemError` case locks the retained compact-read plus one `turn/start` behavior separately from `Idle`, so splitting the old combined match arm cannot silently drop it.

- [ ] **Step 2: Run RED**

  Run: `cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::not_loaded_message_`
  Expected: FAIL because current `NotLoaded | Idle | SystemError` goes directly to `turn/start`.

- [ ] **Step 3: Implement exact-ID resume on the same connection**

  Split `NotLoaded` from the retained idle/system-error arm and use existing real types:

  ```rust
  async fn resume_not_loaded_thread(
      connection: &mut AppServerConnection,
      requested_id: &str,
  ) -> Result<(), ToolErrorData> {
      let response: Value = connection.request(
          "thread/resume",
          json!({"threadId": requested_id, "excludeTurns": true}),
      ).await.map_err(|mut error| {
          error.tool = "thread_message_send".to_owned();
          error.thread_id = Some(requested_id.to_owned());
          error
      })?;
      let thread = response.get("thread")
          .ok_or_else(|| malformed_result("thread_message_send", "thread/resume"))
          .and_then(|value| thread_from_native(value, "thread/resume"))
          .map_err(|mut error| {
              error.tool = "thread_message_send".to_owned();
              error.stage = "thread/resume".to_owned();
              error.thread_id = Some(requested_id.to_owned());
              error
          })?;
      if thread.id != requested_id || matches!(thread.status, ThreadStatus::NotLoaded) {
          let mut error = malformed_result("thread_message_send", "thread/resume");
          error.thread_id = Some(requested_id.to_owned());
          return Err(error);
      }
      if matches!(thread.status, ThreadStatus::Active { .. }) {
          let mut error = ToolErrorData::fixed(
              ToolErrorCategory::NativeConflict,
              "thread_message_send",
              "thread/resume",
          );
          error.thread_id = Some(requested_id.to_owned());
          return Err(error);
      }
      Ok(())
  }
  ```

  Call this only for `ThreadStatus::NotLoaded`, before constructing the existing `turn/start` mutation. Keep original prompt/model/effort, `CompactThreadRead` reconciliation, dispatch-state-before-write, and exact one `turn/start`; never call `mutation_request` for preparatory `thread/resume`.

- [ ] **Step 4: Run the focused GREEN/PASS commands and retained non-replay coverage**

  Run:

  ```bash
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::not_loaded_message_
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::idle_message_reads_once_then_starts_with_overrides -- --exact
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::system_error_message_reads_once_then_starts_with_overrides -- --exact
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::message_race_never_retries_the_opposite_operation -- --exact
  cargo test --locked --bin codex-session-control mcp::tests::outcome_unknown::
  ```

  Expected: PASS; failed resume paths record zero prompt dispatch, and successful resume records one prompt.

- [ ] **Step 5: Commit**

  ```bash
  git add src/mcp/operations.rs src/mcp/tests/mutation_mapping.rs \
    src/mcp/tests/outcome_unknown.rs src/mcp/tests/support.rs
  git commit -m "fix(mcp): resume persisted threads before messaging"
  ```

### Task 6: Direct Stdio Binary and Exact Process Contract

**Owner:** Process/contract worker
**Files:**
- Modify: `src/main.rs`
- Modify: `tests/mcp_contract.rs`

- [ ] **Step 1: Convert process tests to no-argument invocation and add rejection coverage**

  Update `public_catalog_is_exact` and child-guard cases to launch `cargo_bin("codex-session-control")` without `mcp-server` or `--verbose`. Add:

  ```rust
  #[test]
  fn binary_is_direct_stdio_and_accepts_no_commands() {
      let output = Command::new(cargo_bin("codex-session-control"))
          .arg("mcp-server")
          .output()
          .unwrap();
      assert_eq!(output.status.code(), Some(2));
      assert!(output.stdout.is_empty());
      assert!(String::from_utf8(output.stderr).unwrap().contains("does not accept arguments"));
  }
  ```

  Retain assertions that stdout contains only JSON-RPC, stderr contains no MCP result framing, the child has no descendants, and stdin EOF reaps the process.

- [ ] **Step 2: Run RED**

  Run:

  ```bash
  cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
  cargo test --locked --test mcp_contract binary_is_direct_stdio_and_accepts_no_commands -- --exact
  ```

  Expected: `public_catalog_is_exact` may PASS after Task 3's lifecycle detachment; `binary_is_direct_stdio_and_accepts_no_commands` FAILS because the initial direct entrypoint ignores `mcp-server` and exits normally at EOF instead of rejecting arguments.

- [ ] **Step 3: Replace the CLI entrypoint with one direct server**

  Keep only retained modules and a small stderr-only error boundary:

  ```rust
  mod app_server;
  mod error;
  mod mcp;
  mod model;
  #[cfg(test)]
  mod test_support;

  #[tokio::main]
  async fn main() {
      if std::env::args_os().len() != 1 {
          eprintln!("codex-session-control is a stdio MCP server and does not accept arguments");
          std::process::exit(2);
      }
      if let Err(error) = run_mcp_server().await {
          eprintln!("{error}");
          std::process::exit(1);
      }
  }

  async fn run_mcp_server() -> Result<(), String> {
      let running = rmcp::serve_server(
          crate::mcp::SessionControlMcp::new(),
          rmcp::transport::stdio(),
      ).await.map_err(|error| error.to_string())?;
      running.waiting().await.map(|_| ()).map_err(|error| error.to_string())
  }
  ```

  Do not spawn Desktop, Codex, a wrapper, service manager, proxy, or another CSC process.

- [ ] **Step 4: Run the focused GREEN/PASS commands**

  Run:

  ```bash
  cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
  cargo test --locked --test mcp_contract binary_is_direct_stdio_and_accepts_no_commands -- --exact
  cargo test --locked --test mcp_contract child_guard_ -- --nocapture
  cargo test --locked --test desktop_shared_socket_contract
  ```

  Expected: PASS with 13 tools, no children, MCP-only stdout, diagnostic-only stderr, and clean EOF exit.

- [ ] **Step 5: Commit**

  ```bash
  git add src/main.rs tests/mcp_contract.rs
  git commit -m "refactor(cli): run stdio MCP directly"
  ```

### Task 7: Restart Recovery, In-Flight Wait Failure, and No Replay

**Owners:** Endpoint/transport worker and MCP seam worker, sequential by file ownership
**Files:**
- Modify if the RED test exposes a production violation: `src/app_server.rs`
- Modify: `src/app_server/tests.rs`
- Modify: `src/app_server/tests/transport.rs`
- Modify if the RED test exposes a production violation: `src/mcp/operations.rs`
- Modify: `src/mcp/tests/threads_wait.rs`
- Modify: `src/mcp/tests/outcome_unknown.rs`
- Modify: `src/mcp/tests/descendant_interrupt.rs`
- Modify: `tests/desktop_shared_socket_contract.rs`

- [ ] **Step 1: Add RED regression tests for replacement and disconnect**

  Add exact tests:

  ```rust
  future_operation_re_resolves_replaced_socket
  stdio_host_recovers_on_next_call_after_socket_replacement
  in_flight_wait_disconnect_is_not_replayed
  reconciliation_reconnects_read_only_after_socket_replacement
  ambiguous_mutation_replacement_never_replays_the_write
  ```

  `future_operation_re_resolves_replaced_socket` completes one call, drops the first listener/socket, binds a new socket at the same path, and completes a second call through the same client. `stdio_host_recovers_on_next_call_after_socket_replacement` starts the no-argument binary once, completes MCP initialize plus one read tool against the first strict fake authority, replaces the socket at the same path, completes a second independent read tool against the replacement, and asserts the child PID did not change. The wait case scripts a disconnect after the wait request and asserts total connection count remains one. The ambiguous mutation case records exactly one mutation method and permits only a read-only reconciliation method on the replacement authority.

- [ ] **Step 2: Run RED**

  Run:

  ```bash
  cargo test --locked --bin codex-session-control future_operation_re_resolves_replaced_socket -- --exact
  cargo test --locked --test desktop_shared_socket_contract stdio_host_recovers_on_next_call_after_socket_replacement -- --exact
  cargo test --locked --bin codex-session-control in_flight_wait_disconnect_is_not_replayed -- --exact
  cargo test --locked --bin codex-session-control ambiguous_mutation_replacement_never_replays_the_write -- --exact
  ```

  Expected: FAIL to compile because the new replacement/listener-rotation fake API does not exist; after that harness compiles, a behavior failure proves cached endpoint/inode/initialize state or an accidental retry loop.

- [ ] **Step 3: Make the structural boundary minimal**

  Add a test-only `async fn ReplacementHarness::replace_socket(&mut self, script: FakeScript)` that closes and joins the current fake listener, unlinks only its owned socket, binds a new strict `/rpc` listener at the same path, reapplies mode `0600`, and preserves the same endpoint environment. Production changes are allowed only if the now-compiling RED test exposes a violation. The required implementation is the Task 3 seam: each `connect_initialized()` resolves and validates anew; the top-level call owns one initialized connection; `threads_wait` receives only `&mut AppServerConnection`; `mutation_request` performs one `mutate`; `reconcile_request` creates one fresh connection and performs only `request`.

  ```rust
  pub(super) async fn reconcile_request(
      client: &AppServerClient,
      method: &'static str,
      params: Value,
  ) -> Result<Value, ToolErrorData> {
      let mut connection = client.connect_initialized().await?;
      connection.request(method, params).await
  }
  ```

  Do not add retry middleware, a cached endpoint, cached canonical parent, cached inode, connection pool, or mutation replay branch.

- [ ] **Step 4: Run the focused GREEN/PASS commands**

  Run:

  ```bash
  cargo test --locked --bin codex-session-control app_server::tests::transport::
  cargo test --locked --bin codex-session-control mcp::tests::threads_wait::
  cargo test --locked --bin codex-session-control mcp::tests::outcome_unknown::
  cargo test --locked --bin codex-session-control mcp::tests::descendant_interrupt::
  cargo test --locked --test desktop_shared_socket_contract stdio_host_recovers_on_next_call_after_socket_replacement -- --exact
  ```

  Expected: PASS; future calls recover, waits fail once, mutations write once maximum, and read-only reconciliation never asserts acceptance or rejection without existing proof.

- [ ] **Step 5: Commit only actual test and minimal production changes**

  ```bash
  git add src/app_server.rs src/app_server/tests.rs src/app_server/tests/transport.rs \
    src/mcp/operations.rs src/mcp/tests/threads_wait.rs \
    src/mcp/tests/outcome_unknown.rs src/mcp/tests/descendant_interrupt.rs \
    tests/desktop_shared_socket_contract.rs
  git commit -m "test(runtime): prove restart recovery without replay"
  ```

### Task 8: Review Milestone M2 — Retained Runtime

**Covers:** Tasks 5-7
**Review contract:** `## Review Milestones` -> `M2`

- [ ] Dispatch one combined reviewer over the incremental M2 surface; check spec compliance, then DRY/YAGNI, then code quality.
- [ ] Require the reviewer to trace one successful and every rejected `notLoaded` path, every possible prompt write, future-operation replacement, in-flight wait disconnect, outcome-unknown dispatch evidence, descendant fan-out, and read-only reconciliation.
- [ ] Reject any retry abstraction, connection pool, cached endpoint, fallback thread, target substitution, or second prompt path.
- [ ] Fix valid findings, rerun affected tests, repeat after Critical/Important findings, and continue only when no material finding remains.

### Task 9: Clone-Local Legacy Plugin, Native-Build Installer, and Packaging Contracts

**Owner:** Packaging worker
**Files:**
- Create: `.agents/plugins/marketplace.json`
- Create: `plugins/codex-session-control/.codex-plugin/plugin.json`
- Create: `plugins/codex-session-control/.mcp.json`
- Create: `plugins/codex-session-control/bin/.gitignore`
- Create: `scripts/install-local-plugin.sh`
- Create: `tests/plugin_packaging_contract.rs`

- [ ] **Step 1: Write RED structural, installer, cache, and generic-host contracts**

  Add isolated tests named:

  ```rust
  root_manifests_are_exact_and_versioned
  installer_rejects_unsupported_host_before_codex_mutation
  installer_builds_locked_and_atomically_stages_native_executable
  installer_same_root_restages_and_does_not_duplicate_marketplace
  installer_rejects_marketplace_collision_before_plugin_mutation
  installer_suppresses_mise_advisory_for_machine_json
  installer_rejects_invalid_machine_json_before_mutation
  legacy_plugin_host_contract_on_codex_0_149_1
  generic_client_initializes_and_lists_exact_catalog_from_another_cwd
  ```

  Use private isolated `HOME` and `CODEX_HOME`, a command-recording fake `codex` for installer branches, and an ignored Codex 0.149.1 host test for actual cache/environment behavior. `root_manifests_are_exact_and_versioned` owns all repository manifest structure in one place: marketplace/plugin identity, version, source path, command, cwd, exact three-variable forwarding list, and timeout. `legacy_plugin_host_contract_on_codex_0_149_1` uniquely proves the real host's contained relative execution, private cache copy, environment forwarding, refresh, v1 negative control, and removal behavior. The fake must reproduce the observed mise stdout advisory unless `MISE_QUIET=1`, and separately return permanently malformed JSON to prove parsing failure causes zero marketplace/plugin mutation. Hash and mode assertions must compare the staged executable and Codex cache copy before/after same-version and manifest-version refresh. Generic-client coverage invokes the staged binary from an unrelated cwd and sends MCP `initialize` plus `tools/list`.

  The ignored real-host contract requires all three exact opt-ins before doing anything: `CODEX_SESSION_CONTROL_PLUGIN_HOST_CONTRACT=1`, `CODEX_SESSION_CONTROL_CODEX_0_149_1_BIN=/absolute/direct/codex`, and `CODEX_SESSION_CONTROL_PLUGIN_HOST_AUTH_JSON=/absolute/auth.json`. Reject a wrapper, symlink, wrong owner/type/mode, nonabsolute path, or any binary whose direct `--version` output is not exactly `codex-cli 0.149.1`. Create one private mode-0700 temp root, isolated `HOME` and `CODEX_HOME`, and an isolated marketplace; copy the auth source to isolated `CODEX_HOME/auth.json` as a new regular mode-0600 file without printing its contents, path, size, or hash. Invoke only the verified direct binary under `env_clear` with exact isolated environment, use `codex exec --ephemeral --skip-git-repo-check --json` only for the temporary read-only probe call, and never point a command at normal Codex state. The test-owned probe mirrors a complete MCP read-only tool response and exists only to observe contained cwd/argv and the three sentinels; it is not production code. RAII cleanup must reap the probe/CLI children and delete the whole private tree on success, failure, or panic; output and failure messages must not include credential bytes or values. The orchestrator resolves the current direct binary with `mise where codex@0.149.1` and supplies the normal auth file path only through the opt-in variable.

- [ ] **Step 2: Run RED**

  Run: `cargo test --locked --test plugin_packaging_contract`
  Expected: FAIL because the root manifests, checkout installer, and packaging test target do not exist.

- [ ] **Step 3: Create exact manifests**

  Write the marketplace and legacy plugin manifests exactly:

  ```json
  {
    "name": "codex-session-control-local",
    "interface": {
      "displayName": "Codex session control"
    },
    "plugins": [
      {
        "name": "codex-session-control",
        "source": {
          "source": "local",
          "path": "./plugins/codex-session-control"
        },
        "policy": {
          "installation": "AVAILABLE"
        },
        "category": "Coding"
      }
    ]
  }
  ```

  ```json
  {
    "name": "codex-session-control",
    "version": "0.3.2",
    "description": "Control Codex sessions via MCP",
    "author": {
      "name": "Agentlehub"
    },
    "license": "MIT",
    "mcpServers": "./.mcp.json",
    "interface": {
      "displayName": "Codex session control",
      "shortDescription": "Control Codex sessions via MCP",
      "category": "Coding",
      "capabilities": ["Read", "Write"]
    }
  }
  ```

  Write the MCP manifest exactly:

  ```json
  {
    "mcpServers": {
      "codex-session-control": {
        "type": "stdio",
        "command": "./bin/codex-session-control",
        "cwd": ".",
        "env_vars": [
          "XDG_RUNTIME_DIR",
          "CODEX_LINUX_APP_ID",
          "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET"
        ],
        "tool_timeout_sec": 86460
      }
    }
  }
  ```

  Marketplace name is `codex-session-control-local`, source is `./plugins/codex-session-control`, and legacy `plugin.json` has a concrete version exactly equal to `Cargo.toml`. `plugins/codex-session-control/bin/.gitignore` ignores only `/codex-session-control` and keeps the directory marker tracked.

- [ ] **Step 4: Implement one fail-closed installer**

  Preserve a single straight-line Bash entrypoint with branch functions that each own a proven contract:

  ```bash
  script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
  clone_root="$(cd -- "$script_dir/.." && pwd -P)"
  plugin_root="$clone_root/plugins/codex-session-control"
  staged_binary="$plugin_root/bin/codex-session-control"

  case "$(uname -m)" in
    x86_64|amd64) expected_machine='Advanced Micro Devices X86-64' ;;
    aarch64|arm64) expected_machine='AArch64' ;;
    *) printf 'Unsupported architecture: %s\n' "$(uname -m)" >&2; exit 2 ;;
  esac

  (cd "$clone_root" && cargo build --release --locked)
  candidate="$clone_root/target/release/codex-session-control"
  test -f "$candidate" && test -x "$candidate" && test ! -L "$candidate"
  readelf --file-header "$candidate" | grep -F "Machine:" | grep -F "$expected_machine" >/dev/null
  stage="$(mktemp "$plugin_root/bin/.codex-session-control.XXXXXX")"
  trap 'rm -f -- "${stage:-}"' EXIT
  cp -- "$candidate" "$stage"
  chmod 0755 "$stage"
  mv -fT -- "$stage" "$staged_binary"
  stage=
  ```

  Parse/validate all three JSON manifests with `jq`; reject unexpanded template tokens, symlinks, noncanonical clone roots, manifest path escape, version drift, or command/env drift before running Codex. The operator's real `codex` launcher is a mise wrapper that writes an advisory to stdout unless `MISE_QUIET=1`; every machine-readable Codex invocation must suppress that wrapper advisory and then require the entire captured stdout to be valid JSON. Never parse through process substitution, because Bash would mask a failing `jq` as an empty marketplace list and mutate fail-open. Use one justified wrapper and compare the listed root inside `jq` without line-splitting paths:

  ```bash
  run_codex() {
    env MISE_QUIET=1 codex "$@"
  }

  marketplace_json="$(run_codex plugin marketplace list --json)"
  marketplace_count="$(jq -er --arg name codex-session-control-local \
    '.marketplaces | arrays | map(select(.name == $name)) | length' \
    <<<"$marketplace_json")" || {
      printf '%s\n' 'Codex marketplace listing was not valid machine-readable JSON.' >&2
      exit 1
    }
  case "$marketplace_count" in
    0)
      run_codex plugin marketplace add "$clone_root" --json >/dev/null
      ;;
    1)
      jq -e --arg name codex-session-control-local --arg root "$clone_root" \
        '.marketplaces | map(select(.name == $name)) | .[0].root == $root' \
        <<<"$marketplace_json" >/dev/null || {
          printf '%s\n' 'Marketplace name already targets another root.' >&2
          exit 1
        }
      ;;
    *)
      printf '%s\n' 'Marketplace name resolves to multiple roots.' >&2
      exit 1
      ;;
  esac
  marketplace_json="$(run_codex plugin marketplace list --json)"
  jq -e --arg name codex-session-control-local --arg root "$clone_root" \
    '.marketplaces | arrays | map(select(.name == $name))
      | length == 1 and .[0].root == $root' \
    <<<"$marketplace_json" >/dev/null || {
      printf '%s\n' 'Marketplace registration did not converge to the clone root.' >&2
      exit 1
    }
  run_codex plugin add codex-session-control@codex-session-control-local --json >/dev/null
  ```

  If the one listed root is not the same canonical clone root, fail immediately before marketplace or plugin mutation. Invalid or contaminated JSON is an error, never equivalent to an absent marketplace. The final verification requires exactly one matching marketplace entry.

  The script contains no URL, `curl`, checksum, release metadata, prebuilt selection, service command, old-state inspection, global install, symlink, architecture dispatcher, or clone deletion.

- [ ] **Step 5: Run the focused GREEN/PASS commands, including host compatibility when available**

  Run:

  ```bash
  cargo test --locked --test plugin_packaging_contract
  shellcheck scripts/install-local-plugin.sh
  bash -n scripts/install-local-plugin.sh
  jq empty .agents/plugins/marketplace.json \
    plugins/codex-session-control/.codex-plugin/plugin.json \
    plugins/codex-session-control/.mcp.json
  CODEX_SESSION_CONTROL_PLUGIN_HOST_CONTRACT=1 \
  CODEX_SESSION_CONTROL_CODEX_0_149_1_BIN="$(mise where codex@0.149.1)/bin/codex" \
  CODEX_SESSION_CONTROL_PLUGIN_HOST_AUTH_JSON="${CODEX_HOME:-$HOME/.codex}/auth.json" \
    cargo test --locked --test plugin_packaging_contract \
      legacy_plugin_host_contract_on_codex_0_149_1 -- --ignored --exact --nocapture
  ```

  Expected: nonignored suite PASS in isolated state; on a host with exact Codex 0.149.1, contained execution, exact three-variable forwarding, mode/hash-preserving copy, same-version refresh, version-bump refresh, v1 negative control, removal, clone retention, and generic initialize/list PASS.

- [ ] **Step 6: Commit**

  ```bash
  git add .agents/plugins/marketplace.json plugins/codex-session-control \
    scripts/install-local-plugin.sh tests/plugin_packaging_contract.rs
  git commit -m "feat(plugin): install the checkout-local native server"
  ```

### Task 10: Review Milestone M3 — Replacement Packaging

**Covers:** Task 9
**Review contract:** `## Review Milestones` -> `M3`

- [ ] Dispatch one combined reviewer over Task 9; check exact spec packaging first, then DRY/YAGNI, then shell/Rust quality and failure safety.
- [ ] Require evidence for unsupported-host rejection before Codex mutation, one build, one current-host ELF, regular file/mode, atomic rename, exact manifests/env, canonical-root collision handling, same/version-bump cache refresh, removal retention, v1 negative control, and generic-host launch.
- [ ] Reject download/release/checksum code, a second installer, global wrappers/symlinks, literal endpoint values, `CODEX_HOME` forwarding, extra manifest keys, dispatcher machinery, or fake-host-only claims about actual Codex 0.149.1 behavior.
- [ ] Fix valid findings, rerun packaging evidence, repeat after Critical/Important findings, and continue only when the replacement is independently proven.

### Task 11: Delete Obsolete Lifecycle, Installation, Desktop Attachment, and Dependencies

**Owners:** Deletion worker owns paths; version/dependency worker owns `Cargo.toml` and `Cargo.lock` after receiving the deletion worker's usage inventory.
**Prerequisite:** Tasks 15 and 16 are complete even though their detailed sections appear later in this document. The execution waves are authoritative: the live integration root and its old normal-home/protocol support have already been replaced and the integration target compiles before this deletion begins.
**Files:**
- Delete: `src/install.rs`
- Delete: `src/install/`
- Delete: `src/desktop.rs`
- Delete: `src/desktop/`
- Delete: `src/cli.rs`
- Delete: `src/cli_output.rs`
- Delete: `src/diagnostics.rs`
- Delete: `assets/systemd/codex-session-control.service.in`
- Delete: `assets/marketplace/.agents/plugins/marketplace.json`
- Delete: `assets/marketplace/plugins/codex-session-control/.codex-plugin/plugin.json`
- Delete: `assets/marketplace/plugins/codex-session-control/.mcp.json`
- Delete: `install.sh`
- Delete: `scripts/ci/disposable-systemd-user-contract.sh`
- Delete: `tests/cli_contract.rs`
- Delete: `tests/cli_contract/`
- Delete: `.github/workflows/release.yml`
- Delete: `.github/workflows/publish.yml`
- Modify: `src/model.rs`
- Modify: `src/error.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Capture the retained behavioral baseline and exact deletion inventory**

  This task is a subtractive refactor. Do not add a permanent test that merely asserts files or symbols are absent; that would test source shape and earn no place in the retained suite. Before deletion, run the existing public/runtime contracts that must remain green:

  ```bash
  cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
  cargo test --locked --bin codex-session-control mcp::tests::outcome_unknown::
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::not_loaded_message_
  ```

  Record the exact approved path inventory with `git ls-files` and confirm every deletion target is tracked. Expected: all retained behavioral tests PASS and every approved obsolete path is present before deletion.

- [ ] **Step 2: Delete only approved obsolete surfaces and trim retained models/errors**

  Remove the listed paths. In `src/model.rs`, retain only `Thread`, `Turn`, `ThreadGoal`, `ThreadSnapshot`, their enums, serialization/projection contracts, and tests; delete `ProductConfig`, `DesktopAttachmentIdentity`, `InstalledRelease`, saved-state constants, validators, and lifecycle tests. In `src/error.rs`, retain `ToolErrorCategory`, `NativeErrorSummary`, `ToolErrorData`, `DispatchState`, their constructors/messages/serialization tests, and delete `ControllerError` plus lifecycle conversions after whole-tree compile evidence shows no caller.

  The retained boundary stays on the existing public serialized types rather than introducing a replacement model layer:

  ```rust
  #[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
  #[serde(rename_all = "camelCase", deny_unknown_fields)]
  pub struct ThreadSnapshot {
      pub thread_id: String,
      pub name: Option<String>,
      pub status: ThreadStatus,
      pub active_turn_id: Option<String>,
      pub active_turn_status: Option<TurnStatus>,
      pub updated_at: i64,
  }

  #[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct ToolErrorData {
      pub category: ToolErrorCategory,
      pub message: String,
      pub tool: String,
      pub stage: String,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub thread_id: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub turn_id: Option<String>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub native: Option<NativeErrorSummary>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub dispatch: Option<DispatchState>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub observation: Option<Value>,
      #[serde(skip_serializing_if = "Option::is_none")]
      pub reconciliation_error: Option<String>,
  }
  ```

  Before editing dependencies, run:

  ```bash
  for crate in clap reqwest toml hex sha2 tempfile thiserror uzers predicates; do
    printf '%s: ' "$crate"
    rg -l "${crate//-/_}|$crate" src tests build.rs Cargo.toml | tr '\n' ' '
    printf '\n'
  done
  ```

  Remove `clap` and `reqwest`; remove `toml`, `predicates`, `hex`, `sha2`, `tempfile`, `thiserror`, or `uzers` only when the retained tree has no unique use. Move a crate to `[dev-dependencies]` when only tests use it. Preserve `semver` in normal code for user-agent parsing and in build dependencies for the pin. Refresh `Cargo.lock` once with `cargo check`, then require locked commands.

- [ ] **Step 3: Run retained behavior plus direct deletion and dependency evidence**

  Run:

  ```bash
  cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
  cargo test --locked --bin codex-session-control mcp::tests::outcome_unknown::
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::not_loaded_message_
  cargo test --locked --bin codex-session-control
  for path in \
    src/install.rs src/install src/desktop.rs src/desktop \
    src/cli.rs src/cli_output.rs src/diagnostics.rs \
    assets/systemd/codex-session-control.service.in assets/marketplace \
    install.sh scripts/ci/disposable-systemd-user-contract.sh \
    tests/cli_contract.rs tests/cli_contract \
    .github/workflows/release.yml .github/workflows/publish.yml
  do
    test ! -e "$path"
  done
  if rg -n 'systemctl|app-server-attachment\.json|external-app-server-attachment|load_installed_config|Command::(Setup|Update|Status|Enable|Disable|Uninstall|Codex|McpServer)' src; then
    exit 1
  fi
  cargo tree --locked --edges normal,build,dev
  ```

  Expected: retained behavior PASS; every approved path is absent; negative search returns no current production/lifecycle call; dependency tree contains only uniquely used crates.

- [ ] **Step 4: Commit deletion and dependency pruning separately**

  ```bash
  git add -A -- src/install.rs src/install src/desktop.rs src/desktop \
    src/cli.rs src/cli_output.rs src/diagnostics.rs src/model.rs src/error.rs \
    assets/systemd/codex-session-control.service.in assets/marketplace install.sh \
    scripts/ci/disposable-systemd-user-contract.sh tests/cli_contract.rs \
    tests/cli_contract \
    .github/workflows/release.yml .github/workflows/publish.yml
  git commit -m "refactor(runtime): delete the standalone authority lifecycle"
  git add Cargo.toml Cargo.lock
  git commit -m "chore(deps): prune deleted lifecycle dependencies"
  ```

### Task 12: Native x86-64/AArch64 CI and Canonical Local Checks

**Owner:** CI/check worker
**Files:**
- Modify: `scripts/check.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `tests/workflow_contract.rs`

- [ ] **Step 1: Write RED workflow/check contracts**

  Replace release/systemd/seven-command assertions with tests that require exactly two native matrix entries and the same acceptance steps for each:

  ```rust
  #[test]
  fn native_ci_builds_stages_and_executes_both_supported_architectures() {
      let ci = workflow("ci.yml");
      assert_required(&ci, &[
          "ubuntu-24.04", "ubuntu-24.04-arm", "./scripts/check.sh",
          "cargo build --release --locked",
          "cargo test --locked --test plugin_packaging_contract",
          "plugins/codex-session-control/bin/codex-session-control",
          "cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact",
          "readelf --file-header",
      ], "native CI contract");
      assert_eq!(ci.matches("runner: ubuntu-24.04\n").count(), 1);
      assert_eq!(ci.matches("runner: ubuntu-24.04-arm\n").count(), 1);
      assert_eq!(job_ids(&ci), ["native-contract"]);
      assert!(!ci.contains("systemd-integration"));
      assert!(!ci.contains("cross"));
  }
  ```

  Update the check-wrapper contract to require only retained scripts, root manifests, and `ci.yml`.

- [ ] **Step 2: Run RED**

  Run:

  ```bash
  cargo test --locked --test workflow_contract native_ci -- --nocapture
  cargo test --locked --test workflow_contract standard_checks -- --nocapture
  ```

  Expected: FAIL because CI still has systemd/release-era jobs and ARM duplicates a partial gate.

- [ ] **Step 3: Refocus `scripts/check.sh` and CI**

  `scripts/check.sh` must retain secure temporary-directory setup, fmt, clippy, full locked tests, exact actionlint 1.7.12, and validate only:

  ```bash
  shellcheck scripts/check.sh scripts/set-supported-codex-version.sh scripts/install-local-plugin.sh
  bash -n scripts/check.sh scripts/set-supported-codex-version.sh scripts/install-local-plugin.sh
  actionlint .github/workflows/ci.yml
  jq empty .agents/plugins/marketplace.json \
    plugins/codex-session-control/.codex-plugin/plugin.json \
    plugins/codex-session-control/.mcp.json
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  cargo test --workspace --all-features --locked
  ```

  Use one native CI job with an explicit matrix containing `{runner: ubuntu-24.04, machine: X86-64}` and `{runner: ubuntu-24.04-arm, machine: AArch64}`. Each matrix run installs identical prerequisites, installs verified actionlint, runs `./scripts/check.sh`, builds release locked, runs focused installer staging in isolated state, inspects the staged regular mode-0755 ELF with `readelf`, executes the staged binary over stdio, and runs exact catalog coverage. No systemd job or publishing workflow remains.

  The matrix and acceptance commands have one shared definition:

  ```yaml
  jobs:
    native-contract:
      strategy:
        matrix:
          include:
            - runner: ubuntu-24.04
              machine: X86-64
            - runner: ubuntu-24.04-arm
              machine: AArch64
      runs-on: ${{ matrix.runner }}
      steps:
        - name: Run the full repository gate
          run: ./scripts/check.sh
        - name: Build and stage the native server
          run: |
            cargo build --release --locked
            cargo test --locked --test plugin_packaging_contract installer_builds_locked_and_atomically_stages_native_executable -- --exact
            test -f plugins/codex-session-control/bin/codex-session-control
            test -x plugins/codex-session-control/bin/codex-session-control
            test ! -L plugins/codex-session-control/bin/codex-session-control
            readelf --file-header plugins/codex-session-control/bin/codex-session-control | grep -F "${{ matrix.machine }}"
            cargo test --locked --test plugin_packaging_contract generic_client_initializes_and_lists_exact_catalog_from_another_cwd -- --exact
            cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
  ```

- [ ] **Step 4: Run the focused GREEN/PASS commands**

  Run:

  ```bash
  cargo test --locked --test workflow_contract native_ci -- --nocapture
  cargo test --locked --test workflow_contract standard_checks -- --nocapture
  shellcheck scripts/check.sh scripts/set-supported-codex-version.sh scripts/install-local-plugin.sh
  bash -n scripts/check.sh scripts/set-supported-codex-version.sh scripts/install-local-plugin.sh
  actionlint .github/workflows/ci.yml
  ```

  Then run `./scripts/check.sh`. Expected: focused contracts, static checks, and the full wrapper PASS on a coherent tree; Task 15 has already replaced the old integration root before Task 11 deleted lifecycle assets. Hosted CI later supplies fresh native x86-64 and AArch64 execution evidence.

- [ ] **Step 5: Commit**

  ```bash
  git add scripts/check.sh .github/workflows/ci.yml tests/workflow_contract.rs
  git commit -m "ci: verify native plugin binaries on both architectures"
  ```

### Task 13: Current Documentation, Upgrade Path, and Bug Evidence

**Owner:** Documentation worker
**Files:**
- Create: `docs/upgrading.md`
- Modify: `.github/ISSUE_TEMPLATE/bug.yml`
- Modify: `README.md`
- Modify: `CONTRIBUTING.md`
- Modify: `SECURITY.md`
- Modify: `docs/architecture.md`
- Modify: `docs/desktop.md`
- Modify: `docs/security.md`
- Modify: `docs/troubleshooting.md`
- Modify: `tests/workflow_contract.rs`

- [ ] **Step 1: Write RED reader-surface contracts**

  Add a test that scans current reader surfaces and permits historical cleanup commands only inside the explicitly labeled old-installation section of `docs/upgrading.md`:

  ```rust
  const CURRENT_READER_SURFACES: &[&str] = &[
      "README.md", "CONTRIBUTING.md", "SECURITY.md",
      "docs/architecture.md", "docs/desktop.md", "docs/security.md",
      "docs/troubleshooting.md", ".github/ISSUE_TEMPLATE/bug.yml",
  ];
  const REMOVED_COMMANDS: &[&str] = &[
      "codex-session-control setup", "codex-session-control status",
      "codex-session-control enable", "codex-session-control disable",
      "codex-session-control update", "codex-session-control uninstall",
      "codex-session-control codex", "codex-session-control mcp-server",
  ];
  for path in CURRENT_READER_SURFACES {
      let text = fs::read_to_string(root.join(path)).unwrap();
      for removed in REMOVED_COMMANDS {
          assert!(!text.contains(removed), "{path} advertises removed command {removed}");
      }
  }
  let upgrading = fs::read_to_string(root.join("docs/upgrading.md")).unwrap();
  for required in [
      "codex plugin remove codex-session-control@codex-session-control-local",
      "codex plugin marketplace remove codex-session-control-local",
      "systemctl --user disable --now codex-session-control.service",
      "./scripts/install-local-plugin.sh",
      "thirteen-tool catalog",
  ] {
      assert!(upgrading.contains(required), "upgrade guide omitted {required}");
  }
  ```

  Assert the bug form requests host surface, concrete plugin version/visibility, Desktop shared-socket availability, exact stderr/MCP error, and reproduction; reject `--version` and `status` command instructions.

- [ ] **Step 2: Run RED**

  Run:

  ```bash
  cargo test --locked --test workflow_contract reader_facing -- --nocapture
  cargo test --locked --test workflow_contract bug_form -- --nocapture
  cargo test --locked --test workflow_contract upgrading -- --nocapture
  ```

  Expected: FAIL across README, architecture, Desktop, troubleshooting, contribution guidance, security docs, and bug form.

- [ ] **Step 3: Complete the human lifecycle evidence**

  First prove and remove the actual old 0.3.x registration before the new checkout installer can run. The current marketplace name targets the old standalone installation root, so treating cleanup as a later documentation action would make the new installer correctly fail its collision gate.

  **[MANUAL]** Capture redacted old-state evidence from `MISE_QUIET=1 codex plugin list --json`, `MISE_QUIET=1 codex plugin marketplace list --json`, and the old user service status. Confirm the installed plugin/marketplace are the old 0.3.x state, then execute in this order:

  ```bash
  codex plugin remove codex-session-control@codex-session-control-local
  codex plugin marketplace remove codex-session-control-local
  systemctl --user disable --now codex-session-control.service
  ```

  Verify the old plugin and marketplace are absent and the old service is disabled/inactive before running the new installer. If the observed old state or accepted commands differ, stop and repair the upgrade instructions from that evidence; do not weaken the installer collision check and do not add migration code.

  Prepare the disposable bumped clone before the UI action:

  ```bash
  bumped_clone="$(mktemp -d /tmp/csc-version-bump.XXXXXX)"
  git clone --no-hardlinks "$PWD" "$bumped_clone"
  ```

  In that disposable clone only, use `apply_patch` to change package version `0.3.2` to `0.3.3` in its `Cargo.toml`, root-package entry in `Cargo.lock`, and `plugins/codex-session-control/.codex-plugin/plugin.json`; `git diff --check` must show only those three version fields. The bumped installer must still run `cargo build --release --locked`.

  **[MANUAL]** With Desktop using `shared-app-server-socket`, run `./scripts/install-local-plugin.sh`, start a new CLI session and a new Desktop task, and record exact 13-tool presence. In CLI `/plugins` and Desktop Plugins UI, disable then start a new session/task and record absence; re-enable then start a new session/task and record presence. Re-run the unchanged installer and verify same-version cache refresh in new sessions/tasks. Run the Task 9 isolated version-bump host contract; for the UI check, remove the real plugin and marketplace with `codex plugin remove codex-session-control@codex-session-control-local` and `codex plugin marketplace remove codex-session-control-local`, run the bumped test clone's installer, and verify refreshed tools in a new CLI session and Desktop task. Remove the bumped plugin/marketplace with the same native commands and run the real clone's installer before continuing. Finally remove the real plugin once more, verify absence in a new CLI session and Desktop task without deleting the clone/staged binary, then reinstall for the remaining gates. Record only redacted outcomes, manifest/plugin versions, cache hashes/modes, and new-session/task boundaries in `/tmp/csc-phase1-manual-lifecycle.md`; never record task content or environment values.

  **[MANUAL]** Inspect the Desktop-launched CSC process environment and record only that the forwarded variable-name set equals `XDG_RUNTIME_DIR`, `CODEX_LINUX_APP_ID`, and `CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET`. Verify native registration JSON with `MISE_QUIET=1 codex plugin marketplace list --json` and `MISE_QUIET=1 codex plugin list --json`.

  After reinstalling the real clone, delete only the validated disposable clone:

  ```bash
  test -n "$bumped_clone"
  test "$(dirname -- "$bumped_clone")" = /tmp
  case "$(basename -- "$bumped_clone")" in
    csc-version-bump.*) ;;
    *) exit 1 ;;
  esac
  test -d "$bumped_clone"
  test ! -L "$bumped_clone"
  rm -rf -- "$bumped_clone"
  ```

  Expected: install/disable/re-enable/same-version/version-bump/removal evidence is complete for both host surfaces; only new sessions/tasks change visibility; clone and staged binary remain after native removal; exact three-variable names are proven; old cleanup commands are observed rather than guessed.

- [ ] **Step 4: Rewrite the current product surface**

  Produce clean final documentation with these exact responsibilities:

  - `README.md`: Desktop-open dependency, checkout installer, legacy local plugin, exact 13 tools, normal CLI/Desktop/generic-client usage, stable checkout path, update/restage/new-session behavior, native removal, no management CLI.
  - `docs/architecture.md`: stateless stdio process, fresh endpoint resolution/validation per connection, `/rpc`, retained core, restart/no-replay semantics.
  - `docs/desktop.md`: upstream `shared-app-server-socket`, exact environment contract, Plugins UI lifecycle, new-task boundary; no personal fork or attachment descriptor.
  - `docs/security.md`: same-user endpoint predicates, path redaction, no authority ownership, no TCP, no scanning, no legacy state.
  - `docs/troubleshooting.md`: Desktop/socket/plugin visibility/stderr/MCP errors, restaging and new session/task; no status/service commands.
  - `CONTRIBUTING.md`: exact contract preservation, TDD, native architecture gates, disposable live-task ledger, no service/release assumptions.
  - `SECURITY.md`: supported source state without inventing a release number; private reporting; redact socket paths, environment values, credentials, and task content.
  - `.github/ISSUE_TEMPLATE/bug.yml`: only evidence available from the plugin-contained product.

  The bug form replaces command-derived version/status fields with concrete current evidence:

  ```yaml
  - type: dropdown
    id: host_surface
    attributes:
      label: Host surface
      options:
        - Codex Desktop
        - Codex CLI
        - Generic stdio MCP host
    validations:
      required: true
  - type: input
    id: plugin
    attributes:
      label: Plugin version and visibility
      description: Report the manifest version and whether the plugin is enabled and visible in this host.
    validations:
      required: true
  - type: textarea
    id: desktop_socket
    attributes:
      label: Desktop shared-socket availability
      description: Report whether the private shared socket exists; do not paste its full path or environment values.
    validations:
      required: true
  - type: textarea
    id: error
    attributes:
      label: Exact stderr or MCP error
      description: Paste the exact error after redacting paths, credentials, environment values, and task content.
    validations:
      required: true
  - type: textarea
    id: reproduction
    attributes:
      label: Reproduction steps
    validations:
      required: true
  ```

  `docs/upgrading.md` must contain the verified five-step cutover in order and label the old commands as historical 0.3.x cleanup:

  ```markdown
  1. **[MANUAL]** Disable or remove the old plugin, then stop and disable the old service:
     `codex plugin remove codex-session-control@codex-session-control-local`
     `codex plugin marketplace remove codex-session-control-local`
     `systemctl --user disable --now codex-session-control.service`
  2. **[MANUAL]** Start upstream Desktop with `shared-app-server-socket` enabled and verify its private socket exists.
  3. From the new checkout, run `./scripts/install-local-plugin.sh` and verify native marketplace/plugin registration.
  4. **[MANUAL]** Relaunch Desktop and normal Codex CLI tasks so they load the staged MCP executable.
  5. Verify the exact thirteen-tool catalog and confirm no old CSC service or authority process remains.
  ```

  Publish those exact old cleanup commands only after the prerequisite manual proof. Do not add cleanup logic to the installer.

- [ ] **Step 5: Run the focused GREEN/PASS commands and a direct negative search**

  Run:

  ```bash
  cargo test --locked --test workflow_contract reader_facing -- --nocapture
  cargo test --locked --test workflow_contract bug_form -- --nocapture
  cargo test --locked --test workflow_contract upgrading -- --nocapture
  rg -n \
    'codex-session-control (setup|status|enable|disable|update|uninstall|codex|mcp-server)|external-app-server-attachment|app-server-attachment\.json|codex-session-control\.service' \
    README.md CONTRIBUTING.md SECURITY.md docs/architecture.md docs/desktop.md \
    docs/security.md docs/troubleshooting.md docs/upgrading.md .github/ISSUE_TEMPLATE
  git diff --check -- README.md CONTRIBUTING.md SECURITY.md docs .github/ISSUE_TEMPLATE/bug.yml
  ```

  Expected: contract tests PASS; the search returns only the explicitly labeled historical service cleanup in `docs/upgrading.md`, with no current workflow instruction.

- [ ] **Step 6: Commit**

  ```bash
  git add README.md CONTRIBUTING.md SECURITY.md docs/architecture.md docs/desktop.md \
    docs/security.md docs/troubleshooting.md docs/upgrading.md \
    .github/ISSUE_TEMPLATE/bug.yml tests/workflow_contract.rs
  git commit -m "docs: document the Desktop-owned plugin workflow"
  ```

### Task 14: Review Milestone M5 — Subtractive Product and Reader Surface

**Covers:** Tasks 11-13
**Review contract:** `## Review Milestones` -> `M5`

- [ ] Dispatch one combined reviewer over the incremental surface; check deletion/spec compliance first, then DRY/YAGNI, then code/documentation quality.
- [ ] Require an exact removed-path inventory, retained-model/error inventory, dependency use proof, current `scripts/check.sh`, two native CI paths, upgrade evidence, bug evidence, and reader-surface negative search.
- [ ] Reject compatibility shims, old-state readers, installer cleanup, release/download remnants, command aliases, global executables, duplicate manifests, unsupported-version claims, guessed cleanup commands, or documentation that promises hot-loading/process termination.
- [ ] Fix valid findings, rerun affected gates, repeat after Critical/Important findings, and continue only with no material finding.

### Task 15: Failure-Safe Ignored Live All-Tool Ledger

**Owner:** Live-validation worker
**Files:**
- Modify: `tests/app_server_integration.rs`
- Modify: `tests/app_server_integration/cases.rs`
- Modify: `tests/app_server_integration/live_harness.rs`
- Delete: `tests/app_server_integration/normal_home.rs`
- Delete: `tests/app_server_integration/normal_home_paths.rs`
- Delete: `tests/app_server_integration/protocol_support.rs`

- [ ] **Step 1: Write RED ledger and opt-in contracts before live mutation code**

  Add nonignored tests named:

  ```rust
  ledger_persists_each_owned_id_with_file_and_directory_fsync
  ledger_persists_workspace_before_first_creation
  live_gate_requires_exact_opt_in_before_mutation
  recovery_requires_exact_opt_in_and_absolute_ledger
  cleanup_retains_ledger_until_archive_proof
  already_archived_exact_ledger_target_skips_archive_and_converges
  active_exact_ledger_target_archives_once_then_converges
  invalid_exact_read_evidence_fails_closed_and_retains_ledger
  ```

  Add exactly one ignored test:

  ```rust
  #[tokio::test]
  #[ignore = "requires explicit disposable-task opt-in and a live Desktop authority"]
  async fn live_desktop_authority_all_thirteen_tools_are_disposable()
      -> Result<(), Box<dyn Error>>
  {
      cases::live_desktop_authority_all_thirteen_tools_are_disposable().await
  }
  ```

  Do not add a compile-fail/source-inspection test for `OwnedThreadId`. Its safety is the typed API itself: every mutating helper accepts `&OwnedThreadId`, construction remains private to durable ledger recording/recovery, and Tasks 16 and 19 inspect that invariant directly. A permanent test layer that merely proves arbitrary strings do not compile would not earn its place.

- [ ] **Step 2: Run RED without mutating live state**

  Run:

  ```bash
  cargo test --locked --test app_server_integration ledger_ -- --nocapture
  cargo test --locked --test app_server_integration live_gate_requires_exact_opt_in_before_mutation -- --exact
  cargo test --locked --test app_server_integration already_archived_exact_ledger_target_skips_archive_and_converges -- --exact
  cargo test --locked --test app_server_integration active_exact_ledger_target_archives_once_then_converges -- --exact
  cargo test --locked --test app_server_integration invalid_exact_read_evidence_fails_closed_and_retains_ledger -- --exact
  cargo test --locked --test app_server_integration \
    live_desktop_authority_all_thirteen_tools_are_disposable -- --ignored --exact --nocapture
  ```

  Expected: focused reconciliation contracts fail pre-fix (`already_archived...` expects zero archive calls, `active...` expects one archive then exact-read convergence, `invalid...` expects zero archive plus retained ledger/run dir and no deletion), ledger contracts fail because ledger types are absent, and ignored live test exits before mutation because `CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS` is not exactly `1`.

- [ ] **Step 3: Implement an ownership type and crash-durable ledger**

  Keep ownership construction private to successful create/fork responses already tied to the unique workspace:

  ```rust
  #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
  #[serde(transparent)]
  struct OwnedThreadId(String);

  #[derive(Debug, Deserialize, Serialize)]
  #[serde(rename_all = "camelCase", deny_unknown_fields)]
  struct LedgerDocument {
      workspace: PathBuf,
      owned_thread_ids: Vec<OwnedThreadId>,
  }

  struct OwnedThreadLedger {
      run_dir: PathBuf,
      path: PathBuf,
      document: LedgerDocument,
  }

  impl OwnedThreadLedger {
      fn create(run_dir: PathBuf, workspace: PathBuf) -> io::Result<Self> {
          let path = run_dir.join("owned-thread-ids.json");
          let ledger = Self {
              run_dir,
              path,
              document: LedgerDocument {
                  workspace,
                  owned_thread_ids: Vec::new(),
              },
          };
          ledger.persist()?;
          Ok(ledger)
      }

      fn record(&mut self, id: String) -> io::Result<OwnedThreadId> {
          let owned = OwnedThreadId(id);
          self.document.owned_thread_ids.push(owned.clone());
          self.persist()?;
          Ok(owned)
      }

      fn persist(&self) -> io::Result<()> {
          let bytes = serde_json::to_vec(&self.document).map_err(io::Error::other)?;
          atomic_write_fsync(&self.path, bytes)
      }
  }

  fn atomic_write_fsync(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
      let parent = path.parent().ok_or_else(|| io::Error::other("ledger has no parent"))?;
      let mut staged = tempfile::NamedTempFile::new_in(parent)?;
      staged.as_file_mut().set_permissions(fs::Permissions::from_mode(0o600))?;
      staged.write_all(&bytes)?;
      staged.as_file_mut().sync_all()?;
      staged.persist(path).map_err(|error| error.error)?;
      File::open(parent)?.sync_all()
  }
  ```

  Create `/tmp/codex-session-control-live-*` mode `0700`; create the new random workspace, prove it is absent from active and archived lists, and durably write its exact path plus an empty ID list to `owned-thread-ids.json` before the first create. Every later write uses a same-directory temporary regular file, file `fsync`, atomic rename, and parent-directory `fsync`. Every mutating helper accepts `&OwnedThreadId`, never `&str`. Never infer ownership from title, timestamp, `/tmp` scan, or global before/after diff.

- [ ] **Step 4: Drive the built CSC binary and implement unconditional cleanup**

  Before mutation require exact opt-in, a production-valid endpoint, supported initialized version, and a new random workspace absent from both active and archived lists. Spawn `target/debug/codex-session-control` over stdio MCP, assert exact catalog, then exercise every tool on ledger-owned tasks: create, fork, list, read, wait, message, title, goal get/set/pause/resume/clear, and interrupt.

  Structure cleanup outside the operation result so both errors and panics reach the same cleanup boundary:

  ```rust
  let run_result = AssertUnwindSafe(run_all_thirteen_tools(&mut harness, &mut ledger))
      .catch_unwind()
      .await;
  let cleanup_result = async {
      harness.stop_and_reap_mcp_child().await?;
      ledger.recover_exact_workspace_ids(&mut harness).await?;
      harness.archive_and_verify(&ledger.document.owned_thread_ids).await
  }.await;
  match cleanup_result {
      Ok(()) => ledger.remove_proven_clean_run_dir()?,
      Err(error) => return Err(ledger.cleanup_failure(error).into()),
  }
  match run_result {
      Ok(result) => result,
      Err(payload) => std::panic::resume_unwind(payload),
  }
  ```

  `recover_exact_workspace_ids` reads the exact workspace path durably stored in the ledger before mutation, queries only that workspace, records any create/fork result lost between native acceptance and response handling through the same atomic ledger path, and never searches titles, timestamps, other workspaces, or global diffs. `archive_and_verify` opens a fresh `/rpc` native connection, exact-reads each ledger ID, validates the returned ID, and classifies storage by initialized `codexHome` subtree (`sessions` or `archived_sessions`) only. It skips already-archived IDs, archives only IDs proven active, and then polls exact-read until each target is classified archived. If an exact read is missing, reports a mismatched ID, or yields unclassifiable storage, cleanup fails closed and reports the exact ledger path with remaining IDs. `thread/list` remains an ownership-recovery helper only; it cannot prove archive state for preview-empty threads.

  Hard-kill recovery is a separate entry branch and requires both:

  ```rust
  let opted_in = std::env::var_os("CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS")
      .as_deref() == Some(OsStr::new("1"));
  let recovery_ledger = std::env::var_os("CODEX_SESSION_CONTROL_LIVE_RECOVER_LEDGER")
      .map(PathBuf::from)
      .filter(|path| path.is_absolute())
      .filter(|path| path.file_name() == Some(OsStr::new("owned-thread-ids.json")));
  if !opted_in || recovery_ledger.is_none() {
      return Err("hard-kill recovery requires exact opt-in and an absolute ledger path".into());
  }
  ```

  Move only the current direct native seam into `live_harness.rs` before deleting its old module:

  ```rust
  type NativeWebSocket = WebSocketStream<UnixStream>;

  pub(super) struct NativeConnection {
      websocket: NativeWebSocket,
      next_id: u64,
      codex_home: Option<PathBuf>,
  }

  impl NativeConnection {
      async fn connect(socket: &Path) -> Result<Self, Box<dyn Error>> {
          let stream = UnixStream::connect(socket).await?;
          let (websocket, _) = client_async("ws://localhost/rpc", stream).await?;
          Ok(Self {
              websocket,
              next_id: 1,
              codex_home: None,
          })
      }

      pub(super) async fn initialize(&mut self) -> Result<(), Box<dyn Error>> {
          let initialized = self
              .request(
                  "initialize",
                  json!({
                      "clientInfo": {
                          "name": "codex_session_control_live_test",
                          "title": "Codex Session Control Live Test",
                          "version": env!("CARGO_PKG_VERSION"),
                      },
                      "capabilities": {
                          "experimentalApi": true,
                          "mcpServerOpenaiFormElicitation": false,
                          "requestAttestation": false,
                          "optOutNotificationMethods": [],
                      },
                  }),
              )
              .await?;
          let codex_home = initialized
              .get("codexHome")
              .and_then(Value::as_str)
              .filter(|home| Path::new(home).is_absolute())
              .map(PathBuf::from)
              .ok_or_else(|| io::Error::other("initialize omitted an absolute Codex home"))?;
          let user_agent = initialized
              .get("userAgent")
              .and_then(Value::as_str)
              .ok_or_else(|| io::Error::other("initialize omitted a user agent"))?;
          if !has_supported_codex_version(user_agent) {
              return Err(
                  io::Error::other("Desktop authority is not on the supported version").into(),
              );
          }
          self.codex_home = Some(codex_home);
          self.websocket
              .send(Message::text(json!({"method": "initialized"}).to_string()))
              .await?;
          Ok(())
      }

      pub(super) fn initialized_codex_home(&self) -> Option<&Path> {
          self.codex_home.as_deref()
      }

      pub(super) async fn request(
          &mut self,
          method: &str,
          params: impl serde::Serialize,
      ) -> Result<Value, Box<dyn Error>> {
          let id = self.next_id;
          self.next_id += 1;
          self.websocket
              .send(Message::text(
                  json!({"id": id, "method": method, "params": params}).to_string(),
              ))
              .await?;
          tokio::time::timeout(Duration::from_secs(10), async {
              loop {
                  let frame = self
                      .websocket
                      .next()
                      .await
                      .ok_or_else(|| io::Error::other("app-server disconnected"))?
                      .map_err(io::Error::other)?;
                  let Message::Text(text) = frame else {
                      continue;
                  };
                  let value: Value = serde_json::from_str(text.as_str())?;
                  if value.get("id").and_then(Value::as_u64) != Some(id) {
                      continue;
                  }
                  if let Some(error) = value.get("error") {
                      return Err(io::Error::other(format!("{method} failed: {error}")));
                  }
                  return value
                      .get("result")
                      .cloned()
                      .ok_or_else(|| io::Error::other(format!("{method} omitted result")));
              }
          })
          .await
          .map_err(|_| io::Error::other(format!("{method} timed out")))?
          .map_err(Into::into)
      }
  }
  ```

  `initialized_codex_home()` is the only storage-root accessor. Exact storage classification is in `exact_thread_storage` in `tests/app_server_integration/cases.rs`: it validates returned `thread.id` equality, strips and classifies by the first normalized path component under initialized `codexHome` as `sessions` or `archived_sessions`, rejects any non-normal remainder and nested storage markers, and requires at least one rollout-relative component before classification.

  This is the current 10-second correlated-result loop moved intact: it ignores notifications and unrelated IDs, returns the exact matching result, surfaces the matching native error, and fails on EOF/timeout. Remove `mod protocol_support;` and delete `tests/app_server_integration/protocol_support.rs`; do not retain its fake `ResponsesEndpoint`, TCP provider, fake authority, service/process ownership, normal-home, remote CLI, or projection helpers.

- [ ] **Step 5: Run nonmutating focused GREEN/PASS commands and compile the ignored gate**

  Run:

  ```bash
  cargo test --locked --test app_server_integration ledger_ -- --nocapture
  cargo test --locked --test app_server_integration already_archived_exact_ledger_target_skips_archive_and_converges -- --exact
  cargo test --locked --test app_server_integration active_exact_ledger_target_archives_once_then_converges -- --exact
  cargo test --locked --test app_server_integration invalid_exact_read_evidence_fails_closed_and_retains_ledger -- --exact
  cargo test --locked --test app_server_integration live_gate_requires_exact_opt_in_before_mutation -- --exact
  cargo test --locked --test app_server_integration recovery_requires_exact_opt_in_and_absolute_ledger -- --exact
  cargo test --locked --test app_server_integration --no-run
  ```

  Expected: nonignored safety tests PASS with exact behaviors: already-archived path does not archive; active path archives once then converges via exact read; invalid read path dispatches zero archive, retains ledger and run dir, and does not authorize deletion; ignored live gate compiles and no live task is created.

- [ ] **Step 6: Run the authorized live gate**

  Run only after the Desktop/manual prerequisite and exact empty-workspace preflight:

  ```bash
  CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS=1 \
  cargo test --locked --test app_server_integration \
    live_desktop_authority_all_thirteen_tools_are_disposable \
    -- --ignored --exact --nocapture --test-threads=1
  ```

  Expected: PASS; exact 13 tools exercised; every created/forked ID durably recorded before later mutation; MCP child reaped; ledger IDs archived; run directory deleted only after proof. On failure, preserve the printed ledger path and remaining IDs. Recovery uses the same command plus `CODEX_SESSION_CONTROL_LIVE_RECOVER_LEDGER=/absolute/path/owned-thread-ids.json`; it never scans.

- [ ] **Step 7: Commit**

  ```bash
  git add -A -- tests/app_server_integration.rs tests/app_server_integration/cases.rs \
    tests/app_server_integration/live_harness.rs \
    tests/app_server_integration/normal_home.rs \
    tests/app_server_integration/normal_home_paths.rs \
    tests/app_server_integration/protocol_support.rs
  git commit -m "test(live): ledger all disposable tool targets"
  ```

### Task 16: Review Milestone M4 — Live Safety

**Covers:** Task 15
**Review contract:** `## Review Milestones` -> `M4`

- [ ] Dispatch one combined reviewer over Task 15; check exact live-safety specification first, then DRY/YAGNI, then code quality.
- [ ] Require a path-by-path proof for opt-in before mutation, empty unique workspace, atomic/fsynced record-before-use, `OwnedThreadId` compile-time boundary, exact 13-tool exercise, child stop/reap before cleanup, fresh native exact-ID active/archived reconciliation with idempotence, retained ledger on failure, and two-variable hard-kill recovery.
- [ ] Reject any global scan, title/timestamp ownership, untyped mutating target, pre-existing task mutation, cleanup-by-diff, automatic ledger deletion on failed cleanup, or second ignored live end-to-end gate.
- [ ] Fix valid findings, rerun all nonmutating safety tests and the live gate when its behavior changed, repeat after Critical/Important findings, and continue only with no material finding.

### Task 17: Fresh Whole-Tree Verification

**Owner:** Primary orchestrator; workers may run bounded commands and write raw output to `/tmp/csc-phase1-verification/`, but the orchestrator alone judges integration.
**Files:** No production edits; fixes return to the owning task and receive a path-scoped commit.

- [ ] Run `git fetch origin main`, then re-run the exact branch/base/clean gate. Clean means no unrelated dirt; tracked implementation changes must already be committed. If the fetched merge-base differs from `88f2ac5b124abaa2a355ca88304e9439c692eb0a`, stop for target inventory and source re-review.
- [ ] Run focused gates in the specification's order:

  ```bash
  cargo test --locked --bin codex-session-control app_server::endpoint::tests::
  cargo test --locked --bin codex-session-control app_server::tests::transport::websocket_upgrade_uses_exact_rpc_path -- --exact
  cargo test --locked --bin codex-session-control app_server::tests::transport::
  cargo test --locked --bin codex-session-control mcp::tests::mutation_mapping::not_loaded_message_
  cargo test --locked --bin codex-session-control mcp::tests::outcome_unknown::
  cargo test --locked --test workflow_contract tested_codex_version -- --nocapture
  cargo test --locked --test mcp_contract public_catalog_is_exact -- --exact
  cargo test --locked --test desktop_shared_socket_contract
  cargo test --locked --test plugin_packaging_contract
  ```

  Expected: every command PASS.
- [ ] Run deletion and reader-surface evidence:

  ```bash
  for path in \
    src/install.rs src/install src/desktop.rs src/desktop \
    src/cli.rs src/cli_output.rs src/diagnostics.rs \
    assets/systemd/codex-session-control.service.in assets/marketplace \
    install.sh scripts/ci/disposable-systemd-user-contract.sh \
    tests/cli_contract.rs tests/cli_contract \
    tests/app_server_integration/normal_home.rs \
    tests/app_server_integration/normal_home_paths.rs \
    tests/app_server_integration/protocol_support.rs \
    .github/workflows/release.yml .github/workflows/publish.yml
  do
    test ! -e "$path"
  done
  rg -n \
    'codex-session-control (setup|status|enable|disable|update|uninstall|codex|mcp-server)|external-app-server-attachment|app-server-attachment\.json|codex-session-control\.service' \
    README.md CONTRIBUTING.md SECURITY.md docs/architecture.md docs/desktop.md \
    docs/security.md docs/troubleshooting.md docs/upgrading.md .github/ISSUE_TEMPLATE
  ```

  Expected: absence loop exits `0`; search returns only explicitly historical cleanup text in `docs/upgrading.md`.
- [ ] Run canonical full checks last:

  ```bash
  git diff --check origin/main...HEAD
  ./scripts/check.sh
  ```

  Expected: both exit `0`, including format, shell, manifest, actionlint, clippy `-D warnings`, and all locked tests.
- [ ] Push the committed candidate normally to the internal feature branch and dispatch the native workflow before final review:

  ```bash
  git push --set-upstream origin feat/desktop-owned-session-control
  candidate_sha="$(git rev-parse HEAD)"
  gh workflow run ci.yml --repo agentlehub/codex-session-control \
    --ref feat/desktop-owned-session-control
  run_id=
  for attempt in 1 2 3 4 5 6 7 8 9 10; do
    run_id="$(gh run list --repo agentlehub/codex-session-control \
      --workflow ci.yml --branch feat/desktop-owned-session-control \
      --event workflow_dispatch --limit 1 --json databaseId,headSha \
      --jq ".[] | select(.headSha == \"$candidate_sha\") | .databaseId")"
    test -z "$run_id" || break
    sleep 2
  done
  test -n "$run_id"
  gh run watch "$run_id" --repo agentlehub/codex-session-control --exit-status
  gh run view "$run_id" --repo agentlehub/codex-session-control \
    --json url,headSha,jobs
  ```

  Expected: normal push and workflow dispatch succeed; the run's `headSha` equals the candidate SHA; native x86-64 and AArch64 jobs are green. Record the run URL, job evidence, and exact SHA for the pull request Verification section; do not accept cross-compile evidence.
- [ ] If any command or CI job fails, return to its owning task, diagnose root cause before editing, add/retain the narrow regression, commit the targeted fix, and restart Task 17 from the top. Do not enter final review with stale evidence.

### Task 18: Final Whole-Implementation Review Pass 1 — Specification Compliance

**Covers:** All implementation tasks from `88f2ac5b124abaa2a355ca88304e9439c692eb0a` through current `HEAD`
**Review contract:** `## Review Milestones` -> `Final`

- [ ] Dispatch one dedicated specification-compliance reviewer. Give it the approved spec, linked spec review trace, this plan, full diff, commit list, focused/full verification output, hosted native CI evidence, manual lifecycle evidence, and live ledger evidence.
- [ ] Require a numbered verdict against all 32 acceptance criteria and explicit inspection of rejected architectures: no CSC authority/lifecycle, no fallback/scanning/TCP, no contract drift, no mutation replay, no v1 manifest, no standalone distribution, no attached-CLI code, and no external publication.
- [ ] A missing requirement, scope addition, contradiction, unproven manual gate, or Critical/Important finding blocks the pass. Fix valid findings, rerun Task 17, and repeat this review.
- [ ] Save the final concise report at `/tmp/csc-phase1-spec-compliance.md` for transfer into the internal PR evidence. Mark this task complete only on PASS.

### Task 19: Final Whole-Implementation Review Pass 2 — Dedicated DRY/YAGNI

**Covers:** The same whole implementation, only after Task 18 passes
**Review contract:** `## Review Milestones` -> `Final`

- [ ] Dispatch a different reviewer with one required inventory table containing every added production module, production helper, dependency/build-dependency/dev-dependency, installer branch, compatibility path, test module/helper/layer, manifest source, and CI branch.
- [ ] For each row require: exact path/symbol, one concrete retained behavior or plausible regression it uniquely owns, existing smaller alternative considered, and `keep` or `delete` disposition.
- [ ] Run evidence for the reviewer:

  ```bash
  git diff --name-status 88f2ac5b124abaa2a355ca88304e9439c692eb0a..HEAD
  git diff --stat 88f2ac5b124abaa2a355ca88304e9439c692eb0a..HEAD
  cargo tree --locked --edges normal,build,dev
  rg -n 'enum |struct |trait |fn |\[dependencies\]|\[build-dependencies\]|\[dev-dependencies\]' \
    src Cargo.toml scripts/install-local-plugin.sh tests
  ```

- [ ] Delete redundant abstraction, speculative extension point, duplicate manifest/config authority, unused fallback, equivalent test permutation, release-era compatibility, and test layer justified only by count/coverage. Test count or changed LOC is never a keep reason.
- [ ] Any accepted deletion or code change resets the final chain: commit path-scoped, rerun Task 17, rerun Task 18, then repeat Task 19. Save the final PASS report at `/tmp/csc-phase1-dry-yagni.md`.

### Task 20: Final Whole-Implementation Review Pass 3 — Code Quality

**Covers:** The same whole implementation, only after Tasks 18 and 19 pass
**Review contract:** `## Review Milestones` -> `Final`

- [ ] Dispatch a third reviewer for correctness, readability, error attribution/redaction, race safety, Unix metadata semantics, async/process cleanup, shell quoting/atomicity, test stability, CI determinism, and polished documentation. It must not reopen approved product decisions without contradictory evidence.
- [ ] Fix every valid Critical/Important finding and valid Minor finding. A code or documentation change resets the chain to Task 17 so specification and DRY/YAGNI pass again before code quality is repeated.
- [ ] Save the final PASS report at `/tmp/csc-phase1-code-quality.md`.
- [ ] Mark the final milestone complete only when the final tree has fresh evidence in exact order: `./scripts/check.sh` PASS, Task 18 PASS, Task 19 PASS, Task 20 PASS.

### Task 21: Push the Branch and Open the Internal CSC Pull Request

**Owner:** Primary orchestrator
**Files:** `.github/PULL_REQUEST_TEMPLATE.md` is read-only input; do not modify it.

- [ ] Verify exact delivery target and final state:

  ```bash
  git fetch origin main
  git remote get-url origin
  test "$(git remote get-url origin)" = git@github-agent:agentlehub/codex-session-control.git
  test "$(git branch --show-current)" = feat/desktop-owned-session-control
  test "$(git merge-base HEAD origin/main)" = 88f2ac5b124abaa2a355ca88304e9439c692eb0a
  test -z "$(git status --short)"
  git log --oneline origin/main..HEAD
  ```

  Expected: internal origin, exact branch/base, clean tree, scoped Conventional Commits.
- [ ] Build `/tmp/csc-phase1-pr.md` using every section and checklist item from `.github/PULL_REQUEST_TEMPLATE.md`. Summary explains Desktop authority ownership and deletion-oriented architecture; Verification includes fresh focused/full/native/manual/live/review evidence; Impact includes the five-step upgrade and Phase 2 follow-up; no credentials, task data, full socket paths, or environment values.
- [ ] Push normally and create the internal PR:

  ```bash
  git push --set-upstream origin feat/desktop-owned-session-control
  gh pr create \
    --repo agentlehub/codex-session-control \
    --base main \
    --head feat/desktop-owned-session-control \
    --title "feat: use the Desktop-owned session authority" \
    --body-file /tmp/csc-phase1-pr.md
  ```

  Expected: normal push succeeds; `gh` returns an internal CSC PR URL.
- [ ] Inspect the created PR and hosted checks with `gh pr view --json url,baseRefName,headRefName,commits,statusCheckRollup`. Repair the branch and update the same PR only through another normal push if a hosted check exposes a real defect. Any tracked code, test, configuration, workflow, or documentation change after Task 20 resets the final chain: rerun Task 17, then Tasks 18-20 in order, before pushing the repair.
- [ ] Stop Phase 1 with the internal PR open. Do not merge, force-push, tag, release, publish, or update a registry.

### Task 22: Launch and Complete the Resumable Phase 2 Desktop Workstream

**Owner:** Primary orchestrator launches; autonomous fresh-session executor completes
**Files:** No CSC production/test changes and no Desktop code in this task.

- [ ] Validate the Desktop checkout read-only before writing the handoff or autonomous ledger. The known baseline is repository root `/home/korty/dev/codex-desktop-linux`, branch `main`, current base `bd610e96e87bda672f384c79ce5bb87ea0d5a6ee`, `origin=https://github.com/ilysenko/codex-desktop-linux.git`, `fork=https://github.com/kortylokai-web/codex-desktop-linux.git`, and exactly one pre-existing status entry: untracked `_experiments/`. Fetch both remotes, inspect upstream drift, shared-socket implementation, and internal-fork reachability, then choose and record the reviewed current base for the Phase 2 branch. A changed upstream SHA is not automatically a blocker, but it requires a fresh target/source inventory before branching. A path/remote mismatch or any unexpected dirt is a prerequisite failure, not authority to use another checkout silently.

  `_experiments/` is user-owned protected residue: require that it remains an untracked real directory, never read it broadly, and never add, modify, delete, move, clean, or commit it. `.autonomous/` is currently not ignored; after validation, treat only `.autonomous/csc-attached-cli-internal-pr/` as runner-owned local residue. Never stage or commit either path, never use broad `git add -A`, and exclude both paths when judging Phase 2 source dirt. Any other untracked or tracked change stops the runner for classification.
- [ ] Only after that gate passes, record the internal CSC PR URL, exact head SHA, final verification/review evidence, approved decisions, protected-residue rules, and the fact that Phase 1 is unmerged in `/home/korty/dev/codex-desktop-linux/.autonomous/csc-attached-cli-internal-pr/phase-1-handoff.md`. Create this file with `apply_patch`; it contains no credentials, environment values, task content, or full socket paths.
- [ ] Use the `autonomous-skill` runner from the validated Desktop checkout to create a durable fresh-session ledger and auto-continue until the mission checklist is complete:

  ```bash
  cd /home/korty/dev/codex-desktop-linux
  ~/.codex/skills/autonomous-skill/scripts/run-session.sh \
    --task-name csc-attached-cli-internal-pr \
    --network \
    "Read .autonomous/csc-attached-cli-internal-pr/phase-1-handoff.md. Complete the attached-CLI enhancement end to end against this checkout only. Use this existing checkout directly; do not create a worktree. Treat _experiments/ as opaque user-owned residue and never read it broadly or add, modify, delete, move, clean, or commit it. Never stage or commit .autonomous/. First use superpowers:brainstorming, then superpowers:writing-specs, then superpowers:writing-plans; independently review and approve the spec and plan under the operator's preauthorized autonomous decision set. Then execute the approved plan with TDD and subagent-driven development, verify current source and hosted checks, run final reviews in strict order: specification compliance, dedicated DRY/YAGNI, code quality. Push a normal feature branch and open a pull request only against this repository's validated internal fork remote. Do not merge, force-push, tag, release, publish, contact official upstream, or alter the unmerged CSC pull request. Mark the autonomous task complete only when the internal Desktop PR exists and its URL, exact head SHA, verification, and three ordered review results are recorded in progress.md."
  ```

  The runner owns `.autonomous/csc-attached-cli-internal-pr/task_list.md`, `progress.md`, `session.id`, and `session.log`. Its initializer must include explicit checklist items for repository/remote/branch gates, approved spec, approved TDD-ready plan, implementation, fresh verification, specification review, dedicated DRY/YAGNI inventory/deletion pass, code-quality review, normal push, and internal-fork PR inspection. The primary orchestrator waits for runner completion and reads those four files plus the created PR; it does not treat initializer or planning completion as mission completion.
- [ ] If the runner or host is interrupted, resume the exact ledger/session instead of initializing another task:

  ```bash
  cd /home/korty/dev/codex-desktop-linux
  ~/.codex/skills/autonomous-skill/scripts/run-session.sh \
    --task-name csc-attached-cli-internal-pr \
    --continue --resume-last --network
  ```

- [ ] Phase 2 must create/review its own Desktop specification and plan before Desktop code, then execute that plan through the internal Desktop `fork` PR. Do not write Desktop implementation code in the CSC checkout or this Phase 1 plan. Do not open an official-upstream issue, proposal, or PR without later explicit authorization.

## Verification

Completion requires one coherent evidence set for the final tree:

1. Repository gate: exact branch, merge-base `88f2ac5b124abaa2a355ca88304e9439c692eb0a`, approved design ancestor, clean state.
2. Version gate: canonical full-SemVer tables, exact `0.150.0-alpha.12.2` pin, generated README marker, fixture captured from `/opt/codex-desktop/resources/codex`.
3. Runtime gate: endpoint predicate suite, exact `/rpc`, complete client call-site migration, persisted resume, restart recovery, in-flight wait failure, and no-replay suites.
4. Public process gate: direct no-argument stdio, exactly 13 tools, MCP-only stdout, stderr diagnostics, no child processes, EOF exit.
5. Packaging gate: exact manifests, one locked native installer, mode/ELF/atomic staging, collision safety, cache refresh/removal retention, v1 negative control, generic-client initialize/list.
6. Deletion gate: every obsolete path absent, no legacy source symbols/commands, dependency tree pruned from evidence rather than guesswork.
7. Native gate: x86-64 and AArch64 hosted jobs each run full checks, build release, stage and inspect native ELF, execute it, and list the exact catalog.
8. Reader gate: five-step verified upgrade, current plugin/stdio guidance, available bug evidence, no removed current command.
9. Live gate: exact opt-in, private fsynced ledger, `OwnedThreadId`, all 13 tools, cleanup proof, retained ledger on failure, explicit hard-kill recovery.
10. Final order: `git diff --check` -> `./scripts/check.sh` -> specification compliance -> dedicated DRY/YAGNI -> code quality.
11. Delivery gate: clean normal push and internal CSC PR only; no merge, force-push, tag, release, publish, registry, or official-upstream action.

No result from before the final change counts as completion evidence.
