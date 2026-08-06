#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd -- "$script_dir/../.." && pwd)"
supported_codex_version="$(cat "$repository_root/supported-codex-version.txt")"
user=codex-session-control-ci
home=/home/codex-session-control-ci
uid=
runtime=
manager=
runtime_unit=
test_harness="$home/codex-session-control-tests"
app_server_harness="$home/app-server-integration-tests"
controller_binary="$home/codex-session-control-controller"
native_codex_binary="$home/codex-$supported_codex_version"
config_dir="$home/.config/codex-session-control"
data_root="$home/.local/share/codex-session-control"
codex_home="$home/.codex"
probe_home="$home/.codex-version-probe"

cargo_messages="$RUNNER_TEMP/codex-session-control-test-harness.jsonl"
test_harnesses="$RUNNER_TEMP/codex-session-control-test-harnesses.json"
cargo test --bin codex-session-control --locked --no-run \
  --message-format=json >"$cargo_messages"
jq --slurp '
  [
    .[] |
    select(
      .reason == "compiler-artifact" and
      .target.name == "codex-session-control" and
      .target.kind == ["bin"] and
      .profile.test == true and
      (.executable | type == "string")
    ) |
    .executable
  ]
' "$cargo_messages" >"$test_harnesses"
match_count="$(jq --raw-output 'length' "$test_harnesses")"
if [[ "$match_count" -ne 1 ]]; then
  printf "%s\n" \
    "expected exactly one codex-session-control test harness, found $match_count" >&2
  exit 1
fi
test_executable="$(jq --exit-status --raw-output '.[0]' "$test_harnesses")"
test -x "$test_executable"

app_server_messages="$RUNNER_TEMP/app-server-integration-test-harness.jsonl"
app_server_harnesses="$RUNNER_TEMP/app-server-integration-test-harnesses.json"
cargo test --test app_server_integration --locked --no-run \
  --message-format=json >"$app_server_messages"
jq --slurp '
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
' "$app_server_messages" >"$app_server_harnesses"
match_count="$(jq --raw-output 'length' "$app_server_harnesses")"
if [[ "$match_count" -ne 1 ]]; then
  printf "%s\n" \
    "expected exactly one app-server integration test harness, found $match_count" >&2
  exit 1
fi
app_server_executable="$(
  jq --exit-status --raw-output '.[0]' "$app_server_harnesses"
)"
test -x "$app_server_executable"

controller_messages="$RUNNER_TEMP/codex-session-control-controller.jsonl"
controller_binaries="$RUNNER_TEMP/codex-session-control-controllers.json"
cargo build --bin codex-session-control --locked \
  --message-format=json >"$controller_messages"
jq --slurp '
  [
    .[] |
    select(
      .reason == "compiler-artifact" and
      .target.name == "codex-session-control" and
      .target.kind == ["bin"] and
      .profile.test == false and
      (.executable | type == "string")
    ) |
    .executable
  ]
' "$controller_messages" >"$controller_binaries"
match_count="$(jq --raw-output 'length' "$controller_binaries")"
if [[ "$match_count" -ne 1 ]]; then
  printf "%s\n" \
    "expected exactly one codex-session-control controller binary, found $match_count" >&2
  exit 1
fi
controller_executable="$(
  jq --exit-status --raw-output '.[0]' "$controller_binaries"
)"
test -x "$controller_executable"
controller_version="$("$controller_executable" --version)"

npm install --global "@openai/codex@$supported_codex_version"
codex_command="$(command -v codex)"
codex_wrapper="$(readlink --canonicalize "$codex_command")"
npm_codex_root="$(dirname "$(dirname "$codex_wrapper")")"
jq --arg supported_codex_version "$supported_codex_version" --exit-status '
  .name == "@openai/codex" and .version == $supported_codex_version
' "$npm_codex_root/package.json" >/dev/null
native_codex_matches=()
while IFS= read -r -d '' match; do
  native_codex_matches+=("$match")
done < <(
  find "$npm_codex_root/node_modules/@openai" \
    -type f -path '*/vendor/*/bin/codex' -print0
)
match_count="${#native_codex_matches[@]}"
if [[ "$match_count" -ne 1 ]]; then
  printf "%s\n" \
    "expected exactly one npm Codex native executable, found $match_count" >&2
  exit 1
fi
native_codex_executable="${native_codex_matches[0]}"
test -x "$native_codex_executable"
test "$("$native_codex_executable" --version)" = "codex-cli $supported_codex_version"

cleanup() {
  if [[ -n "$uid" ]]; then
    sudo loginctl disable-linger "$user" 2>/dev/null || true
    sudo loginctl terminate-user "$user" 2>/dev/null || true
    sudo systemctl stop "$manager" "$runtime_unit" 2>/dev/null || true
  fi
  sudo userdel --remove "$user" 2>/dev/null || true
}
trap cleanup EXIT

sudo useradd --create-home --home-dir "$home" --shell /bin/bash "$user"
uid="$(id -u "$user")"
manager="user@${uid}.service"
runtime_unit="user-runtime-dir@${uid}.service"
sudo install --owner "$user" --group "$user" --mode 0700 \
  "$test_executable" "$test_harness"
sudo install --owner "$user" --group "$user" --mode 0700 \
  "$app_server_executable" "$app_server_harness"
sudo install --owner "$user" --group "$user" --mode 0700 \
  "$controller_executable" "$controller_binary"
sudo install --owner "$user" --group "$user" --mode 0700 \
  "$native_codex_executable" "$native_codex_binary"
test "$(sudo stat --format=%u "$test_harness")" = "$uid"
test "$(sudo stat --format=%a "$test_harness")" = 700
for executable in \
  "$app_server_harness" \
  "$controller_binary" \
  "$native_codex_binary"; do
  test "$(sudo stat --format=%u "$executable")" = "$uid"
  test "$(sudo stat --format=%a "$executable")" = 700
done
sudo install --directory --owner "$user" --group "$user" --mode 0700 \
  "$probe_home"
test "$(sudo stat --format=%F "$probe_home")" = directory
test "$(sudo stat --format=%u "$probe_home")" = "$uid"
test "$(sudo stat --format=%a "$probe_home")" = 700
test "$(
  sudo -u "$user" env \
    HOME="$home" \
    CODEX_HOME="$probe_home" \
    "$native_codex_binary" --version
)" = "codex-cli $supported_codex_version"
test "$(sudo -u "$user" "$controller_binary" --version)" = "$controller_version"

show_manager_diagnostics() {
  for unit in "$runtime_unit" "$manager"; do
    sudo systemctl status "$unit" --no-pager --full || true
    sudo journalctl --unit "$unit" --boot --no-pager || true
  done
}

start_manager() {
  sudo loginctl enable-linger "$user" || return
  sudo systemctl start "$manager" || return
  sudo systemctl is-active --quiet "$runtime_unit" || return
  sudo systemctl is-active --quiet "$manager" || return
  runtime="$(sudo loginctl show-user "$user" --property=RuntimePath --value)" ||
    return
  test "$runtime" = "/run/user/$uid" || return
  test "$(sudo stat --format=%u "$runtime")" = "$uid" || return
  test "$(sudo stat --format=%a "$runtime")" = 700 || return
  sudo test -S "$runtime/bus" || return
  sudo -u "$user" env \
    HOME="$home" \
    XDG_RUNTIME_DIR="$runtime" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    systemctl --user list-units >/dev/null || return
}

if ! start_manager; then
  show_manager_diagnostics
  exit 1
fi

sudo -u "$user" test ! -e "$codex_home"
sudo -u "$user" test ! -L "$codex_home"
sudo -u "$user" env \
  HOME="$home" \
  XDG_RUNTIME_DIR="$runtime" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
  CI=1 \
  CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER=1 \
  "$test_harness" \
    --exact install::tests::disposable_systemd_user \
    --ignored --nocapture

runtime_dir="$runtime/codex-session-control"
for path in "$runtime_dir" "$data_root" "$config_dir"; do
  if sudo -u "$user" test -d "$path"; then
    sudo -u "$user" rmdir "$path"
  fi
  sudo -u "$user" test ! -e "$path"
done

live_count="$(
  sudo -u "$user" env \
    HOME="$home" \
    XDG_RUNTIME_DIR="$runtime" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    CI=1 \
    CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER=1 \
    CODEX_SESSION_CONTROL_DISPOSABLE_CLI_CANARY=1 \
    CODEX_SESSION_CONTROL_CODEX_BIN="$native_codex_binary" \
    CODEX_SESSION_CONTROL_CONTROLLER_BIN="$controller_binary" \
    "$app_server_harness" live_normal_home_ --ignored --list |
    grep -Ec '^live_normal_home_.*: test$'
)"
test "$live_count" -eq 4
sudo -u "$user" env \
  HOME="$home" \
  XDG_RUNTIME_DIR="$runtime" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
  CI=1 \
  CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER=1 \
  CODEX_SESSION_CONTROL_DISPOSABLE_CLI_CANARY=1 \
  CODEX_SESSION_CONTROL_CODEX_BIN="$native_codex_binary" \
  CODEX_SESSION_CONTROL_CONTROLLER_BIN="$controller_binary" \
  "$app_server_harness" live_normal_home_ \
    --ignored --nocapture --test-threads=1
sudo -u "$user" test ! -e "$codex_home"
sudo -u "$user" env \
  HOME="$home" \
  XDG_RUNTIME_DIR="$runtime" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
  CI=1 \
  CODEX_SESSION_CONTROL_DISPOSABLE_SYSTEMD_USER=1 \
  CODEX_SESSION_CONTROL_DISPOSABLE_CLI_CANARY=1 \
  CODEX_SESSION_CONTROL_CODEX_BIN="$native_codex_binary" \
  CODEX_SESSION_CONTROL_CONTROLLER_BIN="$controller_binary" \
  "$app_server_harness" --ignored \
    --nocapture --test-threads=1 --skip live_normal_home_
sudo -u "$user" test ! -e "$codex_home"
sudo -u "$user" test ! -e "$home/.local/bin/codex-session-control"
sudo -u "$user" \
  test ! -e "$home/.config/systemd/user/codex-session-control.service"
sudo -u "$user" \
  test ! -e "$runtime/codex-session-control/app-server.sock"
sudo -u "$user" test ! -e "$config_dir"
sudo -u "$user" test ! -e "$data_root"
sudo -u "$user" test ! -e "$runtime_dir"
if sudo -u "$user" env \
  HOME="$home" \
  XDG_RUNTIME_DIR="$runtime" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
  systemctl --user list-unit-files --no-legend |
  grep -q "codex-session-control-test-"; then
  printf "%s\n" "disposable systemd unit survived cleanup" >&2
  exit 1
fi

cleanup
trap - EXIT
test ! -e "$runtime"
test ! -e "$home"
test ! -e "/var/lib/systemd/linger/$user"
if sudo systemctl is-active --quiet "$manager"; then
  printf "%s\n" "disposable user manager survived cleanup" >&2
  exit 1
fi
if sudo systemctl is-active --quiet "$runtime_unit"; then
  printf "%s\n" "disposable runtime unit survived cleanup" >&2
  exit 1
fi
! id "$user" >/dev/null 2>&1
