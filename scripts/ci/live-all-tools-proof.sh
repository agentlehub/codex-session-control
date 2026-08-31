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

assert_deferred_launch_signals_for_self_test() {
  local self_root="$1"
  local public_capture="$self_root/deferred-signal.public"
  local leader_record="$self_root/deferred-signal.leader"
  local row signal expected continue_target cleanup_uncertain marker leader root status
  new_capture_file "$public_capture"
  new_capture_file "$leader_record"
  for row in \
    'TERM:1:1:1' \
    'HUP:129:2:0' \
    'INT:130:2:0' \
    'TERM:143:2:0'; do
    IFS=: read -r signal expected continue_target cleanup_uncertain <<<"$row"
    marker="$self_root/deferred-$continue_target-$signal-ran"
    : >"$public_capture"
    : >"$leader_record"
    # The globals are intentionally isolated inside each finalizer boundary.
    # shellcheck disable=SC2030,SC2031
    if (
      local continue_count=0 deadline deferred_signal=''
      local TMPDIR="$self_root"
      capture_root=
      private_wait_capture=/dev/null
      test_leader='' test_pgid='' test_status='' test_status_path=''
      cleanup_failure_code='' pending_failure_code=''
      new_capture_root
      trap 'finalize "$?" exit' EXIT
      trap 'deferred_signal=129' HUP
      trap 'deferred_signal=130' INT
      trap 'deferred_signal=143' TERM
      if [[ "$cleanup_uncertain" -eq 1 ]]; then
        # Reap the child but retain cleanup uncertainty for precedence proof.
        # shellcheck disable=SC2317
        private_wait_for_leader() {
          if wait "$1" >"$private_wait_capture" 2>&1; then
            test_status=0
          else
            test_status=$?
          fi
          return 1
        }
      fi
      # Keep boundary rows fast while still requiring real group disappearance.
      # shellcheck disable=SC2317
      signal_group() {
        builtin kill -KILL -- "-$2" || return 2
      }
      # Inject at the selected production authorization boundary.
      # shellcheck disable=SC2317
      kill() {
        if [[ "${1-}" == -CONT ]]; then
          ((continue_count += 1))
          if [[ "$continue_count" -ne "$continue_target" ]]; then
            builtin kill "$@"
            return
          fi
          if [[ "$continue_target" -eq 1 ]]; then
            builtin kill "$@"
            deadline=$((SECONDS + 1))
            wait_until_group_exists "${2-}" "$deadline" || return 1
            wait_until_leader_is_stopped "${2-}" "$deadline" || return 1
            leader_has_owned_identity "${2-}" || return 1
          fi
          printf '%s\n' "${2-}" >"$leader_record"
          builtin kill "-$signal" "$BASHPID"
          return 0
        fi
        builtin kill "$@"
      }
      # The marker path is expanded by the inner shell.
      # shellcheck disable=SC2016
      spawn_owned_anchor \
        "$capture_root/anchor.stdout" \
        "$capture_root/anchor.stderr" \
        "$capture_root/anchor.status" \
        /bin/sh -c 'printf "ran\n" >"$1"; exec sleep 30' marker "$marker"
      exit 1
    ) >"$public_capture" 2>&1; then
      status=0
    else
      status=$?
    fi
    [[ "$status" -eq "$expected" ]]
    if [[ "$cleanup_uncertain" -eq 1 ]]; then
      [[ "$(<"$public_capture")" == child_reap_failed ]]
    else
      [[ ! -s "$public_capture" ]]
    fi
    [[ ! -e "$marker" ]]
    leader="$(<"$leader_record")"
    [[ -n "$leader" && ! -e "/proc/$leader" ]]
    if ps -e -o pgid= 2>/dev/null | awk -v pgid="$leader" '$1 == pgid { found=1 } END { exit !found }'; then
      return 1
    fi
    if [[ "$cleanup_uncertain" -eq 1 ]]; then
      root="$(compgen -G "$self_root/codex-session-control-live-proof.*")"
      [[ -d "$root" ]]
      if ! rm -rf -- "$root" 2>/dev/null; then
        return 1
      fi
    elif compgen -G "$self_root/codex-session-control-live-proof.*" >/dev/null; then
      return 1
    fi
  done
  test_leader=
  test_pgid=
  test_status=
  test_status_path=
}

assert_private_wait_and_group_cleanup_for_self_test() {
  local self_root="$1"
  local public_capture="$self_root/public"
  local anchor_stdout="$self_root/anchor.stdout"
  local anchor_stderr="$self_root/anchor.stderr"
  local anchor_status="$self_root/anchor.status"
  local killed_leader
  local leader marker helper
  new_capture_file "$public_capture"
  new_capture_file "$anchor_stdout"
  new_capture_file "$anchor_stderr"
  new_capture_file "$anchor_status"

  spawn_owned_anchor \
    "$anchor_stdout" "$anchor_stderr" "$anchor_status" \
    /bin/sh -c 'exec sleep 30'
  killed_leader="$test_leader"
  kill_owned_group_and_wait >"$public_capture" 2>&1
  [[ "$test_status" -eq 137 ]]
  [[ -z "$test_leader" && -z "$test_pgid" ]]
  [[ ! -e "/proc/$killed_leader" ]]
  [[ ! -s "$public_capture" && ! -s "$anchor_status" ]]
  [[ ! -s "$anchor_stdout" && ! -s "$anchor_stderr" ]]

  : >"$public_capture"
  : >"$anchor_status"
  spawn_owned_anchor \
    "$anchor_stdout" "$anchor_stderr" "$anchor_status" \
    /bin/sh -c 'exit 23'
  leader="$test_leader"
  while [[ ! -s "$anchor_status" ]]; do
    ((SECONDS < 10)) || return 1
    sleep 0.01
  done
  cleanup_owned_test >"$public_capture" 2>&1 || return 1
  [[ "$test_status" -eq 23 ]]
  [[ -z "$test_leader" && -z "$test_pgid" ]]
  [[ ! -e "/proc/$leader" ]]
  [[ ! -s "$public_capture" ]]
  [[ ! -s "$anchor_stdout" && ! -s "$anchor_stderr" ]]

  : >"$public_capture"
  marker="$self_root/delayed-group-ran"
  (
    sleep 2
    printf 'ran\n' >"$marker"
    exec setsid /bin/sh -c 'exec sleep 30'
  ) >/dev/null 2>&1 &
  leader=$!
  test_leader="$leader"
  test_pgid="$leader"
  cleanup_owned_test >"$public_capture" 2>&1 || return 1
  [[ ! -e "$marker" ]]
  [[ -z "$test_leader" && -z "$test_pgid" ]]
  [[ ! -s "$public_capture" ]]

  for helper in cleanup_owned_test_inner kill_owned_group_and_wait; do
    : >"$public_capture"
    # The globals are intentionally isolated while exercising retained Bash wait status.
    # shellcheck disable=SC2030,SC2031
    (
      local unexpected_signal=0 leader test_leader test_pgid test_status
      local test_status_path=
      setsid /bin/sh -c 'exit 23' >/dev/null 2>&1 &
      leader=$!
      while [[ -e "/proc/$leader" ]]; do
        ((SECONDS < 10)) || return 1
        sleep 0.01
      done
      test_leader="$leader"
      test_pgid="$leader"
      test_status=
      # shellcheck disable=SC2317
      signal_group() { unexpected_signal=1; return 2; }
      # shellcheck disable=SC2317
      kill() { unexpected_signal=1; return 1; }
      # shellcheck disable=SC2317
      group_state() { return 1; }
      if "$helper"; then
        return 1
      fi
      [[ "$test_status" -eq 23 ]]
      [[ -z "$test_leader" && -z "$test_pgid" ]]
      [[ "$unexpected_signal" -eq 0 ]]
    )
    [[ ! -s "$public_capture" ]]
  done

  (
    local observations=0 unexpected_signal=0
    local test_leader=424242 test_pgid=424242 test_status='' test_status_path=''
    # shellcheck disable=SC2317
    wait_until_leader_is_waitable() { return 0; }
    # shellcheck disable=SC2317
    private_wait_for_leader() { test_status=23; return 0; }
    # shellcheck disable=SC2317
    group_state() {
      [[ -z "$test_leader" ]] || return 2
      ((observations += 1))
      ((observations < 3)) && return 0
      return 1
    }
    # shellcheck disable=SC2317
    signal_group() { unexpected_signal=1; return 2; }
    reap_and_release_owned_test "$((SECONDS + 1))" || return 1
    [[ "$observations" -eq 3 && "$test_status" -eq 23 &&
      -z "$test_leader" && -z "$test_pgid" && "$unexpected_signal" -eq 0 ]]
  )

  (
    local observations=0 unexpected_signal=0
    local test_leader=424242 test_pgid=424242 test_status='' test_status_path=''
    # shellcheck disable=SC2317
    wait_until_leader_is_waitable() { return 0; }
    # shellcheck disable=SC2317
    private_wait_for_leader() { test_status=23; return 0; }
    # shellcheck disable=SC2317
    group_state() {
      [[ -z "$test_leader" ]] || return 2
      ((observations += 1))
      return 2
    }
    # shellcheck disable=SC2317
    signal_group() { unexpected_signal=1; return 2; }
    if reap_and_release_owned_test "$SECONDS"; then
      return 1
    fi
    if cleanup_owned_test_inner; then
      return 1
    fi
    [[ "$observations" -ge 2 && "$test_status" -eq 23 &&
      -z "$test_leader" && "$test_pgid" -eq 424242 && "$unexpected_signal" -eq 0 ]]
  )
  test_leader=
  test_pgid=
  test_status=
  test_status_path=

}

assert_hard_kill_helper_fail_fast_for_self_test() {
  local self_root="$1"
  local trace="$self_root/helper-trace"
  local public_capture="$self_root/helper-public"
  new_capture_file "$trace"
  new_capture_file "$public_capture"

  (
    leader_is_waitable() {
      return 1
    }
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
    leader_is_waitable() {
      return 1
    }
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
    if ! kill_owned_group_and_wait; then
      printf '%s\n' helper_failed >>"$trace"
    else
      printf '%s\n' helper_succeeded >>"$trace"
    fi
  ) >"$public_capture" 2>&1
  [[ "$(tr '\n' ' ' <"$trace")" == "signal_succeeded waitability_timeout helper_failed " ]]
  [[ ! -s "$public_capture" ]]
}

assert_hard_kill_early_failure_for_self_test() {
  local self_root="$1"
  local stdout_path="$self_root/early-hard-kill.stdout"
  local stderr_path="$self_root/early-hard-kill.stderr"
  local status_path="$stdout_path.status"
  local deadline leader
  local test_leader='' test_pgid='' test_status='' test_status_path=''
  local ownership_failure=0 pending_failure_code=''
  local hard_kill_stdout="$stdout_path" hard_kill_stderr="$stderr_path"
  local hard_kill_leader
  new_capture_file "$stdout_path"
  new_capture_file "$stderr_path"
  new_capture_file "$status_path"
  spawn_owned_anchor \
    "$stdout_path" "$stderr_path" "$status_path" \
    /bin/sh -c 'printf "tool_failed\n" >&2; exit 23'
  leader="$test_leader"
  hard_kill_leader="$leader"
  deadline=$((SECONDS + 1))
  if wait_for_hard_kill_ready "$deadline"; then
    return 1
  fi
  if [[ -n "$test_leader" ]]; then
    cleanup_owned_test || return 1
  fi
  [[ "$pending_failure_code" == tool_failed &&
    "$ownership_failure" -eq 0 &&
    -z "$test_leader" && -z "$test_pgid" ]]
  [[ -n "$leader" && ! -e "/proc/$leader" ]]
  if ps -e -o pgid= 2>/dev/null | awk -v pgid="$leader" '$1 == pgid { found=1 } END { exit !found }'; then
    return 1
  fi
}

assert_live_mode_environment_for_self_test() {
  local self_root="$1"
  local capture="$self_root/live-mode-environment"
  local mode expected
  local -a environment
  new_capture_file "$capture"

  for mode in normal hard-kill recovery; do
    environment=()
    live_test_environment "$mode" environment
    # The observed shell must expand its own environment.
    # shellcheck disable=SC2016
    CODEX_SESSION_CONTROL_LIVE_HARD_KILL=hostile \
      CODEX_SESSION_CONTROL_LIVE_RECOVER=hostile \
      env "${environment[@]}" /bin/sh -c '
      printf "all_tools=%s\nhard_kill=%s\nrecovery=%s\n" \
        "${CODEX_SESSION_CONTROL_LIVE_ALL_TOOLS-absent}" \
        "${CODEX_SESSION_CONTROL_LIVE_HARD_KILL-absent}" \
        "${CODEX_SESSION_CONTROL_LIVE_RECOVER-absent}"
    ' >"$capture" 2>&1
    case "$mode" in
      normal) expected=$'all_tools=1\nhard_kill=absent\nrecovery=absent' ;;
      hard-kill) expected=$'all_tools=1\nhard_kill=1\nrecovery=absent' ;;
      recovery) expected=$'all_tools=1\nhard_kill=absent\nrecovery=1' ;;
    esac
    [[ "$(<"$capture")" == "$expected" ]]
  done
}

probe_capture_setup_failure_for_self_test() {
  local missing_tmpdir="$1"
  (
    local capture_root=
    local private_wait_capture=/dev/null
    local pending_failure_code=
    local cleanup_failure_code=
    trap 'finalize "$?" exit' EXIT
    TMPDIR="$missing_tmpdir" new_capture_root
  )
}

probe_capture_cleanup_failure_for_self_test() {
  local cleanup_target="$1"
  local status
  if (
    capture_root="$cleanup_target"
    private_wait_capture=/dev/null
    cleanup_failure_code=
    # shellcheck disable=SC2317
    rm() {
      printf 'raw rm diagnostic for %s\n' "$*" >&2
      return 1
    }
    trap 'finalize "$?" exit' EXIT
    if cleanup_capture; then
      status=0
    else
      status=$?
    fi
    exit "$status"
  ); then
    status=0
  else
    status=$?
  fi
  return "$status"
}

assert_capture_failure_boundaries_for_self_test() {
  local self_root="$1"
  local setup_stdout="$self_root/setup.stdout"
  local setup_stderr="$self_root/setup.stderr"
  local cleanup_stdout="$self_root/cleanup.stdout"
  local cleanup_stderr="$self_root/cleanup.stderr"
  local cleanup_target="$self_root/cleanup-target"
  local cleanup_evidence="$cleanup_target/private-evidence"
  local runner="${BASH_SOURCE[0]}"
  local status
  new_capture_file "$setup_stdout"
  new_capture_file "$setup_stderr"
  new_capture_file "$cleanup_stdout"
  new_capture_file "$cleanup_stderr"

  if probe_capture_setup_failure_for_self_test \
    "$self_root/absent" >"$setup_stdout" 2>"$setup_stderr"; then
    status=0
  else
    status=$?
  fi
  [[ "$status" -ne 0 ]]
  [[ ! -s "$setup_stdout" ]]
  [[ "$(<"$setup_stderr")" == tool_failed ]]

  if ! mkdir -m 0700 "$cleanup_target" 2>/dev/null; then
    record_failure tool_failed
    return 1
  fi
  new_capture_file "$cleanup_evidence"
  printf 'private path=%s pid=%s command=%s\n' \
    "$cleanup_target" 424242 'sleep 30' >"$cleanup_evidence"
  if probe_capture_cleanup_failure_for_self_test \
    "$cleanup_target" >"$cleanup_stdout" 2>"$cleanup_stderr"; then
    status=0
  else
    status=$?
  fi
  [[ "$status" -ne 0 ]]
  [[ ! -s "$cleanup_stdout" ]]
  [[ "$(<"$cleanup_stderr")" == cleanup_failed ]]
  [[ -d "$cleanup_target" ]]
  [[ -s "$cleanup_evidence" ]]
  if grep -Fq "$cleanup_target" "$cleanup_stderr" ||
    grep -Fq 424242 "$cleanup_stderr" ||
    grep -Fq 'sleep 30' "$cleanup_stderr" ||
    grep -Fq 'raw rm diagnostic' "$cleanup_stderr"; then
    return 1
  fi
  if ! rm -rf -- "$cleanup_target" 2>/dev/null; then
    record_failure cleanup_failed
    return 1
  fi
  [[ ! -e "$cleanup_target" ]]

  if TMPDIR="$self_root/absent" timeout 5s "$runner" --self-test \
    >"$setup_stdout" 2>"$setup_stderr"; then
    status=0
  else
    status=$?
  fi
  [[ "$status" -eq 1 ]]
  [[ ! -s "$setup_stdout" ]]
  [[ "$(<"$setup_stderr")" == tool_failed ]]
}

assert_runtime_journal_preflight_for_self_test() {
  local self_root="$1"
  local stdout_capture="$self_root/runtime-preflight.stdout"
  local stderr_capture="$self_root/runtime-preflight.stderr"
  local candidate journal_path status
  local pending_failure_code=
  new_capture_file "$stdout_capture"
  new_capture_file "$stderr_capture"

  for candidate in '' relative/runtime "$self_root/../runtime-authority"; do
    pending_failure_code=
    journal_path=unchanged
    if runtime_journal_preflight \
      "$candidate" journal_path >"$stdout_capture" 2>"$stderr_capture"; then
      status=0
    else
      status=$?
    fi
    [[ "$status" -ne 0 ]]
    [[ -z "$journal_path" ]]
    [[ ! -s "$stdout_capture" ]]
    [[ ! -s "$stderr_capture" ]]
    [[ "$pending_failure_code" == journal_rejected ]]
  done

  pending_failure_code=
  journal_path=
  runtime_journal_preflight /run/user/1000 journal_path \
    >"$stdout_capture" 2>"$stderr_capture"
  [[ "$journal_path" == /run/user/1000/codex-session-control/live-test/current.json ]]
  [[ ! -s "$stdout_capture" ]]
  [[ ! -s "$stderr_capture" ]]
  [[ -z "$pending_failure_code" ]]
}

run_finalizer_context_for_self_test() {
  local context="$1"
  local cleanup_outcome="$5"
  capture_root="$2"
  private_wait_capture="$3"
  pending_failure_code="$4"
  test_leader=424242
  test_pgid=424242
  ownership_failure=0
  cleanup_failure_code=
  # shellcheck disable=SC2317
  cleanup_owned_test() {
    if [[ "$cleanup_outcome" == success ]]; then
      test_leader=
      test_pgid=
      return 0
    fi
    printf 'private path=%s pid=%s command=%s\n' \
      "$capture_root" "$test_leader" 'sleep 30' >>"$private_wait_capture"
    return 1
  }
  trap 'finalize "$?" exit' EXIT
  trap 'finalize 143 signal' TERM
  case "$context" in
    exit) exit 23 ;;
    signal)
      kill -TERM "$BASHPID"
      exit 99
      ;;
  esac
}

assert_final_emission_for_self_test() {
  local self_root="$1"
  local definition scenario context pending cleanup_outcome expected expected_status
  local retained
  local retained_root private_evidence stdout_capture stderr_capture status

  for definition in \
    'bare-exit|exit|none|failure|child_reap_failed|1|1' \
    'signal-cleanup-failure|signal|none|failure|child_reap_failed|1|1' \
    'prior-deadline-cleanup-failure|exit|deadline_exceeded|failure|child_reap_failed|1|1' \
    'prior-child-reap-cleanup-failure|exit|child_reap_failed|failure|child_reap_failed|1|1' \
    'pending-deadline-cleanup-success|exit|deadline_exceeded|success|deadline_exceeded|1|0' \
    'unexpected-nonzero-cleanup-success|exit|none|success|tool_failed|1|0' \
    'signal-cleanup-success|signal|none|success|none|143|0'; do
    IFS='|' read -r \
      scenario context pending cleanup_outcome expected expected_status retained \
      <<<"$definition"
    if [[ "$pending" == none ]]; then
      pending=
    fi
    if [[ "$expected" == none ]]; then
      expected=
    fi
    retained_root="$self_root/finalizer-$scenario"
    private_evidence="$retained_root/private-wait"
    stdout_capture="$self_root/finalizer-$scenario.stdout"
    stderr_capture="$self_root/finalizer-$scenario.stderr"
    if ! mkdir -m 0700 "$retained_root" 2>/dev/null; then
      record_failure tool_failed
      return 1
    fi
    new_capture_file "$private_evidence"
    new_capture_file "$stdout_capture"
    new_capture_file "$stderr_capture"

    if (
      run_finalizer_context_for_self_test \
        "$context" "$retained_root" "$private_evidence" \
        "$pending" "$cleanup_outcome"
    ) >"$stdout_capture" 2>"$stderr_capture"; then
      status=0
    else
      status=$?
    fi

    [[ "$status" -eq "$expected_status" ]]
    [[ ! -s "$stdout_capture" ]]
    if [[ -n "$expected" ]]; then
      [[ "$(<"$stderr_capture")" == "$expected" ]]
    else
      [[ ! -s "$stderr_capture" ]]
    fi
    if grep -Fq "$retained_root" "$stderr_capture" ||
      grep -Fq 424242 "$stderr_capture" ||
      grep -Fq 'sleep 30' "$stderr_capture"; then
      return 1
    fi
    if [[ "$retained" -eq 1 ]]; then
      [[ -d "$retained_root" ]]
      [[ -s "$private_evidence" ]]
      if ! rm -rf -- "$retained_root" 2>/dev/null; then
        record_failure cleanup_failed
        return 1
      fi
    fi
    [[ ! -e "$retained_root" ]]
  done
}

assert_startup_environment_is_sanitized_for_self_test() {
  local self_root="$1"
  local capture="$self_root/startup-environment.capture"
  local startup_hook="$self_root/startup-environment-hook"
  local runner="${BASH_SOURCE[0]}"
  local status
  new_capture_file "$capture"
  new_capture_file "$startup_hook"
  # shellcheck disable=SC2016
  printf '%s\n' \
    'printf "startup_sentinel=%s\n" "${BASH_ENV-absent}" >&2' >"$startup_hook"
  if /usr/bin/env \
    _CSC_LIVE_SELF_TEST_CHILD=1 \
    BASH_ENV="$startup_hook" \
    ENV="$startup_hook" \
    SHELLOPTS=xtrace \
    BASHOPTS=extdebug \
    "$runner" --self-test >"$capture" 2>&1; then
    status=0
  else
    status=$?
  fi
  [[ "$status" -eq 0 ]]
  [[ "$(<"$capture")" == self_test_status=0 ]]
  if grep -Fq startup_sentinel "$capture" ||
    grep -Fq "$self_root" "$capture" ||
    grep -Eq '^\+' "$capture"; then
    return 1
  fi
}

run_self_test() {
  local self_root capture
  new_capture_root
  self_root="$capture_root"
  capture="$self_root/capture"
  new_capture_file "$capture"
  [[ "$(stat --format=%a "$self_root")" == 700 ]]
  printf '%s\n%s\n' hard_kill_ready hard_kill_ready_suffix >"$capture"
  [[ "$(grep -Fxc hard_kill_ready "$capture")" -eq 1 ]]

  assert_live_mode_environment_for_self_test "$self_root"
  assert_capture_failure_boundaries_for_self_test "$self_root"
  assert_runtime_journal_preflight_for_self_test "$self_root"
  assert_final_emission_for_self_test "$self_root"
  if [[ "${_CSC_LIVE_SELF_TEST_CHILD-}" != 1 ]]; then
    assert_startup_environment_is_sanitized_for_self_test "$self_root"
  fi
  assert_hard_kill_helper_fail_fast_for_self_test "$self_root"
  assert_hard_kill_early_failure_for_self_test "$self_root"
  assert_deferred_launch_signals_for_self_test "$self_root"
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
