#!/usr/bin/env bash
# Build Bash 5.3.15 as the default compatibility oracle for CherubSH.
#
# Re-running is a no-op when the existing binary reports 5.3.15.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BASH_VERSION=5.3.15
ORACLE_ROOT="${WS_ROOT}/target/oracle"
BASH_SRC="${BASH_SRC:-${ORACLE_ROOT}/bash-${BASH_VERSION}}"
REFERENCE_SRC="${WS_ROOT}/target/upstream/bash-${BASH_VERSION}"

if [[ ! -d "${BASH_SRC}" ]]; then
    if [[ ! -f "${REFERENCE_SRC}/.cherubsh-upstream-revision" ]]; then
        bash "${WS_ROOT}/tools/fetch-upstream.sh"
    fi
    echo ">> copying the verified Bash ${BASH_VERSION} source..."
    mkdir -p "${BASH_SRC}"
    (cd "${REFERENCE_SRC}" && tar --exclude .git -cf - .) | (cd "${BASH_SRC}" && tar -xf -)
fi

LINE_ENDING_MARKER="${BASH_SRC}/.cherubsh-lf-normalized"
if [[ ! -f "${LINE_ENDING_MARKER}" ]]; then
    while IFS= read -r -d '' file; do
        if grep -Iq . "${file}" && grep -q $'\r$' "${file}"; then
            sed -i 's/\r$//' "${file}"
        fi
    done < <(find "${BASH_SRC}" -type f -print0)
    touch "${LINE_ENDING_MARKER}"
fi

cd "${BASH_SRC}"

oracle_version_ok() {
    [[ -x ./bash ]] && ./bash --version 2>/dev/null | head -n1 | grep -Eq 'version 5\.3\.15'
}

if oracle_version_ok; then
    echo "OK: ${BASH_SRC}/bash (already built, 5.3.15)"
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

if ! ./bash --version 2>/dev/null | head -n1 | grep -Eq 'version 5\.3\.15'; then
    echo "error: built binary does not report version 5.3.15" >&2
    ./bash --version >&2 || true
    exit 1
fi

echo "OK: ${BASH_SRC}/bash"
