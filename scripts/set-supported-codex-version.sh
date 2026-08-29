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
readme="$repository_root/README.md"
marker='<!-- generated: supported-codex-version -->'
replacement="- Native app-server protocol validated against Codex \`$version\`. $marker"

marker_count="$(awk -v marker="$marker" '
  {
    line = $0
    while ((position = index(line, marker)) != 0) {
      count++
      line = substr(line, position + length(marker))
    }
  }
  END { print count + 0 }
' "$readme")"
if [ "$marker_count" -ne 1 ]; then
  printf 'README.md must contain exactly one %s marker\n' "$marker" >&2
  exit 1
fi

transaction_dir=
version_stage=
readme_stage=
version_backup=
readme_backup=
version_replaced=false
readme_replaced=false
committed=false
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ "$committed" = false ]; then
    if [ "$version_replaced" = true ]; then
      mv -- "$version_backup" "$version_file" || status=$?
    fi
    if [ "$readme_replaced" = true ]; then
      mv -- "$readme_backup" "$readme" || status=$?
    fi
  fi
  if [ -n "$transaction_dir" ]; then
    rm -f -- "$version_stage" "$readme_stage" "$version_backup" "$readme_backup" || status=$?
    rmdir -- "$transaction_dir" || status=$?
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 128' HUP INT TERM

transaction_dir="$(mktemp -d "$repository_root/.set-supported-version.XXXXXX")"
version_stage="$transaction_dir/version"
readme_stage="$transaction_dir/README.md"
version_backup="$transaction_dir/version.backup"
readme_backup="$transaction_dir/README.md.backup"

cp --preserve=mode -- "$version_file" "$version_backup"
cp --preserve=mode -- "$readme" "$readme_backup"
printf '%s\n' "$version" >"$version_stage"
awk -v marker="$marker" -v replacement="$replacement" '
  index($0, marker) { print replacement; next }
  { print }
' "$readme" >"$readme_stage"
chmod --reference="$version_file" "$version_stage"
chmod --reference="$readme" "$readme_stage"

version_replaced=true
mv -- "$version_stage" "$version_file"
readme_replaced=true
mv -- "$readme_stage" "$readme"
committed=true
printf 'Supported Codex version is now %s.\n' "$version"
