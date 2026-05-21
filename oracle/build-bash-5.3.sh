#!/usr/bin/env bash
# Build bash-5.3 to serve as the default strict-parity oracle for CherubSH.
#
# Idempotent: re-running is a no-op if ${BASH_SRC}/bash already reports 5.3.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BASH_VERSION=5.3
ORACLE_ROOT="${WS_ROOT}/target/oracle"
LOCAL_SRC="${WS_ROOT}/../bash-5.3"
BASH_SRC="${BASH_SRC:-${ORACLE_ROOT}/bash-${BASH_VERSION}}"
BASH_TARBALL="${ORACLE_ROOT}/bash-${BASH_VERSION}.tar.gz"
BASH_URL="${BASH_URL:-https://ftp.gnu.org/gnu/bash/bash-${BASH_VERSION}.tar.gz}"

if [[ ! -d "${BASH_SRC}" ]]; then
    mkdir -p "${ORACLE_ROOT}"
    if [[ -d "${LOCAL_SRC}" ]]; then
        echo ">> copying local bash-${BASH_VERSION} source..."
        mkdir -p "${BASH_SRC}"
        (cd "${LOCAL_SRC}" && tar --exclude .git -cf - .) | (cd "${BASH_SRC}" && tar -xf -)
    else
        if [[ ! -f "${BASH_TARBALL}" ]]; then
            echo ">> downloading bash-${BASH_VERSION} oracle source..."
            if command -v curl >/dev/null 2>&1; then
                curl -L "${BASH_URL}" -o "${BASH_TARBALL}"
            elif command -v wget >/dev/null 2>&1; then
                wget -O "${BASH_TARBALL}" "${BASH_URL}"
            else
                echo "error: bash ${BASH_VERSION} source not found at ${BASH_SRC}" >&2
                echo "       install curl/wget, set BASH_SRC=/path/to/bash-${BASH_VERSION}, or set BASH_53_PATH=/path/to/bash" >&2
                exit 1
            fi
        fi
        echo ">> extracting bash-${BASH_VERSION} oracle source..."
        tar -xzf "${BASH_TARBALL}" -C "${ORACLE_ROOT}"
    fi
fi

cd "${BASH_SRC}"

oracle_version_ok() {
    [[ -x ./bash ]] && ./bash --version 2>/dev/null | head -n1 | grep -Eq 'version 5\.3(\.0)?'
}

if oracle_version_ok; then
    echo "OK: ${BASH_SRC}/bash (already built, 5.3)"
    exit 0
fi

if [[ ! -f Makefile ]] || [[ configure -nt Makefile ]]; then
    ./configure --without-bash-malloc
fi

CFLAGS="${CFLAGS:-"-O0 -g -std=gnu99 -fcommon -Wno-error"}"

if ! make -j"$(nproc)" CFLAGS="${CFLAGS}" CFLAGS_FOR_BUILD="${CFLAGS}"; then
    cat >&2 <<'HINT'

build failed. common remediation:
  - install bison, autoconf, texinfo, makeinfo via your distro
  - if a specific .c file is rejected, add a targeted -Wno-* flag through CFLAGS
HINT
    exit 1
fi

if ! make recho zecho printenv xcase CFLAGS_FOR_BUILD="${CFLAGS}"; then
    echo "error: failed to build bash test support helpers" >&2
    exit 1
fi
mkdir -p tests
cp recho zecho printenv xcase tests/

if ! ./bash --version 2>/dev/null | head -n1 | grep -Eq 'version 5\.3(\.0)?'; then
    echo "error: built binary does not report version 5.3" >&2
    ./bash --version >&2 || true
    exit 1
fi

echo "OK: ${BASH_SRC}/bash"
