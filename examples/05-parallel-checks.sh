#!/usr/bin/env cherubsh
set -uo pipefail

declare -A check_name=()

start_check() {
    local name=$1 delay=$2 status=$3
    (
        sleep "$delay"
        exit "$status"
    ) &
    check_name[$!]=$name
}

start_check format 0.01 0
start_check unit-tests 0.03 0
start_check integration-tests 0.05 1

remaining=${#check_name[@]}
failures=0
while ((remaining > 0)); do
    finished_pid=
    if wait -n -p finished_pid; then
        status=0
    else
        status=$?
    fi
    name=${check_name[$finished_pid]}
    printf '%-20s status=%d\n' "$name" "$status"
    ((status == 0)) || ((failures += 1))
    unset 'check_name[$finished_pid]'
    ((remaining -= 1))
done

printf 'failed checks: %d\n' "$failures"
((failures == 0))
