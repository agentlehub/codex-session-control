#!/usr/bin/env -S -u BASH_ENV -u ENV -u SHELLOPTS -u BASHOPTS bash --noprofile --norc
# shellcheck shell=bash
set -euo pipefail
set +m
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
test_status=
test_status_path=
ownership_failure=0
cleanup_failure_code=
pending_failure_code=

readonly term_grace_seconds=2
readonly reap_grace_seconds=3

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
    return 2
  fi
  state="$(awk '{print $3}' "/proc/$leader/stat" 2>/dev/null)" || return 2
  [[ "$state" == Z ]]
}

leader_anchor_is_absent() {
  local state
  if leader_is_waitable "$1"; then
    return 1
  else
    state=$?
  fi
  [[ "$state" -eq 2 ]]
}

wait_until_leader_is_stopped() {
  local leader="$1"
  local deadline="$2"
  local state
  while true; do
    state="$(awk '{print $3}' "/proc/$leader/stat" 2>/dev/null)" || return 1
    [[ "$state" == T ]] && return 0
    ((SECONDS < deadline)) || return 1
    sleep 0.01 || return 1
  done
}

leader_has_owned_identity() {
  local leader="$1"
  local stat fields pgid sid
  [[ -r "/proc/$leader/stat" ]] || return 1
  stat="$(<"/proc/$leader/stat")" || return 1
  fields="${stat##*) }"
  [[ "$fields" != "$stat" ]] || return 1
  read -r _ _ pgid sid _ <<<"$fields" || return 1
  [[ "$pgid" == "$leader" && "$sid" == "$leader" ]]
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
      return "$state"
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
  return 0
}

consume_saved_test_status() {
  local saved_status
  if [[ -z "$test_status_path" || ! -s "$test_status_path" ]]; then
    test_status_path=
    return 0
  fi
  if ! IFS= read -r saved_status <"$test_status_path"; then
    return 1
  fi
  if [[ "$saved_status" != 0 && ! "$saved_status" =~ ^[1-9][0-9]{0,2}$ ]] ||
    ((10#$saved_status > 255)); then
    return 1
  fi
  test_status=$((10#$saved_status))
  test_status_path=
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

reap_absent_anchor() {
  local deadline=$((SECONDS + reap_grace_seconds))
  private_wait_for_leader "$test_leader" || true
  test_leader=
  consume_saved_test_status || true
  if confirm_group_absent "$test_pgid" "$deadline"; then
    test_pgid=
  fi
  return 1
}

reap_and_release_owned_test() {
  local deadline="$1"
  local state status_state=0
  if wait_until_leader_is_waitable "$test_leader" "$deadline"; then
    state=0
  else
    state=$?
  fi
  [[ "$state" -eq 0 || "$state" -eq 2 ]] || return 1
  private_wait_for_leader "$test_leader" || return 1
  test_leader=
  consume_saved_test_status || status_state=1
  confirm_group_absent "$test_pgid" "$deadline" || return 1
  test_pgid=
  [[ "$status_state" -eq 0 ]]
}

cleanup_owned_test_inner() {
  local state deadline
  if [[ -z "$test_leader" ]]; then
    if [[ -z "$test_pgid" ]]; then
      return 0
    fi
    deadline=$((SECONDS + reap_grace_seconds))
    confirm_group_absent "$test_pgid" "$deadline" || return 1
    test_pgid=
    return 0
  fi
  [[ -n "$test_pgid" ]] || return 1

  if leader_anchor_is_absent "$test_leader"; then
    reap_absent_anchor
    return 1
  fi

  if signal_group TERM "$test_pgid"; then
    state=0
  else
    state=$?
  fi
  if [[ "$state" -eq 2 ]]; then
    return 1
  fi
  if [[ "$state" -eq 1 ]]; then
    if ! kill -KILL "$test_leader" 2>/dev/null; then
      return 1
    fi
  fi
  deadline=$((SECONDS + term_grace_seconds))
  if ! confirm_group_absent "$test_pgid" "$deadline"; then
    if leader_anchor_is_absent "$test_leader"; then
      reap_absent_anchor
      return 1
    fi
    if signal_group KILL "$test_pgid"; then
      state=0
    else
      state=$?
    fi
    [[ "$state" -ne 2 ]] || return 1
  fi

  reap_and_release_owned_test "$((SECONDS + reap_grace_seconds))"
}

cleanup_owned_test() {
  {
    cleanup_owned_test_inner >>"$private_wait_capture" 2>&1
  } 2>/dev/null
}

kill_owned_group_and_wait() {
  local deadline=$((SECONDS + reap_grace_seconds))
  {
    {
      if leader_anchor_is_absent "$test_leader"; then
        reap_absent_anchor
        return 1
      fi
      signal_group KILL "$test_pgid" || return 1
      reap_and_release_owned_test "$deadline"
    } >>"$private_wait_capture" 2>&1
  } 2>/dev/null
}

record_failure() {
  pending_failure_code="$1"
}

wait_for_test_inner() {
  local leader="$test_leader"
  local state
  ownership_failure=0
  while true; do
    if [[ -s "$test_status_path" ]]; then
      break
    fi
    if leader_is_waitable "$leader"; then
      state=0
    else
      state=$?
    fi
    case "$state" in
      0) break ;;
      1) sleep 0.05 || {
        ownership_failure=1
        return
      } ;;
      *)
        reap_absent_anchor || ownership_failure=1
        return
        ;;
    esac
  done
  cleanup_owned_test_inner || ownership_failure=1
}

wait_for_test() {
  {
    wait_for_test_inner >>"$private_wait_capture" 2>&1
  } 2>/dev/null
}

record_captured_failure() {
  local stdout_path="$1"
  local stderr_path="$2"
  local code
  for code in "${fixed_codes[@]}"; do
    if grep -Fxq "$code" "$stdout_path" "$stderr_path" 2>/dev/null; then
      record_failure "$code"
      return
    fi
  done
  record_failure tool_failed
}

wait_for_hard_kill_ready() {
  local deadline="$1"
  until grep -Fxq hard_kill_ready "$hard_kill_stdout"; do
    if [[ -s "$test_status_path" ]] || leader_anchor_is_absent "$hard_kill_leader"; then
      wait_for_test
      if [[ "$ownership_failure" -ne 0 ]]; then
        record_failure child_reap_failed
      else
        record_captured_failure "$hard_kill_stdout" "$hard_kill_stderr"
      fi
      return 1
    fi
    if ((SECONDS >= deadline)); then
      record_failure deadline_exceeded
      return 1
    fi
    sleep 0.1
  done
}

cleanup_capture() {
  if [[ -n "$cleanup_failure_code" ]]; then
    return 1
  fi
  if ! cleanup_owned_test; then
    cleanup_failure_code=child_reap_failed
    return 1
  fi
  if [[ -n "$capture_root" ]] &&
    [[ "$capture_root" == "${TMPDIR:-/tmp}"/codex-session-control-live-proof.* ]] &&
    [[ -d "$capture_root" ]]; then
    if ! rm -rf -- "$capture_root" 2>/dev/null; then
      cleanup_failure_code=cleanup_failed
      return 1
    fi
  fi
  capture_root=
  private_wait_capture=/dev/null
}

finalize() {
  local status="$1"
  local source="$2"
  local winning_code=
  trap - EXIT HUP INT TERM
  if ! cleanup_capture; then
    winning_code="$cleanup_failure_code"
  elif [[ -n "$pending_failure_code" ]]; then
    winning_code="$pending_failure_code"
  elif [[ "$source" == exit && "$status" -ne 0 ]]; then
    winning_code=tool_failed
  fi
  if [[ -n "$winning_code" ]]; then
    printf '%s\n' "$winning_code" >&2
    exit 1
  fi
  exit "$status"
}

trap 'finalize "$?" exit' EXIT
trap 'finalize 129 signal' HUP
trap 'finalize 130 signal' INT
trap 'finalize 143 signal' TERM

capture_setup_failed() {
  record_failure tool_failed
  return 1
}

new_capture_root() {
  local mode
  if ! capture_root="$(
    mktemp -d "${TMPDIR:-/tmp}/codex-session-control-live-proof.XXXXXX" 2>/dev/null
  )"; then
    capture_setup_failed
    return 1
  fi
  if ! chmod 0700 "$capture_root" 2>/dev/null; then
    capture_setup_failed
    return 1
  fi
  if ! mode="$(stat --format=%a "$capture_root" 2>/dev/null)" || [[ "$mode" != 700 ]]; then
    capture_setup_failed
    return 1
  fi
  private_wait_capture="$capture_root/private-wait"
  new_capture_file "$private_wait_capture"
}

new_capture_file() {
  local path="$1"
  local mode
  if ! : 2>/dev/null >"$path"; then
    capture_setup_failed
    return 1
  fi
  if ! chmod 0600 "$path" 2>/dev/null; then
    capture_setup_failed
    return 1
  fi
  if ! mode="$(stat --format=%a "$path" 2>/dev/null)" || [[ "$mode" != 600 ]]; then
    capture_setup_failed
    return 1
  fi
}

live_test_environment() {
  local mode="$1"
  local -n result="$2"
  result=(
    --unset=CODEX_SESSION_CONTROL_LIVE_HARD_KILL
    --unset=CODEX_SESSION_CONTROL_LIVE_RECOVER
    CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS=1
  )
  case "$mode" in
    normal) ;;
    hard-kill)
      result+=(CODEX_SESSION_CONTROL_LIVE_HARD_KILL=1)
      ;;
    recovery)
      result+=(CODEX_SESSION_CONTROL_LIVE_RECOVER=1)
      ;;
    *) return 1 ;;
  esac
}

runtime_journal_preflight() {
  local runtime="$1"
  local result_name="$2"
  local remainder component
  printf -v "$result_name" '%s' ''
  if [[ -z "$runtime" || "$runtime" != /* ]]; then
    record_failure journal_rejected
    return 1
  fi

  remainder="${runtime#/}"
  while [[ "$remainder" == */* ]]; do
    component="${remainder%%/*}"
    if [[ -z "$component" || "$component" == . || "$component" == .. ]]; then
      record_failure journal_rejected
      return 1
    fi
    remainder="${remainder#*/}"
  done
  if [[ -z "$remainder" || "$remainder" == . || "$remainder" == .. ]]; then
    record_failure journal_rejected
    return 1
  fi

  printf -v "$result_name" '%s' \
    "$runtime/codex-session-control/live-test/current.json"
}

arm_finalizer_signal_traps() {
  trap 'finalize 129 signal' HUP
  trap 'finalize 130 signal' INT
  trap 'finalize 143 signal' TERM
}

continue_owned_leader() {
  local status
  arm_finalizer_signal_traps
  if [[ -n "${deferred_signal-}" ]]; then
    finalize "$deferred_signal" signal
  fi
  # Keep finalizer output outside the command-local diagnostic capture.
  exec 9>&2 || return 1
  trap 'finalize 129 signal 2>&9' HUP
  trap 'finalize 130 signal 2>&9' INT
  trap 'finalize 143 signal 2>&9' TERM
  if kill -CONT "$test_leader" 2>>"$private_wait_capture"; then
    status=0
  else
    status=$?
  fi
  arm_finalizer_signal_traps
  exec 9>&- || return 1
  return "$status"
}

spawn_owned_anchor() {
  local stdout_path="$1"
  local stderr_path="$2"
  local status_path="$3"
  shift 3
  (
    trap 'kill -STOP "$BASHPID"' TERM
    kill -STOP "$BASHPID"
    # shellcheck disable=SC2016
    exec setsid /usr/bin/env \
      -u BASH_ENV -u ENV -u SHELLOPTS -u BASHOPTS \
      "$BASH" --noprofile --norc -c '
      status_path=$1
      shift
      kill -STOP "$BASHPID"
      trap "" TERM
      if (
        trap - TERM
        exec "$@"
      ); then
        harness_status=0
      else
        harness_status=$?
      fi
      if ! printf "%s\n" "$harness_status" >"$status_path"; then
        exit 125
      fi
      kill -STOP "$BASHPID"
      exit "$harness_status"
    ' owned-anchor "$status_path" "$@"
  ) >"$stdout_path" 2>"$stderr_path" &
  test_leader=$!
  test_pgid="$test_leader"
  test_status_path="$status_path"
  ownership_failure=0
  wait_until_leader_is_stopped \
    "$test_leader" "$((SECONDS + 1))" || return "$?"
  confirm_group_absent "$test_pgid" "$SECONDS" || return "$?"
  continue_owned_leader || return "$?"
  wait_until_group_exists "$test_pgid" "$((SECONDS + 1))" || return "$?"
  wait_until_leader_is_stopped \
    "$test_leader" "$((SECONDS + 1))" || return "$?"
  leader_has_owned_identity "$test_leader" || return "$?"
  continue_owned_leader || return "$?"
}

run_self_test() {
  local smoke_root public_capture leader_record descendant_record
  local smoke_leader smoke_pgid smoke_descendant state scenario_status

  new_capture_root || return 1
  smoke_root="$capture_root"
  public_capture="$smoke_root/public"
  leader_record="$smoke_root/leader"
  descendant_record="$smoke_root/descendant"
  new_capture_file "$public_capture" || return 1
  new_capture_file "$leader_record" || return 1
  new_capture_file "$descendant_record" || return 1

  set +e
  (
    set -e
    trap 'finalize "$?" exit' EXIT
    export TMPDIR="$smoke_root"

    new_capture_root || exit "$?"
    smoke_stdout="$capture_root/stdout"
    smoke_stderr="$capture_root/stderr"
    smoke_status="$capture_root/status"
    new_capture_file "$smoke_stdout" || exit "$?"
    new_capture_file "$smoke_stderr" || exit "$?"
    new_capture_file "$smoke_status" || exit "$?"

    # The payload shell must expand its own descendant PID.
    # shellcheck disable=SC2016
    spawn_owned_anchor \
      "$smoke_stdout" "$smoke_stderr" "$smoke_status" \
      "$BASH" --noprofile --norc -c '
        sleep 30 &
        descendant=$!
        printf "%s\n" "$descendant" >"$1"
        wait "$descendant"
      ' cleanup-smoke "$descendant_record" || exit "$?"

    deadline=$((SECONDS + 1))
    until [[ -s "$descendant_record" ]]; do
      ((SECONDS < deadline)) || exit 1
      sleep 0.01
    done
    printf '%s %s\n' "$test_leader" "$test_pgid" >"$leader_record"
    exit 97
  ) >"$public_capture" 2>&1
  scenario_status=$?
  set -e

  [[ "$scenario_status" -eq 1 ]]
  [[ "$(<"$public_capture")" == tool_failed ]]
  IFS=' ' read -r smoke_leader smoke_pgid <"$leader_record"
  IFS= read -r smoke_descendant <"$descendant_record"
  [[ "$smoke_leader" =~ ^[1-9][0-9]*$ ]]
  [[ "$smoke_pgid" == "$smoke_leader" ]]
  [[ "$smoke_descendant" =~ ^[1-9][0-9]*$ ]]
  leader_anchor_is_absent "$smoke_leader"
  if group_state "$smoke_pgid"; then
    return 1
  else
    state=$?
  fi
  [[ "$state" -eq 1 ]]
  [[ ! -e "/proc/$smoke_descendant" ]]

  cleanup_capture
  [[ ! -e "$smoke_root" ]]
  printf '%s\n' 'self_test_status=0'
}

if [[ "${1-}" == --self-test ]] && [[ "$#" -eq 1 ]]; then
  run_self_test
  exit 0
fi
if [[ "$#" -ne 0 ]]; then
  record_failure opt_in_rejected
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly script_dir
repository_root="$(cd -- "$script_dir/../.." && pwd)"
readonly repository_root
journal=
if ! runtime_journal_preflight "${XDG_RUNTIME_DIR-}" journal; then
  exit 1
fi
readonly journal

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
  record_failure tool_failed
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
  record_failure tool_failed
  exit 1
fi
match_count="$(jq --raw-output 'length' "$harnesses" 2>/dev/null)" ||
  {
    record_failure tool_failed
    exit 1
  }
if [[ "$match_count" -ne 1 ]]; then
  record_failure tool_failed
  exit 1
fi
harness="$(jq --exit-status --raw-output '.[0]' "$harnesses" 2>/dev/null)" ||
  {
    record_failure tool_failed
    exit 1
  }
readonly harness
if [[ ! -x "$harness" ]]; then
  record_failure tool_failed
  exit 1
fi

spawn_live_test() {
  local mode="$1"
  local stdout_path="$2"
  local stderr_path="$3"
  local status_path="$stdout_path.status"
  local deferred_signal='' launch_status
  new_capture_file "$stdout_path"
  new_capture_file "$stderr_path"
  new_capture_file "$status_path"
  local -a environment
  if ! live_test_environment "$mode" environment; then
    record_failure opt_in_rejected
    exit 1
  fi
  trap 'deferred_signal=129' HUP
  trap 'deferred_signal=130' INT
  trap 'deferred_signal=143' TERM
  if spawn_owned_anchor \
    "$stdout_path" "$stderr_path" "$status_path" \
    env "${environment[@]}" \
    "$harness" "$live_test_name" \
    --exact --ignored --nocapture --test-threads=1; then
    launch_status=0
  else
    launch_status=$?
  fi
  arm_finalizer_signal_traps
  if [[ -n "$deferred_signal" ]]; then
    finalize "$deferred_signal" signal
  fi
  if [[ "$launch_status" -ne 0 ]]; then
    record_failure child_reap_failed
    exit 1
  fi
}

readonly normal_stdout="$capture_root/normal.stdout"
readonly normal_stderr="$capture_root/normal.stderr"
spawn_live_test normal "$normal_stdout" "$normal_stderr"
wait_for_test
normal_status="$test_status"
if [[ "$ownership_failure" -ne 0 ]]; then
  record_failure child_reap_failed
  exit 1
fi
if [[ "$normal_status" -ne 0 ]]; then
  record_captured_failure "$normal_stdout" "$normal_stderr"
  exit 1
fi
printf '%s\n' 'normal_status=0'
jq --exit-status '. == {"state":{"kind":"idle"}}' "$journal" >/dev/null 2>&1 ||
  {
    record_failure journal_rejected
    exit 1
  }

readonly hard_kill_stdout="$capture_root/hard-kill.stdout"
readonly hard_kill_stderr="$capture_root/hard-kill.stderr"
spawn_live_test hard-kill "$hard_kill_stdout" "$hard_kill_stderr"
hard_kill_leader="$test_leader"
handshake_deadline=$((SECONDS + 180))
wait_for_hard_kill_ready "$handshake_deadline" || exit 1
if ! kill_owned_group_and_wait; then
  record_failure child_reap_failed
  exit 1
fi
hard_kill_status="$test_status"
if [[ "$hard_kill_status" -ne 137 ]]; then
  record_failure tool_failed
  exit 1
fi
printf '%s\n' 'hard_kill_status=137'
jq --exit-status '.state.kind == "active"' "$journal" >/dev/null 2>&1 ||
  {
    record_failure journal_rejected
    exit 1
  }

readonly recovery_stdout="$capture_root/recovery.stdout"
readonly recovery_stderr="$capture_root/recovery.stderr"
spawn_live_test recovery "$recovery_stdout" "$recovery_stderr"
wait_for_test
recovery_status="$test_status"
if [[ "$ownership_failure" -ne 0 ]]; then
  record_failure child_reap_failed
  exit 1
fi
if [[ "$recovery_status" -ne 0 ]]; then
  record_captured_failure "$recovery_stdout" "$recovery_stderr"
  exit 1
fi
if grep -Fxq hard_kill_ready "$recovery_stdout" "$recovery_stderr"; then
  record_failure tool_failed
  exit 1
fi
printf '%s\n' 'recovery_status=0'
jq --exit-status '. == {"state":{"kind":"idle"}}' "$journal" >/dev/null 2>&1 ||
  {
    record_failure cleanup_failed
    exit 1
  }
printf '%s\n' 'journal_state=Idle'
