#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir/.."

mapfile -d '' -t shell_scripts < <(git ls-files -z -- '*.sh')

printf '%s\n' 'Checking Rust formatting...'
cargo fmt --all -- --check

printf '%s\n' 'Checking shell scripts...'
shellcheck "${shell_scripts[@]}"

printf '%s\n' 'Checking shell syntax...'
bash -n "${shell_scripts[@]}"

printf '%s\n' 'Checking live-runner cleanup...'
if ! cleanup_smoke_output="$(
  timeout 20s scripts/ci/live-all-tools-proof.sh --self-test 2>&1
)"; then
  printf '%s\n' 'Live-runner cleanup smoke failed.' >&2
  exit 1
fi
if [[ "$cleanup_smoke_output" != self_test_status=0 ]]; then
  printf '%s\n' 'Live-runner cleanup smoke emitted unexpected output.' >&2
  exit 1
fi
printf '%s\n' "$cleanup_smoke_output"

printf '%s\n' 'Checking workflow syntax...'
actionlint

printf '%s\n' 'Checking marketplace manifest JSON...'
jq empty .agents/plugins/marketplace.json

printf '%s\n' 'Checking plugin manifest JSON...'
jq empty plugins/codex-session-control/.codex-plugin/plugin.json

printf '%s\n' 'Checking MCP manifest JSON...'
jq empty plugins/codex-session-control/.mcp.json

printf '%s\n' 'Checking Rust lints...'
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

printf '%s\n' 'Running Rust tests...'
cargo test --workspace --all-features --locked

printf '%s\n' 'All checks passed.'
