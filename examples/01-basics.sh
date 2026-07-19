#!/usr/bin/env cherubsh
set -euo pipefail

greet() {
    local name=${1:-world}
    printf 'hello %s\n' "$name"
}

greet "${1:-CherubSH}"

items=(alpha "two words" gamma)
printf 'array length: %s\n' "${#items[@]}"
for item in "${items[@]}"; do
    printf 'item: <%s>\n' "$item"
done

declare -A status=(
    [parser]=passing
    [expansion]=passing
    [upstream-tests]=passing
)

for key in "${!status[@]}"; do
    printf '%s=%s\n' "$key" "${status[$key]}"
done | sort

mode=${MODE:-demo}
case "$mode" in
    demo | dev)
        printf 'mode: %s\n' "$mode"
        ;;
    *)
        printf 'unknown mode: %s\n' "$mode" >&2
        exit 2
        ;;
esac
