#!/usr/bin/env cherubsh
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
log_file=${1:-"$script_dir/data/access.log"}

declare -A requests_by_status=()
declare -A requests_by_path=()
declare -i request_count=0
declare -i total_duration=0

while read -r timestamp status path duration_ms; do
    [[ -z ${timestamp:-} || $timestamp == \#* ]] && continue
    if [[ ! $status =~ ^[0-9]{3}$ || ! $duration_ms =~ ^[0-9]+$ ]]; then
        printf 'invalid log row: %s %s %s %s\n' \
            "$timestamp" "$status" "$path" "$duration_ms" >&2
        exit 2
    fi
    ((requests_by_status[$status] += 1))
    ((requests_by_path[$path] += 1))
    ((request_count += 1))
    ((total_duration += duration_ms))
done <"$log_file"

printf 'requests: %d\n' "$request_count"
if ((request_count > 0)); then
    printf 'average latency: %d ms\n' "$((total_duration / request_count))"
fi

printf 'status counts:\n'
while IFS= read -r status; do
    printf '  %s: %d\n' "$status" "${requests_by_status[$status]}"
done < <(printf '%s\n' "${!requests_by_status[@]}" | sort)

printf 'path counts:\n'
while IFS= read -r path; do
    printf '  %s: %d\n' "$path" "${requests_by_path[$path]}"
done < <(printf '%s\n' "${!requests_by_path[@]}" | sort)
