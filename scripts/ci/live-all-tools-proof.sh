#!/usr/bin/env bash
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
leader_reaped=0
test_status=
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
  test_leader=
  test_pgid=
  leader_reaped=0
  [[ "$state" -eq 1 ]]
}

reap_and_release_owned_test() {
  local deadline="$1"
  local state
  if wait_until_leader_is_waitable "$test_leader" "$deadline"; then
    state=0
  else
    state=$?
  fi
  [[ "$state" -eq 0 || "$state" -eq 2 ]] || return 1
  if ! private_wait_for_leader "$test_leader"; then
    test_leader=
    test_pgid=
    leader_reaped=0
    return 1
  fi
  release_owned_test
}

cleanup_owned_test_inner() {
  local state deadline leader_signal
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
  if [[ "$state" -eq 1 ]]; then
    leader_signal=KILL
  else
    leader_signal=TERM
  fi
  if ! kill "-$leader_signal" "$test_leader" 2>/dev/null &&
    kill -0 "$test_leader" 2>/dev/null; then
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

  reap_and_release_owned_test "$((SECONDS + reap_grace_seconds))"
}

cleanup_owned_test() {
  {
    cleanup_owned_test_inner >>"$private_wait_capture" 2>&1
  } 2>/dev/null
}

kill_owned_group_and_wait() {
  {
    {
      signal_group KILL "$test_pgid" || return 1
      reap_and_release_owned_test "$((SECONDS + reap_grace_seconds))"
    } >>"$private_wait_capture" 2>&1
  } 2>/dev/null
}

record_failure() {
  pending_failure_code="$1"
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

assert_private_wait_and_group_cleanup_for_self_test() {
  local self_root="$1"
  local public_capture="$self_root/public"
  local killed_leader killed_pgid cleanup_leader cleanup_pgid
  local scenario leader marker deferred_signal
  new_capture_file "$public_capture"

  setsid /bin/sh -c 'exec sleep 30' >/dev/null 2>&1 &
  test_leader=$!
  test_pgid="$test_leader"
  leader_reaped=0
  wait_until_group_exists "$test_pgid" "$((SECONDS + reap_grace_seconds))"
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
  wait_until_group_exists "$test_pgid" "$((SECONDS + reap_grace_seconds))" || return 1
  cleanup_leader="$test_leader"
  cleanup_pgid="$test_pgid"
  cleanup_owned_test >"$public_capture" 2>&1 || return 1
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

  for scenario in launch-signal delayed-group owned-launch; do
    : >"$public_capture"
    marker="$self_root/$scenario-ran"
    if [[ "$scenario" == launch-signal ]]; then
      deferred_signal=
      trap 'deferred_signal=143' TERM
    fi
    (
      if [[ "$scenario" != delayed-group ]]; then
        kill -STOP "$BASHPID"
      else
        sleep 2
      fi
      printf 'ran\n' >"$marker"
      exec setsid /bin/sh -c 'exec sleep 30'
    ) >/dev/null 2>&1 &
    leader=$!
    if [[ "$scenario" == launch-signal ]]; then
      kill -TERM "$BASHPID"
    fi
    test_leader="$leader"
    test_pgid="$leader"
    leader_reaped=0
    if [[ "$scenario" == launch-signal ]]; then
      trap 'finalize 143 signal' TERM
      [[ "$deferred_signal" -eq 143 ]]
      wait_until_leader_is_stopped "$leader" "$((SECONDS + 1))"
    elif [[ "$scenario" == owned-launch ]]; then
      wait_until_leader_is_stopped "$leader" "$((SECONDS + 1))"
      confirm_group_absent "$leader" "$SECONDS"
      kill -CONT "$leader"
      wait_until_group_exists "$leader" "$((SECONDS + 1))"
      leader_has_owned_identity "$leader"
    fi
    if ! cleanup_owned_test >"$public_capture" 2>&1; then
      kill -CONT "$leader" 2>/dev/null || true
      wait_until_group_exists "$leader" "$((SECONDS + reap_grace_seconds))" || true
      cleanup_owned_test >"$public_capture" 2>&1 || return 1
      return 1
    fi
    if [[ "$scenario" == owned-launch ]]; then
      [[ -e "$marker" ]]
    else
      [[ ! -e "$marker" ]]
    fi
    [[ -z "$test_leader" && -z "$test_pgid" ]]
    [[ ! -s "$public_capture" ]]
  done

  (
    test_leader=424242
    test_pgid=424242
    leader_reaped=1
    group_state() {
      return 0
    }
    if release_owned_test; then
      return 1
    fi
    [[ -z "$test_leader" && -z "$test_pgid" && "$leader_reaped" -eq 0 ]]
  )
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
  leader_reaped=0
  ownership_failure=0
  cleanup_failure_code=
  # shellcheck disable=SC2317
  cleanup_owned_test() {
    if [[ "$cleanup_outcome" == success ]]; then
      test_leader=
      test_pgid=
      leader_reaped=0
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
  local deferred_signal=
  new_capture_file "$stdout_path"
  new_capture_file "$stderr_path"
  local -a environment
  if ! live_test_environment "$mode" environment; then
    record_failure opt_in_rejected
    exit 1
  fi
  trap 'deferred_signal=129' HUP
  trap 'deferred_signal=130' INT
  trap 'deferred_signal=143' TERM
  (
    kill -STOP "$BASHPID"
    exec setsid env "${environment[@]}" \
      "$harness" "$live_test_name" \
      --exact --ignored --nocapture --test-threads=1
  ) >"$stdout_path" 2>"$stderr_path" &
  test_leader=$!
  test_pgid="$test_leader"
  leader_reaped=0
  ownership_failure=0
  trap 'finalize 129 signal' HUP
  trap 'finalize 130 signal' INT
  trap 'finalize 143 signal' TERM
  if [[ -n "$deferred_signal" ]]; then
    finalize "$deferred_signal" signal
  fi
  if ! wait_until_leader_is_stopped \
    "$test_leader" "$((SECONDS + 1))" ||
    ! confirm_group_absent "$test_pgid" "$SECONDS" ||
    ! kill -CONT "$test_leader" 2>/dev/null ||
    ! wait_until_group_exists "$test_pgid" "$((SECONDS + 1))" ||
    ! leader_has_owned_identity "$test_leader"; then
    record_failure child_reap_failed
    exit 1
  fi
}

wait_for_test_inner() {
  local leader="$test_leader"
  local state
  ownership_failure=0
  while true; do
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
        reap_and_release_owned_test "$SECONDS" || ownership_failure=1
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
hard_kill_pgid="$test_pgid"
handshake_deadline=$((SECONDS + 180))
until grep -Fxq hard_kill_ready "$hard_kill_stdout"; do
  if ! kill -0 "$hard_kill_leader" 2>/dev/null; then
    wait_for_test
    hard_kill_status="$test_status"
    record_captured_failure "$hard_kill_stdout" "$hard_kill_stderr"
    exit 1
  fi
  if ((SECONDS >= handshake_deadline)); then
    record_failure deadline_exceeded
    exit 1
  fi
  sleep 0.1
done
if ! kill_owned_group_and_wait; then
  record_failure tool_failed
  exit 1
fi
hard_kill_status="$test_status"
if [[ "$hard_kill_status" -ne 137 ]]; then
  record_failure tool_failed
  exit 1
fi
if group_state "$hard_kill_pgid"; then
  hard_kill_group_state=0
else
  hard_kill_group_state=$?
fi
if [[ "$hard_kill_group_state" -ne 1 ]]; then
  record_failure child_reap_failed
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
