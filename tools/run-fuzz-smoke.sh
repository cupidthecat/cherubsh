#!/usr/bin/env bash
# Run a bounded generated comparison against the configured Bash oracle.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CHERUBSH_BIN="${CHERUBSH:-${WORKSPACE_ROOT}/target/debug/cherubsh}"
ORACLE_BIN="${BASH_ORACLE_PATH:-${WORKSPACE_ROOT}/target/oracle/bash-5.3.15/bash}"
FUZZ_CASES="${FUZZ_CASES:-100}"
FUZZ_SEED="${FUZZ_SEED:-20260731}"
FUZZ_ARTIFACT_DIR="${FUZZ_ARTIFACT_DIR:-}"

if [[ ! -x "${CHERUBSH_BIN}" ]]; then
    echo "error: CherubSH binary is not executable: ${CHERUBSH_BIN}" >&2
    exit 2
fi
if [[ ! -x "${ORACLE_BIN}" ]]; then
    echo "error: Bash oracle is not executable: ${ORACLE_BIN}" >&2
    echo "hint: run tools/run-parity.sh first or set BASH_ORACLE_PATH" >&2
    exit 2
fi

ARTIFACT_ARGS=()
if [[ -n "${FUZZ_ARTIFACT_DIR}" ]]; then
    ARTIFACT_ARGS=(--artifact-dir "${FUZZ_ARTIFACT_DIR}")
fi

exec python3 "${SCRIPT_DIR}/fuzz-differential.py" \
    --cherub "${CHERUBSH_BIN}" \
    --bash "${ORACLE_BIN}" \
    --cases "${FUZZ_CASES}" \
    --seed "${FUZZ_SEED}" \
    "${ARTIFACT_ARGS[@]}"
