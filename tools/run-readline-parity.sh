#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ORACLE_ROOT="${READLINE_ORACLE_ROOT:-${WORKSPACE_ROOT}/target/oracle/readline-8.3}"
IMPLEMENTATION_ROOT="${READLINE_OUTPUT_ROOT:-${WORKSPACE_ROOT}/target/readline}"
REPORT_ROOT="${READLINE_PARITY_REPORT_ROOT:-${WORKSPACE_ROOT}/target/parity/readline}"
EXAMPLE_ROOT="${ORACLE_ROOT}/source/examples"
PTY_CAPTURE="${WORKSPACE_ROOT}/tests/readline/pty_capture.py"

if pkg-config --exists tinfo 2>/dev/null; then
    read -r -a TERMINAL_LIBS <<< "$(pkg-config --libs tinfo)"
else
    TERMINAL_LIBS=(-ltermcap)
fi

"${WORKSPACE_ROOT}/oracle/build-readline-8.3.sh"
"${WORKSPACE_ROOT}/tools/build-readline.sh"

case "${REPORT_ROOT}" in
    "${WORKSPACE_ROOT}"/target/parity/readline) ;;
    *) echo "refusing to replace unexpected report path: ${REPORT_ROOT}" >&2; exit 1 ;;
esac
rm -rf -- "${REPORT_ROOT}"
mkdir -p "${REPORT_ROOT}/oracle" "${REPORT_ROOT}/implementation" "${REPORT_ROOT}/home"

compile_fixture() {
    local root="$1"
    local fixture="$2"
    local output="$3"
    local library="$4"
    shift 4
    cc -std=c11 -Wall -Wextra -Werror \
        "$@" \
        -I"${root}/include" \
        "${fixture}" \
        -L"${root}/lib" -Wl,-rpath,"${root}/lib" \
        "-l${library}" "${TERMINAL_LIBS[@]}" \
        -o "${output}"
}

compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/abi_smoke.c" "${REPORT_ROOT}/oracle/abi-smoke" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/abi_smoke.c" "${REPORT_ROOT}/implementation/abi-smoke" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/history_link.c" "${REPORT_ROOT}/oracle/history-link" history
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/history_link.c" "${REPORT_ROOT}/implementation/history-link" history
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/readline_loop.c" "${REPORT_ROOT}/oracle/readline-loop" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/readline_loop.c" "${REPORT_ROOT}/implementation/readline-loop" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/custom_binding.c" "${REPORT_ROOT}/oracle/custom-binding" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/custom_binding.c" "${REPORT_ROOT}/implementation/custom-binding" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/public_behavior.c" "${REPORT_ROOT}/oracle/public-behavior" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/public_behavior.c" "${REPORT_ROOT}/implementation/public-behavior" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/abi_layout.c" "${REPORT_ROOT}/oracle/abi-layout" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/abi_layout.c" "${REPORT_ROOT}/implementation/abi-layout" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/allocator_ownership.c" "${REPORT_ROOT}/oracle/allocator-ownership" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/allocator_ownership.c" "${REPORT_ROOT}/implementation/allocator-ownership" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/callback_lifecycle.c" "${REPORT_ROOT}/oracle/callback-lifecycle" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/callback_lifecycle.c" "${REPORT_ROOT}/implementation/callback-lifecycle" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/completion_inputrc.c" "${REPORT_ROOT}/oracle/completion-inputrc" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/completion_inputrc.c" "${REPORT_ROOT}/implementation/completion-inputrc" readline
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/history_state.c" "${REPORT_ROOT}/oracle/history-state" history
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/history_state.c" "${REPORT_ROOT}/implementation/history-state" history
compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/redisplay_streams.c" "${REPORT_ROOT}/oracle/redisplay-streams" readline
compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/redisplay_streams.c" "${REPORT_ROOT}/implementation/redisplay-streams" readline

for fixture in allocator_ownership callback_lifecycle completion_inputrc history_state; do
    output_name="${fixture//_/-}-asan"
    compile_fixture "${ORACLE_ROOT}" "${WORKSPACE_ROOT}/tests/readline/${fixture}.c" \
        "${REPORT_ROOT}/oracle/${output_name}" readline -fsanitize=address -fno-omit-frame-pointer
    compile_fixture "${IMPLEMENTATION_ROOT}" "${WORKSPACE_ROOT}/tests/readline/${fixture}.c" \
        "${REPORT_ROOT}/implementation/${output_name}" readline -fsanitize=address -fno-omit-frame-pointer
done

for side in oracle implementation; do
    HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TERM=xterm-256color \
        "${REPORT_ROOT}/${side}/abi-smoke" > "${REPORT_ROOT}/${side}/abi-smoke.out"
    "${REPORT_ROOT}/${side}/history-link" > "${REPORT_ROOT}/${side}/history-link.out"
    "${REPORT_ROOT}/${side}/public-behavior" > "${REPORT_ROOT}/${side}/public-behavior.out"
    "${REPORT_ROOT}/${side}/abi-layout" > "${REPORT_ROOT}/${side}/abi-layout.out"
    "${REPORT_ROOT}/${side}/allocator-ownership" > "${REPORT_ROOT}/${side}/allocator-ownership.out"
    "${REPORT_ROOT}/${side}/callback-lifecycle" > "${REPORT_ROOT}/${side}/callback-lifecycle.out"
    "${REPORT_ROOT}/${side}/completion-inputrc" > "${REPORT_ROOT}/${side}/completion-inputrc.out"
    "${REPORT_ROOT}/${side}/history-state" > "${REPORT_ROOT}/${side}/history-state.out"
    "${REPORT_ROOT}/${side}/redisplay-streams" > "${REPORT_ROOT}/${side}/redisplay-streams.out"
    for fixture in allocator-ownership callback-lifecycle completion-inputrc history-state; do
        ASAN_OPTIONS=detect_leaks=1:abort_on_error=1 \
            "${REPORT_ROOT}/${side}/${fixture}-asan" >/dev/null
    done
    printf 'typed line\n\004' \
        | HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TERM=xterm-256color \
            python3 "${PTY_CAPTURE}" "${REPORT_ROOT}/${side}/readline-loop" \
        > "${REPORT_ROOT}/${side}/readline-loop.out"
    printf 'so\t\t\n\004' \
        | HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TERM=xterm-256color \
            python3 "${PTY_CAPTURE}" "${REPORT_ROOT}/${side}/readline-loop" \
        > "${REPORT_ROOT}/${side}/readline-loop-completion.out"
    printf '\030q\n\030m\nr\n' \
        | HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TERM=xterm-256color \
            python3 "${PTY_CAPTURE}" "${REPORT_ROOT}/${side}/custom-binding" \
        > "${REPORT_ROOT}/${side}/custom-binding.out"
done

diff -u "${REPORT_ROOT}/oracle/abi-smoke.out" "${REPORT_ROOT}/implementation/abi-smoke.out"
diff -u "${REPORT_ROOT}/oracle/history-link.out" "${REPORT_ROOT}/implementation/history-link.out"
diff -u "${REPORT_ROOT}/oracle/readline-loop.out" "${REPORT_ROOT}/implementation/readline-loop.out"
diff -u "${REPORT_ROOT}/oracle/readline-loop-completion.out" "${REPORT_ROOT}/implementation/readline-loop-completion.out"
diff -u "${REPORT_ROOT}/oracle/custom-binding.out" "${REPORT_ROOT}/implementation/custom-binding.out"
diff -u "${REPORT_ROOT}/oracle/public-behavior.out" "${REPORT_ROOT}/implementation/public-behavior.out"
diff -u "${REPORT_ROOT}/oracle/abi-layout.out" "${REPORT_ROOT}/implementation/abi-layout.out"
diff -u "${REPORT_ROOT}/oracle/allocator-ownership.out" "${REPORT_ROOT}/implementation/allocator-ownership.out"
diff -u "${REPORT_ROOT}/oracle/callback-lifecycle.out" "${REPORT_ROOT}/implementation/callback-lifecycle.out"
diff -u "${REPORT_ROOT}/oracle/completion-inputrc.out" "${REPORT_ROOT}/implementation/completion-inputrc.out"
diff -u "${REPORT_ROOT}/oracle/history-state.out" "${REPORT_ROOT}/implementation/history-state.out"
diff -u "${REPORT_ROOT}/oracle/redisplay-streams.out" "${REPORT_ROOT}/implementation/redisplay-streams.out"

perl -ne 'if (/^extern.*?([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*(?:\(|;|\[)/) { print "$1\n" }' \
    "${IMPLEMENTATION_ROOT}"/include/readline/{readline,history,keymaps,tilde}.h \
    | sort -u > "${REPORT_ROOT}/public-symbols.txt"
nm -D --defined-only --format=posix "${IMPLEMENTATION_ROOT}/lib/libreadline.so" \
    | cut -d' ' -f1 | sort -u > "${REPORT_ROOT}/implementation-symbols.txt"
comm -23 "${REPORT_ROOT}/public-symbols.txt" "${REPORT_ROOT}/implementation-symbols.txt" \
    > "${REPORT_ROOT}/missing-symbols.txt"
test ! -s "${REPORT_ROOT}/missing-symbols.txt"

test "$(readelf -d "${IMPLEMENTATION_ROOT}/lib/libreadline.so.8.3" | sed -n 's/.*SONAME.*\[\(.*\)\].*/\1/p')" = "libreadline.so.8"
test "$(readelf -d "${IMPLEMENTATION_ROOT}/lib/libhistory.so.8.3" | sed -n 's/.*SONAME.*\[\(.*\)\].*/\1/p')" = "libhistory.so.8"
test ! "${IMPLEMENTATION_ROOT}/lib/libhistory.so.8.3" -ef "${IMPLEMENTATION_ROOT}/lib/libreadline.so.8.3"
PKG_CONFIG_PATH="${IMPLEMENTATION_ROOT}/lib/pkgconfig" pkg-config --exists 'readline = 8.3' 'history = 8.3'

capture_plain_example() {
    local output_base="$1"
    shift
    set +e
    (
        cd "${REPORT_ROOT}/home"
        HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TZ=UTC "$@"
    ) > "${output_base}.stdout" 2> "${output_base}.stderr"
    local status=$?
    set -e
    printf '%s\n' "${status}" > "${output_base}.status"
}

capture_stdin_example() {
    local input="$1"
    local output_base="$2"
    shift 2
    set +e
    printf '%s' "${input}" | (
        cd "${REPORT_ROOT}/home"
        HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TZ=UTC "$@"
    ) > "${output_base}.stdout" 2> "${output_base}.stderr"
    local status=${PIPESTATUS[1]}
    set -e
    printf '%s\n' "${status}" > "${output_base}.status"
}

capture_pty_example() {
    local input="$1"
    local output_base="$2"
    shift 2
    set +e
    (
        cd "${REPORT_ROOT}/home"
        printf '%s' "${input}" \
            | HOME="${REPORT_ROOT}/home" LC_ALL=C.UTF-8 TZ=UTC \
                python3 "${PTY_CAPTURE}" "$@"
    ) > "${output_base}.stdout"
    local status=$?
    set -e
    printf '%s\n' "${status}" > "${output_base}.status"
}

capture_example_set() {
    local side="$1"
    local output_dir="${REPORT_ROOT}/${side}/examples"
    mkdir -p "${output_dir}"

    capture_plain_example "${output_dir}/rlversion" "${EXAMPLE_ROOT}/rlversion"
    capture_plain_example "${output_dir}/rlkeymaps" "${EXAMPLE_ROOT}/rlkeymaps"
    capture_stdin_example $'echo first\n!!\necho "$HOME"\n!$\nlist\nquit\n' \
        "${output_dir}/histexamp" "${EXAMPLE_ROOT}/histexamp"
    perl -pi -e 's/: [A-Z][a-z]{2} [0-9]{2}:[0-9]{2}:/: TIME:/g' \
        "${output_dir}/histexamp.stdout"
    capture_pty_example $'hello\nexit\n' \
        "${output_dir}/rlbasic" "${EXAMPLE_ROOT}/rlbasic"
    capture_pty_example $'hello\nexit\n' \
        "${output_dir}/callback" "${EXAMPLE_ROOT}/rl-callbacktest"
    capture_pty_example $'answer\n' \
        "${output_dir}/rl" "${EXAMPLE_ROOT}/rl" -p 'readline$ '

    local duplicate_history="${output_dir}/duplicate-history"
    printf 'keep\ndrop\nkeep\nlast\ndrop\n' > "${duplicate_history}"
    capture_plain_example "${output_dir}/hist-erasedups" \
        "${EXAMPLE_ROOT}/hist_erasedups" "${duplicate_history}"
    cp "${duplicate_history}" "${output_dir}/hist-erasedups.result"

    local purge_history="${output_dir}/purge-history"
    printf 'keep\ndrop one\nkeep two\ndrop two\n' > "${purge_history}"
    capture_plain_example "${output_dir}/hist-purgecmd" \
        "${EXAMPLE_ROOT}/hist_purgecmd" -f "${purge_history}" -r '^drop '
    cp "${purge_history}" "${output_dir}/hist-purgecmd.result"
    rm -f "${duplicate_history}" "${purge_history}"
}

make -C "${EXAMPLE_ROOT}" clean >/dev/null 2>&1
make -C "${EXAMPLE_ROOT}" -j"${JOBS:-2}" all >/dev/null
capture_example_set oracle
make -C "${EXAMPLE_ROOT}" clean >/dev/null 2>&1
make -C "${EXAMPLE_ROOT}" -j"${JOBS:-2}" all \
    READLINE_LIB="${IMPLEMENTATION_ROOT}/lib/libreadline.so" \
    HISTORY_LIB="${IMPLEMENTATION_ROOT}/lib/libhistory.so" \
    LDFLAGS="-L${IMPLEMENTATION_ROOT}/lib -Wl,-rpath,${IMPLEMENTATION_ROOT}/lib" \
    >/dev/null
capture_example_set implementation
diff -ru "${REPORT_ROOT}/oracle/examples" "${REPORT_ROOT}/implementation/examples"

printf 'Readline parity report: %s\n' "${REPORT_ROOT}"
