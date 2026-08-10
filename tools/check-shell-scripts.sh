#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

if ! command -v shellcheck >/dev/null 2>&1; then
    echo "error: ShellCheck is required to lint maintenance scripts" >&2
    exit 2
fi

scripts=()
while IFS= read -r -d '' script; do
    scripts+=("${script}")
done < <(git -C "${WS_ROOT}" ls-files -z -- 'tools/*.sh' 'oracle/*.sh')

if ((${#scripts[@]} == 0)); then
    echo "error: no tracked maintenance scripts found" >&2
    exit 1
fi

cd "${WS_ROOT}"
shellcheck --severity=warning -- "${scripts[@]}"
printf 'ShellCheck passed for %d maintenance scripts.\n' "${#scripts[@]}"
