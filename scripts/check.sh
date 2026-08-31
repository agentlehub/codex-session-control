#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir/.."

missing_tools=()
for tool in bash sh cargo rustc shellcheck jq actionlint; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing_tools+=("$tool")
  fi
done

if [ "${#missing_tools[@]}" -gt 0 ]; then
  printf '%s\n' 'Missing required tools:' >&2
  printf '  %s\n' "${missing_tools[@]}" >&2
  exit 1
fi

preflight_failed=0
printf '%s\n' 'Checking Rust formatter...'
if ! cargo fmt --version; then
  printf '%s\n' 'Rust formatter is unavailable: cargo fmt --version failed.' >&2
  preflight_failed=1
fi
printf '%s\n' 'Checking Rust Clippy...'
if ! cargo clippy --version; then
  printf '%s\n' 'Rust Clippy is unavailable: cargo clippy --version failed.' >&2
  preflight_failed=1
fi
if [ "$preflight_failed" -ne 0 ]; then
  exit 1
fi

printf '%s\n' 'Checking actionlint version...'
if ! actionlint -version | grep -x '1\.7\.12'; then
  printf '%s\n' 'actionlint 1.7.12 is required.' >&2
  exit 1
fi

check_tmp="$(mktemp --directory "${HOME:?HOME must be set}/.csc.XXXXXX")"
cleanup_check_tmp() {
  case "${check_tmp:-}" in
    "$HOME"/.csc.*) rm -rf -- "$check_tmp" ;;
  esac
}
trap cleanup_check_tmp EXIT

if [ "$(stat --format=%F "$check_tmp")" != directory ] ||
  [ "$(stat --format=%u "$check_tmp")" != "$(id -u)" ] ||
  [ "$(stat --format=%a "$check_tmp")" != 700 ]; then
  printf '%s\n' 'Failed to create a private owner-only test directory.' >&2
  exit 1
fi
export TMPDIR="$check_tmp"

printf '%s\n' 'Checking Rust formatting...'
cargo fmt --all -- --check

printf '%s\n' 'Checking shell scripts...'
shellcheck scripts/check.sh scripts/set-supported-codex-version.sh \
  scripts/install-local-plugin.sh

printf '%s\n' 'Checking shell syntax...'
bash -n scripts/check.sh scripts/set-supported-codex-version.sh \
  scripts/install-local-plugin.sh

printf '%s\n' 'Checking workflow syntax...'
actionlint .github/workflows/ci.yml

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
