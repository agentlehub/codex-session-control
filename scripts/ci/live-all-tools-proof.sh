#!/usr/bin/env bash
set -euo pipefail
umask 077

readonly live_test_name=live_desktop_authority_all_thirteen_tools_are_disposable
readonly fixed_codes=(
  hard_kill_ready
  opt_in_rejected
  journal_rejected
  endpoint_rejected
  identity_unverified
  version_unsupported
  child_spawn_failed
  child_reap_failed
  tool_failed
  deadline_exceeded
  archive_proof_failed
  cleanup_failed
)

capture_root=
test_pid=
test_status=

is_fixed_code() {
  local candidate="${1-}"
  local code
  for code in "${fixed_codes[@]}"; do
    if [[ "$candidate" == "$code" ]]; then
      return 0
    fi
  done
  return 1
}

require_status() {
  local actual="$1"
  local expected="$2"
  [[ "$actual" -eq "$expected" ]]
}

cleanup_capture() {
  if [[ -n "$test_pid" ]] && kill -0 "$test_pid" 2>/dev/null; then
    kill -TERM "$test_pid" 2>/dev/null || true
    wait "$test_pid" 2>/dev/null || true
  fi
  test_pid=
  if [[ -n "$capture_root" ]] &&
    [[ "$capture_root" == "${TMPDIR:-/tmp}"/codex-session-control-live-proof.* ]] &&
    [[ -d "$capture_root" ]]; then
    rm -rf -- "$capture_root"
  fi
  capture_root=
}

trap cleanup_capture EXIT HUP INT TERM

new_capture_root() {
  capture_root="$(
    mktemp -d "${TMPDIR:-/tmp}/codex-session-control-live-proof.XXXXXX"
  )"
  chmod 0700 "$capture_root"
  [[ "$(stat --format=%a "$capture_root")" == 700 ]]
}

new_capture_file() {
  local path="$1"
  : >"$path"
  chmod 0600 "$path"
  [[ "$(stat --format=%a "$path")" == 600 ]]
}

run_self_test() {
  local self_root trap_probe capture status
  new_capture_root
  self_root="$capture_root"
  trap_probe="$self_root/trap-probe"
  (
    set -euo pipefail
    mkdir -m 0700 "$trap_probe"
    trap 'rmdir "$trap_probe"' EXIT
  )
  [[ ! -e "$trap_probe" ]]

  capture="$self_root/capture"
  new_capture_file "$capture"
  [[ "$(stat --format=%a "$self_root")" == 700 ]]
  printf '%s\n%s\n' hard_kill_ready hard_kill_ready_suffix >"$capture"
  [[ "$(grep -Fxc hard_kill_ready "$capture")" -eq 1 ]]
  for status in 0 137; do
    require_status "$status" "$status"
  done
  if require_status 1 0; then
    exit 1
  fi
  local code
  for code in "${fixed_codes[@]}"; do
    is_fixed_code "$code"
  done
  if is_fixed_code hard_kill_ready_suffix; then
    exit 1
  fi
  cleanup_capture
  [[ ! -e "$self_root" ]]
  printf '%s\n' 'self_test_status=0'
}

if [[ "${1-}" == --self-test ]] && [[ "$#" -eq 1 ]]; then
  run_self_test
  exit 0
fi
if [[ "$#" -ne 0 ]]; then
  printf '%s\n' opt_in_rejected >&2
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly script_dir
repository_root="$(cd -- "$script_dir/../.." && pwd)"
readonly repository_root
: "${XDG_RUNTIME_DIR:?}"
readonly journal="$XDG_RUNTIME_DIR/codex-session-control/live-test/current.json"

new_capture_root
readonly cargo_messages="$capture_root/cargo.jsonl"
readonly cargo_stderr="$capture_root/cargo.stderr"
readonly harnesses="$capture_root/harnesses.json"
new_capture_file "$cargo_messages"
new_capture_file "$cargo_stderr"
new_capture_file "$harnesses"

if ! (
  cd "$repository_root"
  cargo test --locked --test app_server_integration --no-run \
    --message-format=json >"$cargo_messages" 2>"$cargo_stderr"
); then
  printf '%s\n' tool_failed >&2
  exit 1
fi
if ! jq --slurp '
  [
    .[] |
    select(
      .reason == "compiler-artifact" and
      .target.name == "app_server_integration" and
      .target.kind == ["test"] and
      .profile.test == true and
      (.executable | type == "string")
    ) |
    .executable
  ]
' "$cargo_messages" >"$harnesses" 2>/dev/null; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
match_count="$(jq --raw-output 'length' "$harnesses" 2>/dev/null)" ||
  {
    printf '%s\n' tool_failed >&2
    exit 1
  }
if [[ "$match_count" -ne 1 ]]; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
harness="$(jq --exit-status --raw-output '.[0]' "$harnesses" 2>/dev/null)" ||
  {
    printf '%s\n' tool_failed >&2
    exit 1
  }
readonly harness
if [[ ! -x "$harness" ]]; then
  printf '%s\n' tool_failed >&2
  exit 1
fi

spawn_live_test() {
  local mode="$1"
  local stdout_path="$2"
  local stderr_path="$3"
  new_capture_file "$stdout_path"
  new_capture_file "$stderr_path"
  local -a environment=(
    "CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS=1"
  )
  case "$mode" in
    normal) ;;
    hard-kill)
      environment+=("CODEX_SESSION_CONTROL_LIVE_HARD_KILL=1")
      ;;
    recovery)
      environment+=("CODEX_SESSION_CONTROL_LIVE_RECOVER=1")
      ;;
    *)
      printf '%s\n' opt_in_rejected >&2
      exit 1
      ;;
  esac
  setsid env "${environment[@]}" \
    "$harness" "$live_test_name" \
    --exact --ignored --nocapture --test-threads=1 \
    >"$stdout_path" 2>"$stderr_path" &
  test_pid=$!
}

wait_for_test() {
  local pid="$1"
  set +e
  wait "$pid"
  test_status=$?
  set -e
  test_pid=
}

emit_captured_failure() {
  local stdout_path="$1"
  local stderr_path="$2"
  local code
  for code in "${fixed_codes[@]}"; do
    if grep -Fxq "$code" "$stdout_path" "$stderr_path"; then
      printf '%s\n' "$code" >&2
      return
    fi
  done
  printf '%s\n' tool_failed >&2
}

readonly normal_stdout="$capture_root/normal.stdout"
readonly normal_stderr="$capture_root/normal.stderr"
spawn_live_test normal "$normal_stdout" "$normal_stderr"
wait_for_test "$test_pid"
normal_status="$test_status"
if ! require_status "$normal_status" 0; then
  emit_captured_failure "$normal_stdout" "$normal_stderr"
  exit 1
fi
printf '%s\n' 'normal_status=0'
jq --exit-status '. == {"state":{"kind":"idle"}}' "$journal" >/dev/null 2>&1 ||
  {
    printf '%s\n' journal_rejected >&2
    exit 1
  }

readonly hard_kill_stdout="$capture_root/hard-kill.stdout"
readonly hard_kill_stderr="$capture_root/hard-kill.stderr"
spawn_live_test hard-kill "$hard_kill_stdout" "$hard_kill_stderr"
hard_kill_pid="$test_pid"
handshake_deadline=$((SECONDS + 180))
until grep -Fxq hard_kill_ready "$hard_kill_stdout"; do
  if ! kill -0 "$hard_kill_pid" 2>/dev/null; then
    wait_for_test "$hard_kill_pid"
    hard_kill_status="$test_status"
    emit_captured_failure "$hard_kill_stdout" "$hard_kill_stderr"
    exit 1
  fi
  if ((SECONDS >= handshake_deadline)); then
    printf '%s\n' deadline_exceeded >&2
    exit 1
  fi
  sleep 0.1
done
if ! kill -KILL "$hard_kill_pid" 2>/dev/null; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
wait_for_test "$hard_kill_pid"
hard_kill_status="$test_status"
if ! require_status "$hard_kill_status" 137; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
if kill -0 -- "-$hard_kill_pid" 2>/dev/null; then
  printf '%s\n' child_reap_failed >&2
  exit 1
fi
printf '%s\n' 'hard_kill_status=137'
jq --exit-status '.state.kind == "active"' "$journal" >/dev/null 2>&1 ||
  {
    printf '%s\n' journal_rejected >&2
    exit 1
  }

readonly recovery_stdout="$capture_root/recovery.stdout"
readonly recovery_stderr="$capture_root/recovery.stderr"
spawn_live_test recovery "$recovery_stdout" "$recovery_stderr"
wait_for_test "$test_pid"
recovery_status="$test_status"
if ! require_status "$recovery_status" 0; then
  emit_captured_failure "$recovery_stdout" "$recovery_stderr"
  exit 1
fi
if grep -Fxq hard_kill_ready "$recovery_stdout" "$recovery_stderr"; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
printf '%s\n' 'recovery_status=0'
jq --exit-status '. == {"state":{"kind":"idle"}}' "$journal" >/dev/null 2>&1 ||
  {
    printf '%s\n' cleanup_failed >&2
    exit 1
  }
printf '%s\n' 'journal_state=Idle'
