#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -ne 1 ]; then
  printf 'Usage: %s VERSION\n' "${0##*/}" >&2
  exit 2
fi

version="$1"
LC_ALL=C
numeric_identifier='(0|[1-9][0-9]*)'
prerelease_identifier="($numeric_identifier|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)"
build_identifier='[0-9A-Za-z-]+'
semver_pattern="^$numeric_identifier\\.$numeric_identifier\\.$numeric_identifier(-$prerelease_identifier(\\.$prerelease_identifier)*)?(\\+$build_identifier(\\.$build_identifier)*)?$"
if ! [[ "$version" =~ $semver_pattern ]]; then
  printf 'VERSION must be canonical SemVer: %s\n' "$version" >&2
  exit 2
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd -- "$script_dir/.." && pwd)"
version_file="$repository_root/supported-codex-version.txt"
stage=
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$stage" ]; then
    rm -f -- "$stage" || status=$?
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 128' HUP INT TERM

stage="$(mktemp "$repository_root/.set-supported-version.XXXXXX")"
printf '%s\n' "$version" >"$stage"
chmod --reference="$version_file" "$stage"
mv -- "$stage" "$version_file"
stage=
printf 'Supported Codex version is now %s.\n' "$version"
