#!/usr/bin/env bash
# Build the pinned oracles and run every compatibility suite.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ORACLE_VERSION="${BASH_ORACLE_VERSION:-5.3.15}"
case "${ORACLE_VERSION}" in
    5.2|5.2.21)
        ORACLE_VERSION="5.2.21"
        ORACLE="${BASH_ORACLE_PATH:-${BASH_521_PATH:-${WS_ROOT}/target/oracle/bash-5.2.21/bash}}"
        ORACLE_BUILDER="${WS_ROOT}/oracle/build-bash-5.2.21.sh"
        VERSION_RE='version 5\.2\.21'
        DEFAULT_TESTS_DIR="${WS_ROOT}/target/oracle/bash-5.2.21/tests"
        ;;
    5.3|5.3.0|5.3.15)
        ORACLE_VERSION="5.3.15"
        ORACLE="${BASH_ORACLE_PATH:-${BASH_5315_PATH:-${BASH_53_PATH:-${WS_ROOT}/target/oracle/bash-5.3.15/bash}}}"
        ORACLE_BUILDER="${WS_ROOT}/oracle/build-bash-5.3.15.sh"
        VERSION_RE='version 5\.3\.15'
        DEFAULT_TESTS_DIR="${WS_ROOT}/target/oracle/bash-5.3.15/tests"
        ;;
    *)
        echo "error: unsupported BASH_ORACLE_VERSION=${ORACLE_VERSION}" >&2
        exit 2
        ;;
esac

cd "${WS_ROOT}"

if ! [[ -x "${ORACLE}" ]] || ! "${ORACLE}" --version 2>/dev/null | head -n1 | grep -Eq "${VERSION_RE}"; then
    echo ">> oracle missing or wrong version; building bash-${ORACLE_VERSION}..."
    bash "${ORACLE_BUILDER}"
fi

export BASH_ORACLE_VERSION="${ORACLE_VERSION}"
export BASH_ORACLE_PATH="${ORACLE}"
if [[ "${ORACLE_VERSION}" == "5.2.21" ]]; then
    export BASH_521_PATH="${ORACLE}"
    export BASH_521_TESTS_DIR="${BASH_TESTS_DIR:-${BASH_521_TESTS_DIR:-${DEFAULT_TESTS_DIR}}}"
else
    bash "${WS_ROOT}/oracle/build-bash-5.3.15-loadables.sh"
    export BASH_5315_PATH="${ORACLE}"
    export BASH_53_PATH="${ORACLE}"
    export BASH_5315_TESTS_DIR="${BASH_TESTS_DIR:-${BASH_5315_TESTS_DIR:-${BASH_53_TESTS_DIR:-${DEFAULT_TESTS_DIR}}}}"
    export BASH_53_TESTS_DIR="${BASH_5315_TESTS_DIR}"
fi
export BASH_TESTS_DIR="${BASH_TESTS_DIR:-${DEFAULT_TESTS_DIR}}"
export RUN_PARITY_TESTS=1
export RUN_UPSTREAM_PARITY=1
export RUN_LOADABLE_PARITY=1
export RUN_OILS_PARITY=1
export UPSTREAM_PARITY_REPORT_DIR="${UPSTREAM_PARITY_REPORT_DIR:-${WS_ROOT}/target/parity/upstream}"
export OILS_PARITY_REPORT_DIR="${OILS_PARITY_REPORT_DIR:-${WS_ROOT}/target/parity/oils}"
if [[ "${RUN_BRUSH_PARITY:-}" == "1" ]]; then
    export BRUSH_PARITY_REPORT_DIR="${BRUSH_PARITY_REPORT_DIR:-${WS_ROOT}/target/parity/brush}"
fi
# Avoid host nss-systemd/userdb stalls in upstream tests that invoke `ls -l`.
export SYSTEMD_NSS_DYNAMIC_BYPASS="${SYSTEMD_NSS_DYNAMIC_BYPASS:-1}"
export SYSTEMD_BYPASS_USERDB="${SYSTEMD_BYPASS_USERDB:-1}"

LOG="${WS_ROOT}/parity.log"
echo ">> running cargo test --workspace (log: ${LOG})"
set +e
cargo test --workspace --locked -- --nocapture 2>&1 | tee "${LOG}"
CARGO_RC=${PIPESTATUS[0]}
set -e

READLINE_LOG="${WS_ROOT}/target/parity/readline.log"
mkdir -p "$(dirname "${READLINE_LOG}")"
echo ">> running GNU Readline 8.3 parity (log: ${READLINE_LOG})"
set +e
bash "${WS_ROOT}/tools/run-readline-parity.sh" 2>&1 | tee "${READLINE_LOG}"
READLINE_RC=${PIPESTATUS[0]}
set -e

# Tally upstream parity outcomes from the log.
PASS=$(grep -cE '^ PASS '         "${LOG}" || true)
FAIL=$(grep -cE '^ FAIL '         "${LOG}" || true)
TIMEOUT=$(grep -cE '^TIMEOUT '    "${LOG}" || true)
XFAIL=$(grep -cE '^xfail '        "${LOG}" || true)
XPASS=$(grep -cE '^XPASS '        "${LOG}" || true)

echo
echo ">> Bash comparisons: PASS=${PASS} FAIL=${FAIL} TIMEOUT=${TIMEOUT} XFAIL=${XFAIL} XPASS=${XPASS}"
if [[ -f "${UPSTREAM_PARITY_REPORT_DIR}/report.tsv" ]]; then
    echo ">> upstream report: ${UPSTREAM_PARITY_REPORT_DIR}/report.tsv"
fi
if [[ "${RUN_BRUSH_PARITY:-}" == "1" ]] && [[ -f "${BRUSH_PARITY_REPORT_DIR}/report.tsv" ]]; then
    BRUSH_PASS=$(awk -F '\t' '$1 == "PASS" { count++ } END { print count + 0 }' "${BRUSH_PARITY_REPORT_DIR}/report.tsv")
    BRUSH_FAIL=$(awk -F '\t' '$1 == "FAIL" { count++ } END { print count + 0 }' "${BRUSH_PARITY_REPORT_DIR}/report.tsv")
    BRUSH_SKIP=$(awk -F '\t' '$1 == "SKIP" { count++ } END { print count + 0 }' "${BRUSH_PARITY_REPORT_DIR}/report.tsv")
    echo ">> brush parity: PASS=${BRUSH_PASS} FAIL=${BRUSH_FAIL} SKIP=${BRUSH_SKIP}"
    echo ">> brush report: ${BRUSH_PARITY_REPORT_DIR}/report.tsv"
fi
OILS_PASS=0
OILS_KNOWN=0
OILS_FAIL=0
OILS_DRIFT=0
OILS_XPASS=0
OILS_STALE=0
if [[ -f "${OILS_PARITY_REPORT_DIR}/report.tsv" ]]; then
    OILS_PASS=$(awk -F '\t' '$1 == "PASS" { count++ } END { print count + 0 }' "${OILS_PARITY_REPORT_DIR}/report.tsv")
    OILS_KNOWN=$(awk -F '\t' '$1 == "KNOWN" { count++ } END { print count + 0 }' "${OILS_PARITY_REPORT_DIR}/report.tsv")
    OILS_FAIL=$(awk -F '\t' '$1 == "FAIL" { count++ } END { print count + 0 }' "${OILS_PARITY_REPORT_DIR}/report.tsv")
    OILS_DRIFT=$(awk -F '\t' '$1 == "DRIFT" { count++ } END { print count + 0 }' "${OILS_PARITY_REPORT_DIR}/report.tsv")
    OILS_XPASS=$(awk -F '\t' '$1 == "XPASS" { count++ } END { print count + 0 }' "${OILS_PARITY_REPORT_DIR}/report.tsv")
    OILS_STALE=$(awk -F '\t' '$1 == "STALE" { count++ } END { print count + 0 }' "${OILS_PARITY_REPORT_DIR}/report.tsv")
    echo ">> Oils parity: PASS=${OILS_PASS} KNOWN=${OILS_KNOWN} FAIL=${OILS_FAIL} DRIFT=${OILS_DRIFT} XPASS=${OILS_XPASS} STALE=${OILS_STALE}"
    echo ">> Oils report: ${OILS_PARITY_REPORT_DIR}/report.tsv"
fi
echo ">> cargo test exit: ${CARGO_RC}"
echo ">> readline parity exit: ${READLINE_RC}"

if [[ ${CARGO_RC} -ne 0 ]] || [[ ${READLINE_RC} -ne 0 ]] || [[ ${FAIL} -ne 0 ]] || [[ ${TIMEOUT} -ne 0 ]] || [[ ${XPASS} -ne 0 ]] || [[ ${OILS_FAIL} -ne 0 ]] || [[ ${OILS_DRIFT} -ne 0 ]] || [[ ${OILS_XPASS} -ne 0 ]] || [[ ${OILS_STALE} -ne 0 ]]; then
    echo ">> FAIL: parity sweep has unexpected outcomes" >&2
    exit 1
fi

echo ">> OK"
