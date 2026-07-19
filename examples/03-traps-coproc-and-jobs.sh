#!/usr/bin/env cherubsh
set -euo pipefail

tmp=${TMPDIR:-/tmp}/cherubsh-trap-$$.txt
trap 'rm -f "$tmp"; printf "cleanup complete\n"' EXIT

printf 'background work\n' >"$tmp" &
writer=$!
wait "$writer"
printf 'waited for pid %s: %s\n' "$writer" "$(cat "$tmp")"

coproc ECHOER {
    while IFS= read -r line; do
        printf 'coproc saw: %s\n' "$line"
    done
}

printf 'ping\n' >&"${ECHOER[1]}"
IFS= read -r reply <&"${ECHOER[0]}"
printf '%s\n' "$reply"

exec {ECHOER[1]}>&-
wait "$ECHOER_PID" 2>/dev/null || true
