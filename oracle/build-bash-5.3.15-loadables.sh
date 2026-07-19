#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BASH_ROOT="${BASH_5315_ROOT:-${WORKSPACE_ROOT}/target/oracle/bash-5.3.15}"

if [[ ! -x "${BASH_ROOT}/bash" ]]; then
    bash "${SCRIPT_DIR}/build-bash-5.3.15.sh"
fi

make -C "${BASH_ROOT}/examples/loadables" -j"${JOBS:-2}" all
printf 'Bash 5.3.15 loadable builtins: %s\n' "${BASH_ROOT}/examples/loadables"
