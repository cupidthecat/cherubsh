#!/usr/bin/env bash
# CI driver: build the bash-5.2.21 oracle if needed, then run the
# full parity sweep (fixture-level + upstream test suite). Tallies results
# and exits non-zero on any unexpected outcome.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ORACLE="${BASH_521_PATH:-${WS_ROOT}/target/oracle/bash-5.2.21/bash}"

cd "${WS_ROOT}"

if ! [[ -x "${ORACLE}" ]] || ! "${ORACLE}" --version 2>/dev/null | head -n1 | grep -q 'version 5\.2\.21'; then
    echo ">> oracle missing or wrong version; building bash-5.2.21..."
    bash "${WS_ROOT}/oracle/build-bash-5.2.21.sh"
fi

export BASH_521_PATH="${ORACLE}"
export BASH_521_TESTS_DIR="${BASH_521_TESTS_DIR:-${WS_ROOT}/vendor/bash-5.2.21/tests}"
export RUN_PARITY_TESTS=1
export RUN_UPSTREAM_PARITY=1
export UPSTREAM_PARITY_REPORT_DIR="${UPSTREAM_PARITY_REPORT_DIR:-${WS_ROOT}/target/parity/upstream}"
# Avoid host nss-systemd/userdb stalls in upstream tests that invoke `ls -l`.
export SYSTEMD_NSS_DYNAMIC_BYPASS="${SYSTEMD_NSS_DYNAMIC_BYPASS:-1}"
export SYSTEMD_BYPASS_USERDB="${SYSTEMD_BYPASS_USERDB:-1}"

LOG="${WS_ROOT}/parity.log"
echo ">> running cargo test --workspace (log: ${LOG})"
set +e
cargo test --workspace -- --nocapture 2>&1 | tee "${LOG}"
CARGO_RC=${PIPESTATUS[0]}
set -e

# Tally upstream parity outcomes from the log.
PASS=$(grep -cE '^ PASS '         "${LOG}" || true)
FAIL=$(grep -cE '^ FAIL '         "${LOG}" || true)
TIMEOUT=$(grep -cE '^TIMEOUT '    "${LOG}" || true)
XFAIL=$(grep -cE '^xfail '        "${LOG}" || true)
XPASS=$(grep -cE '^XPASS '        "${LOG}" || true)

echo
echo ">> upstream parity: PASS=${PASS} FAIL=${FAIL} TIMEOUT=${TIMEOUT} XFAIL=${XFAIL} XPASS=${XPASS}"
if [[ -f "${UPSTREAM_PARITY_REPORT_DIR}/report.tsv" ]]; then
    echo ">> upstream report: ${UPSTREAM_PARITY_REPORT_DIR}/report.tsv"
fi
echo ">> cargo test exit: ${CARGO_RC}"

if [[ ${CARGO_RC} -ne 0 ]] || [[ ${FAIL} -ne 0 ]] || [[ ${TIMEOUT} -ne 0 ]] || [[ ${XPASS} -ne 0 ]]; then
    echo ">> FAIL: parity sweep has unexpected outcomes" >&2
    exit 1
fi

echo ">> OK"
