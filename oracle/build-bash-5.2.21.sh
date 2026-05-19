#!/usr/bin/env bash
# Build bash-5.2.21 to serve as the strict-parity oracle for CherubSH. Modern
# gcc (>=14) rejects bash 5.2.21's K&R-era declarations and tentative
# definitions, so we force legacy permissive flags.
#
# Idempotent: re-running is a no-op if ${BASH_SRC}/bash already reports 5.2.21.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BASH_VERSION=5.2.21
ORACLE_ROOT="${WS_ROOT}/target/oracle"
BASH_SRC="${BASH_SRC:-${ORACLE_ROOT}/bash-${BASH_VERSION}}"
BASH_TARBALL="${ORACLE_ROOT}/bash-${BASH_VERSION}.tar.gz"
BASH_URL="${BASH_URL:-https://ftp.gnu.org/gnu/bash/bash-${BASH_VERSION}.tar.gz}"

if [[ ! -d "${BASH_SRC}" ]]; then
    mkdir -p "${ORACLE_ROOT}"
    if [[ ! -f "${BASH_TARBALL}" ]]; then
        echo ">> downloading bash-${BASH_VERSION} oracle source..."
        if command -v curl >/dev/null 2>&1; then
            curl -L "${BASH_URL}" -o "${BASH_TARBALL}"
        elif command -v wget >/dev/null 2>&1; then
            wget -O "${BASH_TARBALL}" "${BASH_URL}"
        else
            echo "error: bash ${BASH_VERSION} source not found at ${BASH_SRC}" >&2
            echo "       install curl/wget, set BASH_SRC=/path/to/bash-${BASH_VERSION}, or set BASH_521_PATH=/path/to/bash" >&2
            exit 1
        fi
    fi
    echo ">> extracting bash-${BASH_VERSION} oracle source..."
    tar -xzf "${BASH_TARBALL}" -C "${ORACLE_ROOT}"
fi

cd "${BASH_SRC}"

if [[ -x ./bash ]] && ./bash --version 2>/dev/null | head -n1 | grep -q 'version 5\.2\.21'; then
    echo "OK: ${BASH_SRC}/bash (already built, 5.2.21)"
    exit 0
fi

# Configure if needed. --without-bash-malloc avoids the bundled malloc which
# is brittle on modern glibc; we want a plain libc-malloc oracle.
if [[ ! -f Makefile ]] || [[ configure -nt Makefile ]]; then
    ./configure --without-bash-malloc
fi

LEGACY_CFLAGS="-O0 -g -std=gnu89 -fcommon \
-Wno-implicit-int -Wno-implicit-function-declaration \
-Wno-error -Wno-return-mismatch -Wno-incompatible-pointer-types \
-Wno-int-conversion -Wno-builtin-declaration-mismatch"

# CFLAGS_FOR_BUILD also needs the legacy flags: bash builds host helpers
# (mkbuiltins, mksignames) with $(CC_FOR_BUILD) which uses CFLAGS_FOR_BUILD,
# not CFLAGS. Without this, the helpers fail before any target object compiles.
if ! make -j"$(nproc)" \
        CFLAGS="${LEGACY_CFLAGS}" \
        CFLAGS_FOR_BUILD="${LEGACY_CFLAGS}"; then
    cat >&2 <<'HINT'

build failed. common remediation:
  - install bison, autoconf, texinfo, makeinfo via your distro
  - if a specific .c file is rejected, narrow CFLAGS in this script
  - some distros need additional -Wno-* flags for gcc >=15
HINT
    exit 1
fi

# The upstream test drivers expect helper executables in bash-5.2.21/tests
# (for example `recho` and `printenv`) with `.` on PATH. A plain bash build can
# leave these absent until `make tests` is run, but `make tests` would execute
# the oracle suite. Build and copy only the support helpers here.
if ! make recho zecho printenv xcase CFLAGS_FOR_BUILD="${LEGACY_CFLAGS}"; then
    echo "error: failed to build bash test support helpers" >&2
    exit 1
fi
mkdir -p tests
cp recho zecho printenv xcase tests/

if ! ./bash --version 2>/dev/null | head -n1 | grep -q 'version 5\.2\.21'; then
    echo "error: built binary does not report version 5.2.21" >&2
    ./bash --version >&2 || true
    exit 1
fi

echo "OK: ${BASH_SRC}/bash"
