#!/usr/bin/env bash
# Provision the pinned Bash oracle and run the ordinary Rust workspace tests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ORACLE_VERSION=5.3.15
DEFAULT_ORACLE="${WS_ROOT}/target/oracle/bash-${ORACLE_VERSION}/bash"
CARGO_BIN="${CARGO_BIN:-cargo}"

oracle_version_ok() {
    local path=$1
    [[ -x "${path}" ]] &&
        "${path}" --version 2>/dev/null | head -n1 | grep -Eq 'version 5\.3\.15'
}

if [[ -n ${BASH_ORACLE_PATH:-} ]]; then
    ORACLE="${BASH_ORACLE_PATH}"
    if ! oracle_version_ok "${ORACLE}"; then
        echo "error: BASH_ORACLE_PATH must be an executable GNU Bash 5.3.15 binary: ${ORACLE}" >&2
        exit 2
    fi
else
    ORACLE="${DEFAULT_ORACLE}"
    if ! oracle_version_ok "${ORACLE}"; then
        echo ">> Bash ${ORACLE_VERSION} oracle missing or invalid; building it..."
        bash "${WS_ROOT}/oracle/build-bash-5.3.15.sh"
    fi
    if ! oracle_version_ok "${ORACLE}"; then
        echo "error: Bash ${ORACLE_VERSION} oracle was not produced at ${ORACLE}" >&2
        exit 1
    fi
fi

export BASH_ORACLE_VERSION="${ORACLE_VERSION}"
export BASH_ORACLE_PATH="${ORACLE}"

cd "${WS_ROOT}"
exec "${CARGO_BIN}" test --workspace --locked "$@"
