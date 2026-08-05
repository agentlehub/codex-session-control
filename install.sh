#!/bin/sh
set -eu

if [ ! -f "$0" ]; then
    printf '%s\n' "installer must be run from a regular file" >&2
    exit 1
fi

case "$0" in
    /*) installer_path=$0 ;;
    *)
        installer_directory=$(dirname "$0")
        installer_name=$(basename "$0")
        installer_path=$(CDPATH='' cd -P "$installer_directory" && pwd)/$installer_name
        ;;
esac

if [ ! -f "$installer_path" ]; then
    printf '%s\n' "installer must be run from a regular file" >&2
    exit 1
fi

case "$(uname -s)" in
    Linux) ;;
    *)
        printf '%s\n' "codex-session-control supports Linux only" >&2
        exit 1
        ;;
esac

machine=$(uname -m)
case "$machine" in
    x86_64) target=x86_64-unknown-linux-gnu ;;
    aarch64) target=aarch64-unknown-linux-gnu ;;
    *)
        printf '%s\n' "unsupported architecture: $machine" >&2
        exit 1
        ;;
esac

command -v curl >/dev/null 2>&1
command -v sha256sum >/dev/null 2>&1

umask 077
temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/codex-session-control.XXXXXX") || {
    printf '%s\n' "Bootstrap failed before candidate verification." >&2
    printf 'Retry: %s\n' "$installer_path" >&2
    exit 1
}
chmod 0700 "$temporary_directory"

verified=0
candidate=
cleanup() {
    status=$?
    trap - EXIT
    if [ "$status" -eq 0 ]; then
        rm -rf "$temporary_directory"
        exit 0
    fi
    if [ "$verified" -eq 1 ]; then
        printf '%s\n' "Verified candidate preserved after setup failure." >&2
        printf 'Retry: %s setup\n' "$candidate" >&2
        printf 'Cleanup: rm -rf %s\n' "$temporary_directory" >&2
    else
        rm -rf "$temporary_directory"
        printf '%s\n' "Bootstrap failed before candidate verification." >&2
        printf 'Retry: %s\n' "$installer_path" >&2
    fi
    exit "$status"
}
trap cleanup EXIT

repository=agentlehub/codex-session-control
latest_url=https://api.github.com/repos/$repository/releases/latest
latest_json=$(curl \
    --fail \
    --location \
    --connect-timeout 10 \
    --speed-time 30 \
    --speed-limit 1 \
    "$latest_url")
tag=$(printf '%s\n' "$latest_json" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
case "$tag" in
    "" | *[!A-Za-z0-9._-]*) exit 1 ;;
esac

asset=codex-session-control-$target
candidate=$temporary_directory/$asset
checksums=$temporary_directory/SHA256SUMS
download_root=https://github.com/$repository/releases/download/$tag
curl \
    --fail \
    --location \
    --connect-timeout 10 \
    --speed-time 30 \
    --speed-limit 1 \
    --output "$candidate" \
    "$download_root/$asset"
curl \
    --fail \
    --location \
    --connect-timeout 10 \
    --speed-time 30 \
    --speed-limit 1 \
    --output "$checksums" \
    "$download_root/SHA256SUMS"

checksum_line=$(awk -v asset="$asset" '$2 == asset { print }' "$checksums")
checksum_count=$(printf '%s\n' "$checksum_line" | awk 'NF { count += 1 } END { print count + 0 }')
[ "$checksum_count" -eq 1 ]
checksum=$(printf '%s\n' "$checksum_line" | awk '{ print $1 }')
[ "${#checksum}" -eq 64 ]
case "$checksum" in
    *[!0-9a-f]*) exit 1 ;;
esac
[ "$checksum_line" = "$checksum  $asset" ]
printf '%s  %s\n' "$checksum" "$asset" >"$temporary_directory/SHA256SUMS.selected"

(
    cd "$temporary_directory"
    sha256sum --check SHA256SUMS.selected
)
verified=1
chmod 0700 "$candidate"
"$candidate" setup
