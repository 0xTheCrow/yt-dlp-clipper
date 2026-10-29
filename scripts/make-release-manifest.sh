#!/usr/bin/env bash
# Write a release's latest.json: for each platform key, the file name and SHA-256
# of that platform's self-update payload. The app reads this to find its update
# instead of inferring it from file names.
#
# Usage: make-release-manifest.sh <output> <platform-key> '<payload-glob>' [...]
# Platform keys are `<os>-<arch>` as Rust's std::env::consts spells them
# (linux-x86_64, windows-x86_64, macos-aarch64). Each glob must match exactly
# one file.
set -euo pipefail

if (( $# < 3 || $# % 2 == 0 )); then
    echo "usage: $0 <output> <platform-key> '<payload-glob>' [...]" >&2
    exit 1
fi
output="$1"
shift

shopt -s nullglob
entries=()
while (( $# )); do
    platform_key="$1"
    payload_glob="$2"
    shift 2
    # shellcheck disable=SC2206
    matches=( $payload_glob )
    if (( ${#matches[@]} != 1 )); then
        echo "error: $platform_key needs exactly one file matching $payload_glob," \
             "found ${#matches[@]}" >&2
        exit 1
    fi
    payload="${matches[0]}"
    sha256="$(sha256sum "$payload" | cut -d' ' -f1)"
    entries+=("    \"$platform_key\": { \"file\": \"$(basename "$payload")\", \"sha256\": \"$sha256\" }")
done

{
    echo '{'
    echo '  "platforms": {'
    for i in "${!entries[@]}"; do
        separator=','
        (( i == ${#entries[@]} - 1 )) && separator=''
        echo "${entries[$i]}$separator"
    done
    echo '  }'
    echo '}'
} > "$output"
echo "==> Wrote $output"
