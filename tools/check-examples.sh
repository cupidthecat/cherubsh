#!/usr/bin/env bash
# Run the repository's non-interactive examples with CherubSH.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CHERUBSH_BIN="${CHERUBSH:-${WORKSPACE_ROOT}/target/debug/cherubsh}"
RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cherubsh-examples.XXXXXX")"

cleanup() {
    rm -rf "${RUN_DIR}"
}
trap cleanup EXIT

if [[ ! -x "${CHERUBSH_BIN}" ]]; then
    echo "error: CherubSH binary is not executable: ${CHERUBSH_BIN}" >&2
    echo "hint: set CHERUBSH=/path/to/cherubsh or run cargo build -p cherubsh" >&2
    exit 2
fi

run_example() {
    local script_name="$1"
    local expected_status="$2"
    local output_file="${RUN_DIR}/${script_name}.out"
    local status

    if (
        cd "${RUN_DIR}"
        "${CHERUBSH_BIN}" --norc "${WORKSPACE_ROOT}/examples/${script_name}"
    ) >"${output_file}" 2>&1; then
        status=0
    else
        status=$?
    fi

    if [[ "${status}" != "${expected_status}" ]]; then
        echo "error: ${script_name} exited ${status}, expected ${expected_status}" >&2
        sed 's/^/  /' "${output_file}" >&2
        exit 1
    fi
}

assert_output_contains() {
    local script_name="$1"
    local expected_text="$2"
    local output_file="${RUN_DIR}/${script_name}.out"

    if ! grep -Fqx -- "${expected_text}" "${output_file}"; then
        echo "error: ${script_name} did not print: ${expected_text}" >&2
        sed 's/^/  /' "${output_file}" >&2
        exit 1
    fi
}

run_example "01-basics.sh" 0
assert_output_contains "01-basics.sh" "hello CherubSH"
assert_output_contains "01-basics.sh" "array length: 3"

run_example "02-expansion-and-redirection.sh" 0
assert_output_contains "02-expansion-and-redirection.sh" "  process substitution matched"

run_example "03-traps-coproc-and-jobs.sh" 0
assert_output_contains "03-traps-coproc-and-jobs.sh" "cleanup complete"

run_example "04-log-summary.sh" 0
assert_output_contains "04-log-summary.sh" "requests: 6"

run_example "05-parallel-checks.sh" 1
assert_output_contains "05-parallel-checks.sh" "failed checks: 1"

run_example "06-completion-and-history.sh" 0
assert_output_contains "06-completion-and-history.sh" "completion candidates for \"t\":"
assert_output_contains "06-completion-and-history.sh" "cherub-task test"

printf 'examples: ok\n'
