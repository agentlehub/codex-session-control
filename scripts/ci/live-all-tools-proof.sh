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
private_wait_capture=/dev/null
test_leader=
test_pgid=
leader_reaped=0
test_status=
ownership_failure=0

readonly term_grace_seconds=2
readonly reap_grace_seconds=3

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

group_state() {
  local pgid="$1"
  local diagnostic status
  if diagnostic="$(LC_ALL=C kill -0 -- "-$pgid" 2>&1)"; then
    status=0
  else
    status=$?
  fi
  if [[ "$status" -eq 0 ]]; then
    return 0
  fi
  if [[ "$diagnostic" == *"No such process"* ]]; then
    return 1
  fi
  return 2
}

signal_group() {
  local signal="$1"
  local pgid="$2"
  local diagnostic status
  if diagnostic="$(LC_ALL=C kill "-$signal" -- "-$pgid" 2>&1)"; then
    status=0
  else
    status=$?
  fi
  if [[ "$status" -eq 0 ]]; then
    return 0
  fi
  if [[ "$diagnostic" == *"No such process"* ]]; then
    return 1
  fi
  return 2
}

leader_is_waitable() {
  local leader="$1"
  local state
  if [[ ! -r "/proc/$leader/stat" ]]; then
    return 0
  fi
  state="$(awk '{print $3}' "/proc/$leader/stat" 2>/dev/null)" || return 2
  [[ "$state" == Z ]]
}

wait_until_leader_is_waitable() {
  local leader="$1"
  local deadline="$2"
  local state
  while true; do
    if leader_is_waitable "$leader"; then
      return 0
    else
      state=$?
    fi
    if [[ "$state" -ne 1 ]]; then
      return 1
    fi
    if ((SECONDS >= deadline)); then
      return 1
    fi
    sleep 0.05 || return 1
  done
}

private_wait_for_leader() {
  local leader="$1"
  local status
  if wait "$leader" >"$private_wait_capture" 2>&1; then
    status=0
  else
    status=$?
  fi
  if [[ "$status" -eq 127 ]]; then
    return 1
  fi
  test_status="$status"
  leader_reaped=1
  return 0
}

confirm_group_absent() {
  local pgid="$1"
  local deadline="$2"
  local state
  while true; do
    if group_state "$pgid"; then
      state=0
    else
      state=$?
    fi
    case "$state" in
      1) return 0 ;;
      2) return 1 ;;
    esac
    if ((SECONDS >= deadline)); then
      return 1
    fi
    sleep 0.05 || return 1
  done
}

wait_until_group_exists() {
  local pgid="$1"
  local deadline="$2"
  local state
  while true; do
    if group_state "$pgid"; then
      state=0
    else
      state=$?
    fi
    case "$state" in
      0) return 0 ;;
      2) return 1 ;;
    esac
    if ((SECONDS >= deadline)); then
      return 1
    fi
    sleep 0.01
  done
}

release_owned_test() {
  local state
  if [[ "$leader_reaped" -ne 1 ]]; then
    return 1
  fi
  if group_state "$test_pgid"; then
    state=0
  else
    state=$?
  fi
  if [[ "$state" -eq 1 ]]; then
    test_leader=
    test_pgid=
    leader_reaped=0
    return 0
  fi
  return 1
}

cleanup_owned_test_inner() {
  local state deadline
  if [[ -z "$test_leader" || -z "$test_pgid" ]]; then
    return 0
  fi

  if signal_group TERM "$test_pgid"; then
    state=0
  else
    state=$?
  fi
  if [[ "$state" -eq 2 ]]; then
    return 1
  fi
  deadline=$((SECONDS + term_grace_seconds))
  if confirm_group_absent "$test_pgid" "$deadline"; then
    state=0
  else
    state=$?
  fi
  if [[ "$state" -ne 0 ]]; then
    if signal_group KILL "$test_pgid"; then
      state=0
    else
      state=$?
    fi
    if [[ "$state" -eq 2 ]]; then
      return 1
    fi
  fi

  if [[ "$leader_reaped" -ne 1 ]]; then
    deadline=$((SECONDS + reap_grace_seconds))
    wait_until_leader_is_waitable "$test_leader" "$deadline" || return 1
    private_wait_for_leader "$test_leader" || return 1
  fi
  deadline=$((SECONDS + reap_grace_seconds))
  confirm_group_absent "$test_pgid" "$deadline" || return 1
  release_owned_test
}

cleanup_owned_test() {
  cleanup_owned_test_inner >>"$private_wait_capture" 2>&1
}

kill_owned_group_and_wait() {
  {
    signal_group KILL "$test_pgid" || return 1
    wait_until_leader_is_waitable \
      "$test_leader" "$((SECONDS + reap_grace_seconds))" || return 1
    private_wait_for_leader "$test_leader" || return 1
    confirm_group_absent \
      "$test_pgid" "$((SECONDS + reap_grace_seconds))" || return 1
    release_owned_test || return 1
  } >>"$private_wait_capture" 2>&1
}

cleanup_capture() {
  if ! cleanup_owned_test; then
    ownership_failure=1
  fi
  if [[ -n "$capture_root" ]] &&
    [[ "$capture_root" == "${TMPDIR:-/tmp}"/codex-session-control-live-proof.* ]] &&
    [[ -d "$capture_root" ]]; then
    rm -rf -- "$capture_root"
  fi
  capture_root=
  private_wait_capture=/dev/null
}

handle_signal() {
  local status="$1"
  trap - EXIT HUP INT TERM
  cleanup_capture
  exit "$status"
}

trap cleanup_capture EXIT
trap 'handle_signal 129' HUP
trap 'handle_signal 130' INT
trap 'handle_signal 143' TERM

new_capture_root() {
  capture_root="$(
    mktemp -d "${TMPDIR:-/tmp}/codex-session-control-live-proof.XXXXXX"
  )"
  chmod 0700 "$capture_root"
  [[ "$(stat --format=%a "$capture_root")" == 700 ]]
  private_wait_capture="$capture_root/private-wait"
  new_capture_file "$private_wait_capture"
}

new_capture_file() {
  local path="$1"
  : >"$path"
  chmod 0600 "$path"
  [[ "$(stat --format=%a "$path")" == 600 ]]
}

assert_private_wait_and_group_cleanup_for_self_test() {
  local self_root="$1"
  local public_capture="$self_root/public"
  local killed_leader killed_pgid cleanup_leader cleanup_pgid
  new_capture_file "$public_capture"

  setsid /bin/sh -c 'exec sleep 30' >/dev/null 2>&1 &
  test_leader=$!
  test_pgid="$test_leader"
  leader_reaped=0
  wait_until_group_exists "$test_pgid" "$((SECONDS + 1))"
  killed_leader="$test_leader"
  killed_pgid="$test_pgid"
  kill_owned_group_and_wait >"$public_capture" 2>&1
  [[ "$test_status" -eq 137 ]]
  [[ -z "$test_leader" && -z "$test_pgid" ]]
  if grep -Fq "$killed_leader" "$public_capture"; then
    return 1
  fi
  if grep -Fq 'sleep 30' "$public_capture"; then
    return 1
  fi
  [[ ! -s "$public_capture" ]]
  local killed_group_state
  if group_state "$killed_pgid"; then
    killed_group_state=0
  else
    killed_group_state=$?
  fi
  [[ "$killed_group_state" -eq 1 ]]

  : >"$public_capture"
  setsid /bin/sh -c 'exec sleep 30' >/dev/null 2>&1 &
  test_leader=$!
  test_pgid="$test_leader"
  leader_reaped=0
  wait_until_group_exists "$test_pgid" "$((SECONDS + 1))"
  cleanup_leader="$test_leader"
  cleanup_pgid="$test_pgid"
  cleanup_owned_test >"$public_capture" 2>&1
  [[ -z "$test_leader" && -z "$test_pgid" ]]
  if grep -Fq "$cleanup_leader" "$public_capture"; then
    return 1
  fi
  if grep -Fq 'sleep 30' "$public_capture"; then
    return 1
  fi
  [[ ! -s "$public_capture" ]]
  local cleanup_group_state
  if group_state "$cleanup_pgid"; then
    cleanup_group_state=0
  else
    cleanup_group_state=$?
  fi
  [[ "$cleanup_group_state" -eq 1 ]]
}

assert_hard_kill_helper_fail_fast_for_self_test() {
  local self_root="$1"
  local trace="$self_root/helper-trace"
  local public_capture="$self_root/helper-public"
  new_capture_file "$trace"
  new_capture_file "$public_capture"

  (
    signal_group() {
      printf '%s\n' signal_error >>"$trace"
      return 2
    }
    wait_until_leader_is_waitable() {
      printf '%s\n' waitability_reached >>"$trace"
      return 1
    }
    private_wait_for_leader() {
      printf '%s\n' private_wait_reached >>"$trace"
      return 0
    }
    confirm_group_absent() {
      printf '%s\n' absence_reached >>"$trace"
      return 1
    }
    release_owned_test() {
      printf '%s\n' release_reached >>"$trace"
      return 0
    }
    if ! kill_owned_group_and_wait; then
      printf '%s\n' helper_failed >>"$trace"
    else
      printf '%s\n' helper_succeeded >>"$trace"
    fi
  ) >"$public_capture" 2>&1
  [[ "$(tr '\n' ' ' <"$trace")" == "signal_error helper_failed " ]]
  [[ ! -s "$public_capture" ]]

  : >"$trace"
  (
    signal_group() {
      printf '%s\n' signal_succeeded >>"$trace"
      return 0
    }
    wait_until_leader_is_waitable() {
      printf '%s\n' waitability_timeout >>"$trace"
      return 1
    }
    private_wait_for_leader() {
      printf '%s\n' private_wait_reached >>"$trace"
      return 0
    }
    confirm_group_absent() {
      printf '%s\n' absence_reached >>"$trace"
      return 1
    }
    release_owned_test() {
      printf '%s\n' release_reached >>"$trace"
      return 0
    }
    if ! kill_owned_group_and_wait; then
      printf '%s\n' helper_failed >>"$trace"
    else
      printf '%s\n' helper_succeeded >>"$trace"
    fi
  ) >"$public_capture" 2>&1
  [[ "$(tr '\n' ' ' <"$trace")" == "signal_succeeded waitability_timeout helper_failed " ]]
  [[ ! -s "$public_capture" ]]
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
  assert_hard_kill_helper_fail_fast_for_self_test "$self_root"
  assert_private_wait_and_group_cleanup_for_self_test "$self_root"
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
  test_leader=$!
  test_pgid="$test_leader"
  leader_reaped=0
  ownership_failure=0
  wait_until_group_exists "$test_pgid" "$((SECONDS + 1))" || {
    printf '%s\n' child_reap_failed >&2
    exit 1
  }
}

wait_for_test_inner() {
  local leader="$test_leader"
  local pgid="$test_pgid"
  local state
  ownership_failure=0
  if ! private_wait_for_leader "$leader"; then
    ownership_failure=1
    return
  fi
  if group_state "$pgid"; then
    state=0
  else
    state=$?
  fi
  if [[ "$state" -eq 1 ]]; then
    release_owned_test || ownership_failure=1
  else
    ownership_failure=1
  fi
}

wait_for_test() {
  wait_for_test_inner >>"$private_wait_capture" 2>&1
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
wait_for_test
normal_status="$test_status"
if [[ "$ownership_failure" -ne 0 ]]; then
  printf '%s\n' child_reap_failed >&2
  exit 1
fi
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
hard_kill_leader="$test_leader"
hard_kill_pgid="$test_pgid"
handshake_deadline=$((SECONDS + 180))
until grep -Fxq hard_kill_ready "$hard_kill_stdout"; do
  if ! kill -0 "$hard_kill_leader" 2>/dev/null; then
    wait_for_test
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
if ! kill_owned_group_and_wait; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
hard_kill_status="$test_status"
if ! require_status "$hard_kill_status" 137; then
  printf '%s\n' tool_failed >&2
  exit 1
fi
if group_state "$hard_kill_pgid"; then
  hard_kill_group_state=0
else
  hard_kill_group_state=$?
fi
if [[ "$hard_kill_group_state" -ne 1 ]]; then
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
wait_for_test
recovery_status="$test_status"
if [[ "$ownership_failure" -ne 0 ]]; then
  printf '%s\n' child_reap_failed >&2
  exit 1
fi
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
