#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_ROOT="${READLINE_SRC:-${WORKSPACE_ROOT}/vendor/readline-8.3}"
ORACLE_ROOT="${READLINE_ORACLE_ROOT:-${WORKSPACE_ROOT}/target/oracle/readline-8.3}"
BUILD_ROOT="${ORACLE_ROOT}/source"
DOWNLOAD_ROOT="${WORKSPACE_ROOT}/target/upstream/downloads"

if [[ -f "${BUILD_ROOT}/patchlevel" \
      && "$(tail -n 1 "${BUILD_ROOT}/patchlevel" | tr -d '[:space:]')" = "3" \
      && -f "${ORACLE_ROOT}/lib/libreadline.so.8.3" \
      && -f "${ORACLE_ROOT}/lib/libhistory.so.8.3" ]]; then
    printf 'Readline 8.3 patch 3 oracle: %s\n' "${ORACLE_ROOT}"
    exit 0
fi

if [[ ! -f "${SOURCE_ROOT}/configure" || ! -f "${SOURCE_ROOT}/readline.h" ]]; then
    echo "readline 8.3 source not found at ${SOURCE_ROOT}" >&2
    exit 1
fi

"${WORKSPACE_ROOT}/tools/fetch-upstream.sh" >/dev/null

case "${BUILD_ROOT}" in
    "${WORKSPACE_ROOT}"/target/oracle/readline-8.3/source) ;;
    *) echo "refusing to replace unexpected build path: ${BUILD_ROOT}" >&2; exit 1 ;;
esac

rm -rf -- "${BUILD_ROOT}"
mkdir -p "${BUILD_ROOT}"
cp -a "${SOURCE_ROOT}/." "${BUILD_ROOT}/"

while IFS= read -r -d '' file; do
    sed -i 's/\r$//' "${file}"
done < <(find "${BUILD_ROOT}" -type f \( -name '*.c' -o -name '*.h' -o -name '*.in' -o -name '*.sh' -o -name 'configure' -o -name 'Makefile' \) -print0)

for patch_number in 1 2 3; do
    patch_name="$(printf 'readline83-%03d' "${patch_number}")"
    patch -d "${BUILD_ROOT}" -p0 --forward --batch < "${DOWNLOAD_ROOT}/${patch_name}"
done

(
    cd "${BUILD_ROOT}"
    ./configure --prefix="${ORACLE_ROOT}" --enable-shared
    make -j"${JOBS:-2}"
    make install
)

test "$(tail -n 1 "${BUILD_ROOT}/patchlevel" | tr -d '[:space:]')" = "3"
test -f "${ORACLE_ROOT}/lib/libreadline.a"
test -f "${BUILD_ROOT}/shlib/libreadline.so.8.3"
test -f "${BUILD_ROOT}/shlib/libhistory.so.8.3"

cp -f "${BUILD_ROOT}/shlib/libreadline.so.8.3" "${ORACLE_ROOT}/lib/libreadline.so.8.3"
cp -f "${BUILD_ROOT}/shlib/libhistory.so.8.3" "${ORACLE_ROOT}/lib/libhistory.so.8.3"
ln -sfn libreadline.so.8.3 "${ORACLE_ROOT}/lib/libreadline.so.8"
ln -sfn libreadline.so.8 "${ORACLE_ROOT}/lib/libreadline.so"
ln -sfn libhistory.so.8.3 "${ORACLE_ROOT}/lib/libhistory.so.8"
ln -sfn libhistory.so.8 "${ORACLE_ROOT}/lib/libhistory.so"

printf 'Readline 8.3 patch 3 oracle: %s\n' "${ORACLE_ROOT}"
