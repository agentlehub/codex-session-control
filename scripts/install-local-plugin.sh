#!/usr/bin/env bash

set -euo pipefail

die() {
  printf '%s\n' "$1" >&2
  exit 1
}

run_codex() {
  env MISE_QUIET=1 codex "$@"
}

require_single_json_object() {
  jq -e --slurp 'length == 1 and (.[0] | type == "object")' <<<"$1" >/dev/null
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
clone_root="$(cd -- "$script_dir/.." && pwd -P)"
plugin_root="$clone_root/plugins/codex-session-control"
plugin_manifest="$plugin_root/.codex-plugin/plugin.json"
mcp_manifest="$plugin_root/.mcp.json"
marketplace_manifest="$clone_root/.agents/plugins/marketplace.json"
staged_binary="$plugin_root/bin/codex-session-control"

test -d "$clone_root" || die 'Checkout root is unavailable.'
require_checkout_directory() {
  local path="$1"
  local canonical_path

  test -d "$path" || die 'Checkout directory is unavailable.'
  test ! -L "$path" || die 'Checkout directory must not be a symlink.'
  canonical_path="$(realpath -e -- "$path")" || die 'Checkout directory is unavailable.'
  case "$canonical_path" in
    "$clone_root"/*) ;;
    *) die 'Checkout directory escapes the canonical checkout.' ;;
  esac
  test "$canonical_path" = "$path" || die 'Checkout directory must not be a symlink.'
}

require_checkout_directory "$clone_root/.agents"
require_checkout_directory "$clone_root/.agents/plugins"
require_checkout_directory "$clone_root/plugins"
require_checkout_directory "$plugin_root"
require_checkout_directory "$plugin_root/.codex-plugin"
require_checkout_directory "$plugin_root/bin"
test ! -L "$plugin_manifest" || die 'Plugin manifest must not be a symlink.'
test ! -L "$mcp_manifest" || die 'MCP manifest must not be a symlink.'
test ! -L "$marketplace_manifest" || die 'Marketplace manifest must not be a symlink.'
test ! -L "$staged_binary" || die 'Staged executable must not be a symlink.'

case "$(uname -m)" in
  x86_64|amd64) expected_machine='Advanced Micro Devices X86-64' ;;
  aarch64|arm64) expected_machine='AArch64' ;;
  *) printf 'Unsupported architecture: %s\n' "$(uname -m)" >&2; exit 2 ;;
esac

for manifest in "$marketplace_manifest" "$plugin_manifest" "$mcp_manifest"; do
  test -f "$manifest" || die 'Plugin manifest is missing.'
  jq empty "$manifest" >/dev/null || die 'Plugin manifest is not valid JSON.'
done

cargo_metadata="$(cd -- "$clone_root" && cargo metadata --locked --no-deps --format-version 1)"
cargo_version="$(jq -er '
  [.packages[] | select(.name == "codex-session-control") | .version] |
  if length == 1 then .[0] else error("expected one CSC package") end
' <<<"$cargo_metadata")" || die 'Cargo metadata did not identify one plugin version.'

jq -e '
  . == {
    name: "codex-session-control-local",
    interface: {displayName: "Codex session control"},
    plugins: [{
      name: "codex-session-control",
      source: {source: "local", path: "./plugins/codex-session-control"},
      policy: {installation: "AVAILABLE"},
      category: "Coding"
    }]
  }
' "$marketplace_manifest" >/dev/null || die 'Marketplace manifest does not match the checkout-local contract.'

jq -e --arg version "$cargo_version" '
  . == {
    name: "codex-session-control",
    version: $version,
    description: "Control Codex sessions via MCP",
    author: {name: "Agentlehub"},
    license: "MIT",
    mcpServers: "./.mcp.json",
    interface: {
      displayName: "Codex session control",
      shortDescription: "Control Codex sessions via MCP",
      category: "Coding",
      capabilities: ["Read", "Write"]
    }
  }
' "$plugin_manifest" >/dev/null || die 'Legacy plugin manifest does not match Cargo metadata.'

jq -e '
  . == {
    mcpServers: {
      "codex-session-control": {
        type: "stdio",
        command: "./bin/codex-session-control",
        cwd: ".",
        env_vars: [
          "XDG_RUNTIME_DIR",
          "CODEX_LINUX_APP_ID",
          "CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET"
        ],
        tool_timeout_sec: 86460
      }
    }
  }
' "$mcp_manifest" >/dev/null || die 'Legacy MCP manifest does not match the forwarding contract.'
test ! -e "$plugin_root/mcp.json" || die 'The v1 root MCP manifest is not supported yet.'

(cd "$clone_root" && cargo build --release --locked)
candidate="$clone_root/target/release/codex-session-control"
if ! { test -f "$candidate" && test -x "$candidate" && test ! -L "$candidate"; }; then
  die 'Locked build did not produce a regular executable.'
fi
stage="$(mktemp "$plugin_root/bin/.codex-session-control.XXXXXX")"
trap 'rm -f -- "${stage:-}"' EXIT
cp -- "$candidate" "$stage"
chmod 0755 "$stage"
if ! { test -f "$stage" && test ! -L "$stage"; }; then
  die 'Temporary staging file is not regular.'
fi
readelf --file-header "$stage" | grep -F 'Machine:' | grep -F "$expected_machine" >/dev/null \
  || die 'Built executable does not match the current architecture.'
mv -fT -- "$stage" "$staged_binary"
stage=

marketplace_json="$(run_codex plugin marketplace list --json)" \
  || die 'Codex marketplace listing failed.'
require_single_json_object "$marketplace_json" \
  || die 'Codex marketplace listing was not valid machine-readable JSON.'
marketplace_count="$(jq -er --arg name 'codex-session-control-local' '
  if (.marketplaces | type) == "array" then
    [.marketplaces[] | select(.name == $name)] | length
  else
    error("marketplaces must be an array")
  end
' <<<"$marketplace_json")" \
  || die 'Codex marketplace listing was not valid machine-readable JSON.'

case "$marketplace_count" in
  0)
    marketplace_add_json="$(run_codex plugin marketplace add "$clone_root" --json)" \
      || die 'Codex marketplace registration failed.'
    require_single_json_object "$marketplace_add_json" \
      || die 'Codex marketplace registration was not valid machine-readable JSON.'
    ;;
  1)
    jq -e --arg name 'codex-session-control-local' --arg root "$clone_root" '
      [.marketplaces[] | select(.name == $name)] | .[0].root == $root
    ' <<<"$marketplace_json" >/dev/null \
      || die 'Marketplace name already targets another root.'
    ;;
  *)
    die 'Marketplace name resolves to multiple roots.'
    ;;
esac

marketplace_json="$(run_codex plugin marketplace list --json)" \
  || die 'Codex marketplace listing failed.'
require_single_json_object "$marketplace_json" \
  || die 'Codex marketplace listing was not valid machine-readable JSON.'
jq -e --arg name 'codex-session-control-local' --arg root "$clone_root" '
  if (.marketplaces | type) == "array" then
    [.marketplaces[] | select(.name == $name)] | length == 1 and .[0].root == $root
  else
    false
  end
' <<<"$marketplace_json" >/dev/null \
  || die 'Marketplace registration did not converge to the clone root.'

plugin_add_json="$(run_codex plugin add codex-session-control@codex-session-control-local --json)" \
  || die 'Codex plugin registration failed.'
require_single_json_object "$plugin_add_json" \
  || die 'Codex plugin registration was not valid machine-readable JSON.'
