#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_ROOT="${READLINE_OUTPUT_ROOT:-${WORKSPACE_ROOT}/target/readline}"
PROFILE="${READLINE_PROFILE:-release}"

case "${PROFILE}" in
    release) PROFILE_FLAG=(--release); CARGO_DIR=release ;;
    debug) PROFILE_FLAG=(); CARGO_DIR=debug ;;
    *) echo "unsupported profile: ${PROFILE}" >&2; exit 2 ;;
esac

"${CARGO:-cargo}" build -p cherubsh-readline-ffi --locked "${PROFILE_FLAG[@]}"
mkdir -p "${OUTPUT_ROOT}/lib" "${OUTPUT_ROOT}/include"
cp -a "${WORKSPACE_ROOT}/include/readline" "${OUTPUT_ROOT}/include/"
cp -f "${WORKSPACE_ROOT}/target/${CARGO_DIR}/libreadline.so" "${OUTPUT_ROOT}/lib/libreadline.so.8.3"
cp -f "${WORKSPACE_ROOT}/target/${CARGO_DIR}/libreadline.a" "${OUTPUT_ROOT}/lib/libreadline.a"
cp -f "${OUTPUT_ROOT}/lib/libreadline.a" "${OUTPUT_ROOT}/lib/libhistory.a"
ln -sfn libreadline.so.8.3 "${OUTPUT_ROOT}/lib/libreadline.so.8"
ln -sfn libreadline.so.8 "${OUTPUT_ROOT}/lib/libreadline.so"
ln -sfn libreadline.so.8.3 "${OUTPUT_ROOT}/lib/libhistory.so.8.3"
ln -sfn libhistory.so.8.3 "${OUTPUT_ROOT}/lib/libhistory.so.8"
ln -sfn libhistory.so.8 "${OUTPUT_ROOT}/lib/libhistory.so"

printf 'Readline-compatible libraries: %s\n' "${OUTPUT_ROOT}"
