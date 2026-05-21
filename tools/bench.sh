#!/usr/bin/env bash
# Compare CherubSH against Bash on startup and representative shell workloads.
#
# Environment knobs:
#   RUNS=15              measured runs per case
#   WARMUPS=3            warmup runs per case
#   CHERUBSH=path        CherubSH binary to test
#   BASH_ORACLE_VERSION  Bash oracle version: 5.3 default, or 5.2.21
#   BASH_ORACLE_PATH     explicit Bash oracle binary path
#   BASH_53_PATH=path    Bash 5.3 oracle binary
#   BASH_521_PATH=path   Bash 5.2.21 oracle binary
#   BENCH_BUILD=0        skip cargo release build

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUNS="${RUNS:-15}"
WARMUPS="${WARMUPS:-3}"
CHERUBSH="${CHERUBSH:-${WS_ROOT}/target/release/cherubsh}"
BENCH_BUILD="${BENCH_BUILD:-1}"
ORACLE_VERSION="${BASH_ORACLE_VERSION:-5.3}"

case "${ORACLE_VERSION}" in
    5.2|5.2.21)
        ORACLE_VERSION="5.2.21"
        ORACLE_LABEL="bash-5.2.21"
        RATIO_COLUMN="ratio_vs_bash_521"
        ORACLE="${BASH_ORACLE_PATH:-${BASH_521_PATH:-${WS_ROOT}/target/oracle/bash-5.2.21/bash}}"
        ORACLE_BUILDER="${WS_ROOT}/oracle/build-bash-5.2.21.sh"
        VERSION_RE='version 5\.2\.21'
        ;;
    5.3|5.3.0)
        ORACLE_VERSION="5.3"
        ORACLE_LABEL="bash-5.3"
        RATIO_COLUMN="ratio_vs_bash_53"
        ORACLE="${BASH_ORACLE_PATH:-${BASH_53_PATH:-${WS_ROOT}/target/oracle/bash-5.3/bash}}"
        ORACLE_BUILDER="${WS_ROOT}/oracle/build-bash-5.3.sh"
        VERSION_RE='version 5\.3(\.0)?'
        ;;
    *)
        echo "error: unsupported BASH_ORACLE_VERSION=${ORACLE_VERSION}" >&2
        exit 2
        ;;
esac

if ! [[ "${RUNS}" =~ ^[0-9]+$ ]] || (( RUNS < 1 )); then
    echo "error: RUNS must be a positive integer" >&2
    exit 2
fi
if ! [[ "${WARMUPS}" =~ ^[0-9]+$ ]]; then
    echo "error: WARMUPS must be a non-negative integer" >&2
    exit 2
fi

cd "${WS_ROOT}"

if [[ "${BENCH_BUILD}" != 0 ]]; then
    cargo build --release -p cherubsh >/dev/null
fi

if ! [[ -x "${CHERUBSH}" ]]; then
    echo "error: CherubSH binary not found at ${CHERUBSH}" >&2
    exit 2
fi

if ! [[ -x "${ORACLE}" ]] || ! "${ORACLE}" --version 2>/dev/null | head -n1 | grep -Eq "${VERSION_RE}"; then
    echo ">> oracle missing or wrong version; building bash-${ORACLE_VERSION}..."
    bash "${ORACLE_BUILDER}" >/dev/null
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/cherubsh-bench.XXXXXX")"
cleanup() {
    rm -rf "${TMP}"
}
trap cleanup EXIT

mkdir -p "${TMP}/home" "${TMP}/scripts" "${TMP}/glob" "${TMP}/dirs/a"

for i in $(seq -w 1 400); do
    printf 'data-%s\n' "${i}" >"${TMP}/glob/file-${i}.txt"
done

for d in a b c; do
    mkdir -p "${TMP}/glob/nested/${d}"
    for i in $(seq -w 1 50); do
        printf 'nested-%s-%s\n' "${d}" "${i}" >"${TMP}/glob/nested/${d}/item-${i}.txt"
    done
done

INPUT="${TMP}/input-lines.txt"
for i in $(seq -w 1 1000); do
    printf 'line-%s:alpha beta gamma\n' "${i}" >>"${INPUT}"
done

LIB="${TMP}/lib-many-functions.sh"
for i in $(seq 0 499); do
    printf 'bench_func_%03d() { : "${1:-}"; }\n' "${i}" >>"${LIB}"
done

ASSIGN_LIB="${TMP}/lib-many-assignments.sh"
for i in $(seq 0 999); do
    printf 'v_%03d=%d\n' "${i}" "${i}" >>"${ASSIGN_LIB}"
done

RC="${TMP}/benchrc"
{
    printf 'PS1="bench$ "\n'
    printf 'PROMPT_COMMAND=:\n'
    for i in $(seq 0 199); do
        printf 'rc_func_%03d() { : "${1:-}"; }\n' "${i}"
    done
} >"${RC}"

write_script() {
    local name="$1"
    shift
    local path="${TMP}/scripts/${name}.sh"
    printf '%s\n' "$@" >"${path}"
    chmod +x "${path}"
    printf '%s\n' "${path}"
}

LARGE_PARSE="${TMP}/scripts/large-parse.sh"
{
    printf 'sum=0\n'
    for i in $(seq 1 2000); do
        printf '((sum += %d))\n' "${i}"
    done
    printf ': "$sum"\n'
} >"${LARGE_PARSE}"
chmod +x "${LARGE_PARSE}"

LONG_CMD_PAYLOAD='x=0'
for i in $(seq 1 500); do
    LONG_CMD_PAYLOAD="${LONG_CMD_PAYLOAD}; ((x += ${i}))"
done
LONG_CMD_PAYLOAD="${LONG_CMD_PAYLOAD}; : \"\$x\""

ARITH_LOOP="$(write_script arith-loop \
    'limit=${BENCH_N:-50000}' \
    'sum=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    ((sum += (i * 3) % 17))' \
    'done' \
    ': "$sum"')"

ARITH_WHILE="$(write_script arith-while \
    'limit=${BENCH_N:-50000}' \
    'sum=0' \
    'i=0' \
    'while ((i < limit)); do' \
    '    ((sum += (i * 5) % 23, i++))' \
    'done' \
    ': "$sum"')"

NESTED_LOOPS="$(write_script nested-loops \
    'limit=${BENCH_N:-1200}' \
    'sum=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    for j in 1 2 3 4 5; do' \
    '        [[ $j == 3 ]] && continue' \
    '        ((sum += j))' \
    '    done' \
    'done' \
    ': "$sum"')"

SIMPLE_BUILTINS="$(write_script simple-builtins \
    'limit=${BENCH_N:-80000}' \
    'count=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    : "$i"' \
    '    true' \
    '    ((count++))' \
    'done' \
    '[[ $count -eq $limit ]]')"

VARIABLE_ASSIGNMENTS="$(write_script variable-assignments \
    'limit=${BENCH_N:-30000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    A=$i B="value-$i" C=${A:-fallback}' \
    '    : "$A$B$C"' \
    'done')"

FUNCTION_CALLS="$(write_script function-calls \
    'limit=${BENCH_N:-30000}' \
    'f() { local x=${1:-0}; ((x += 1)); }' \
    'for ((i = 0; i < limit; i++)); do' \
    '    f "$i"' \
    'done')"

LOCAL_VARIABLES="$(write_script local-variables \
    'limit=${BENCH_N:-20000}' \
    'f() {' \
    '    local a=${1:-0} b=${2:-x}' \
    '    local c="${a}-${b}"' \
    '    : "$c"' \
    '}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    f "$i" value' \
    'done')"

ALIAS_EXPANSION="$(write_script alias-expansion \
    'shopt -s expand_aliases' \
    "alias noop=':'" \
    'limit=${BENCH_N:-30000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    noop "$i"' \
    'done')"

ARRAYS="$(write_script arrays \
    'limit=${BENCH_N:-12000}' \
    'a=()' \
    'for ((i = 0; i < limit; i++)); do' \
    '    a[i]=$i' \
    'done' \
    '[[ ${#a[@]} -eq $limit ]]')"

ARRAY_EXPANSION="$(write_script array-expansion \
    'a=(alpha beta gamma delta epsilon zeta eta theta iota kappa)' \
    'limit=${BENCH_N:-15000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    joined="${a[*]}"' \
    '    copy=("${a[@]}")' \
    '    keys=("${!a[@]}")' \
    'done' \
    '[[ ${#copy[@]} -eq 10 && ${#keys[@]} -eq 10 && -n $joined ]]')"

ARRAY_SLICE="$(write_script array-slice \
    'a=()' \
    'for i in {0..99}; do a[i]="value-$i"; done' \
    'limit=${BENCH_N:-12000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    slice=("${a[@]:10:20}")' \
    'done' \
    '[[ ${#slice[@]} -eq 20 ]]')"

ASSOC_ARRAYS="$(write_script assoc-arrays \
    'limit=${BENCH_N:-6000}' \
    'declare -A m' \
    'for ((i = 0; i < limit; i++)); do' \
    '    m["k$i"]=$i' \
    'done' \
    '[[ ${#m[@]} -eq $limit ]]')"

ASSOC_LOOKUPS="$(write_script assoc-lookups \
    'declare -A m' \
    'for ((i = 0; i < 500; i++)); do m["k$i"]=$i; done' \
    'limit=${BENCH_N:-25000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    key="k$((i % 500))"' \
    '    x=${m[$key]}' \
    'done' \
    ': "$x"')"

PARAM_EXPANSION="$(write_script param-expansion \
    'limit=${BENCH_N:-30000}' \
    's=alpha_beta_gamma_delta_alpha_beta_gamma_delta' \
    'for ((i = 0; i < limit; i++)); do' \
    '    x=${s//alpha/omega}' \
    '    x=${x#omega_}' \
    '    x=${x%_delta}' \
    'done' \
    ': "$x"')"

PARAM_SUBSTRING_CASEMOD="$(write_script param-substring-casemod \
    'limit=${BENCH_N:-25000}' \
    's=alphaBetaGammaDelta' \
    'for ((i = 0; i < limit; i++)); do' \
    '    x=${s:2:8}' \
    '    y=${s^^}' \
    '    z=${s,,}' \
    'done' \
    ': "$x$y$z"')"

PARAM_DEFAULTS="$(write_script param-defaults \
    'limit=${BENCH_N:-30000}' \
    'set_value=value' \
    'unset missing_value' \
    'for ((i = 0; i < limit; i++)); do' \
    '    x=${missing_value:-fallback}' \
    '    y=${set_value:+alternate}' \
    '    z=${x}${y}' \
    'done' \
    ': "$z"')"

PARAM_NAMEREF="$(write_script param-nameref \
    'limit=${BENCH_N:-20000}' \
    'target=0' \
    'declare -n ref=target' \
    'for ((i = 0; i < limit; i++)); do' \
    '    ref=$i' \
    '    [[ $target -eq $i ]] || exit 1' \
    'done')"

CONDITIONALS="$(write_script conditionals \
    'limit=${BENCH_N:-30000}' \
    'hit=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    s="item-$((i % 100))-alpha"' \
    '    if [[ $s == item-*-alpha && ${#s} -gt 10 && $i -ge 0 ]]; then' \
    '        ((hit++))' \
    '    fi' \
    'done' \
    '[[ $hit -eq $limit ]]')"

CASE_PATTERNS="$(write_script case-patterns \
    'limit=${BENCH_N:-30000}' \
    'hit=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    case "src/module-$((i % 10)).rs" in' \
    '        src/module-[02468].rs) ((hit += 2));;' \
    '        src/module-[13579].rs) ((hit += 1));;' \
    '        *) ((hit += 0));;' \
    '    esac' \
    'done' \
    ': "$hit"')"

REGEX_MATCH="$(write_script regex-match \
    'limit=${BENCH_N:-25000}' \
    'hit=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    s="item-$i-alpha"' \
    '    [[ $s =~ ^item-[0-9]+-(alpha|beta)$ ]] && ((hit++))' \
    'done' \
    '[[ $hit -eq $limit ]]')"

EXTGLOB_MATCH="$(write_script extglob-match \
    'shopt -s extglob' \
    "pat='@(alpha|beta)-+([0-9])'" \
    'limit=${BENCH_N:-25000}' \
    'hit=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    s="alpha-$i"' \
    '    [[ $s == $pat ]] && ((hit++))' \
    'done' \
    '[[ $hit -eq $limit ]]')"

WORD_SPLITTING="$(write_script word-splitting \
    'limit=${BENCH_N:-15000}' \
    'words="alpha beta  gamma   delta epsilon zeta eta theta"' \
    'count=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    for word in $words; do' \
    '        case $word in' \
    '            a*|e*) ((count++));;' \
    '        esac' \
    '    done' \
    'done' \
    ': "$count"')"

BRACE_EXPANSION="$(write_script brace-expansion \
    'limit=${BENCH_N:-2000}' \
    'count=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    for word in bench/{alpha,beta,gamma,delta}/{01..20}; do' \
    '        [[ $word == bench/* ]] && ((count++))' \
    '    done' \
    'done' \
    '[[ $count -eq $((limit * 80)) ]]')"

COMMAND_SUBST="$(write_script command-subst \
    'limit=${BENCH_N:-1500}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    x=$(printf "%s" "$i")' \
    'done' \
    ': "$x"')"

COMMAND_SUBST_NESTED="$(write_script command-subst-nested \
    'limit=${BENCH_N:-700}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    x=$(printf "%s" "$(printf "%s" "$i")")' \
    'done' \
    ': "$x"')"

HERE_STRINGS_READ="$(write_script here-strings-read \
    'limit=${BENCH_N:-5000}' \
    'line="alpha beta gamma"' \
    'count=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    IFS=" " read -r a b c <<<"$line"' \
    '    [[ $b == beta ]] && ((count++))' \
    'done' \
    '[[ $count -eq $limit ]]')"

READ_LOOP="$(write_script read-loop \
    'count=0' \
    'while IFS=: read -r left right; do' \
    '    [[ $left == line-* && $right == alpha* ]] && ((count++))' \
    'done < "$BENCH_TMP/input-lines.txt"' \
    '[[ $count -eq 1000 ]]')"

MAPFILE_LOAD="$(write_script mapfile-load \
    'mapfile -t lines < "$BENCH_TMP/input-lines.txt"' \
    '[[ ${#lines[@]} -eq 1000 ]]')"

PRINTF_BUILTIN="$(write_script printf-builtin \
    'limit=${BENCH_N:-12000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    printf -v out "item-%05d:%s" "$i" value' \
    'done' \
    ': "$out"')"

TEST_BUILTIN="$(write_script test-builtin \
    'limit=${BENCH_N:-50000}' \
    's=value' \
    'for ((i = 0; i < limit; i++)); do' \
    '    [ "$i" -ge 0 ] && [ -n "$s" ]' \
    'done')"

COMMAND_LOOKUP="$(write_script command-lookup \
    'limit=${BENCH_N:-8000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    type printf >/dev/null' \
    '    command -v sh >/dev/null' \
    'done')"

COMPGEN_WORDS="$(write_script compgen-words \
    'limit=${BENCH_N:-5000}' \
    'words="alpha beta gamma delta epsilon zeta eta theta"' \
    'for ((i = 0; i < limit; i++)); do' \
    '    compgen -W "$words" -- e >/dev/null' \
    'done')"

REDIRECTIONS="$(write_script redirections \
    'limit=${BENCH_N:-2000}' \
    'out="$BENCH_TMP/redirection.out"' \
    ': >"$out"' \
    'for ((i = 0; i < limit; i++)); do' \
    '    printf "%s\n" "$i" >>"$out"' \
    'done' \
    '[[ -s $out ]]')"

REDIRECTION_GROUPS="$(write_script redirection-groups \
    'limit=${BENCH_N:-1500}' \
    'out="$BENCH_TMP/redirection-group.out"' \
    ': >"$out"' \
    'for ((i = 0; i < limit; i++)); do' \
    '    { printf "%s\n" "$i"; printf "%s\n" "$((i + 1))"; } >>"$out"' \
    'done' \
    '[[ -s $out ]]')"

HEREDOC_READ="$(write_script heredoc-read \
    'limit=${BENCH_N:-2000}' \
    'count=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    read -r a b <<EOF' \
    'alpha beta' \
    'EOF' \
    '    [[ $b == beta ]] && ((count++))' \
    'done' \
    '[[ $count -eq $limit ]]')"

EVAL_PARSE="$(write_script eval-parse \
    'limit=${BENCH_N:-3000}' \
    'x=0' \
    'for ((i = 0; i < limit; i++)); do' \
    '    eval "case $((i % 3)) in 0) ((x+=1));; 1) ((x+=2));; *) ((x+=3));; esac"' \
    'done' \
    ': "$x"')"

SUBSHELLS="$(write_script subshells \
    'limit=${BENCH_N:-2000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    (: "$i")' \
    'done')"

BACKGROUND_WAIT="$(write_script background-wait \
    'limit=${BENCH_N:-100}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    (: "$i") &' \
    '    wait "$!"' \
    'done')"

PIPELINE_EXTERNAL="$(write_script pipeline-external \
    'limit=${BENCH_N:-250}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    printf "%s\n" "$i" | wc -l >/dev/null' \
    'done')"

PIPELINE_BUILTIN="$(write_script pipeline-builtin \
    'limit=${BENCH_N:-1000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    printf "%s\n" "$i" | { read -r line; : "$line"; }' \
    'done')"

PROCESS_SUBST="$(write_script process-subst \
    'limit=${BENCH_N:-100}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    cmp -s <(printf "%s\n" "$i") <(printf "%s\n" "$i")' \
    'done')"

EXTERNAL_COMMANDS="$(write_script external-commands \
    'limit=${BENCH_N:-120}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    /usr/bin/true' \
    'done')"

SHOPT_OPTIONS="$(write_script shopt-options \
    'limit=${BENCH_N:-5000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    shopt -s nullglob extglob' \
    '    shopt -u nullglob extglob' \
    '    set -o pipefail' \
    '    set +o pipefail' \
    'done')"

POSITIONAL_PARAMS="$(write_script positional-params \
    'limit=${BENCH_N:-12000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    set -- alpha "$i" gamma delta' \
    '    [[ $# -eq 4 ]] || exit 1' \
    '    shift' \
    'done')"

GETOPTS_PARSE="$(write_script getopts-parse \
    'limit=${BENCH_N:-5000}' \
    'for ((i = 0; i < limit; i++)); do' \
    '    OPTIND=1' \
    '    while getopts "ab:c" opt -a -b value -c; do' \
    '        : "$opt"' \
    '    done' \
    'done')"

TRAP_SET="$(write_script trap-set \
    'limit=${BENCH_N:-5000}' \
    'for ((i = 0; i < limit; i++)); do' \
    "    trap ':' EXIT" \
    '    trap - EXIT' \
    'done')"

CD_PWD="$(write_script cd-pwd \
    'limit=${BENCH_N:-2000}' \
    'start=$PWD' \
    'for ((i = 0; i < limit; i++)); do' \
    '    cd "$BENCH_TMP/dirs/a"' \
    '    pwd >/dev/null' \
    '    cd "$start"' \
    'done')"

SOURCE_MANY_FUNCTIONS="$(write_script source-many-functions \
    '. "$BENCH_LIB"' \
    'bench_func_250 ok' \
    'type bench_func_499 >/dev/null')"

SOURCE_MANY_ASSIGNMENTS="$(write_script source-many-assignments \
    '. "$BENCH_TMP/lib-many-assignments.sh"' \
    '[[ ${v_999:-} -eq 999 ]]')"

GLOB_SCAN="$(write_script glob-scan \
    'count=0' \
    'for f in $BENCH_GLOB_DIR/*.txt; do' \
    '    [[ -f $f ]] && ((count++))' \
    'done' \
    '[[ $count -eq 400 ]]')"

GLOB_ARRAY_CAPTURE="$(write_script glob-array-capture \
    'files=()' \
    'for f in $BENCH_GLOB_DIR/*.txt; do' \
    '    files+=("$f")' \
    'done' \
    '[[ ${#files[@]} -eq 400 ]]')"

GLOBSTAR_SCAN="$(write_script globstar-scan \
    'shopt -s globstar nullglob' \
    'count=0' \
    'for f in $BENCH_GLOB_DIR/**/*.txt; do' \
    '    [[ -f $f ]] && ((count++))' \
    'done' \
    '[[ $count -eq 550 ]]')"

CASES=(
    $'startup_no_rc\tcmd\t:'
    $'startup_controlled_rc\tinteractive_rc\t:'
    "cmd_many_statements"$'\t'"cmd"$'\t'"${LONG_CMD_PAYLOAD}"
    "large_parse_script"$'\t'"script"$'\t'"${LARGE_PARSE}"
    "simple_builtins"$'\t'"script"$'\t'"${SIMPLE_BUILTINS}"
    "variable_assignments"$'\t'"script"$'\t'"${VARIABLE_ASSIGNMENTS}"
    "arith_loop"$'\t'"script"$'\t'"${ARITH_LOOP}"
    "arith_while"$'\t'"script"$'\t'"${ARITH_WHILE}"
    "nested_loops"$'\t'"script"$'\t'"${NESTED_LOOPS}"
    "function_calls"$'\t'"script"$'\t'"${FUNCTION_CALLS}"
    "local_variables"$'\t'"script"$'\t'"${LOCAL_VARIABLES}"
    "alias_expansion"$'\t'"script"$'\t'"${ALIAS_EXPANSION}"
    "arrays"$'\t'"script"$'\t'"${ARRAYS}"
    "array_expansion"$'\t'"script"$'\t'"${ARRAY_EXPANSION}"
    "array_slice"$'\t'"script"$'\t'"${ARRAY_SLICE}"
    "assoc_arrays"$'\t'"script"$'\t'"${ASSOC_ARRAYS}"
    "assoc_lookups"$'\t'"script"$'\t'"${ASSOC_LOOKUPS}"
    "param_expansion"$'\t'"script"$'\t'"${PARAM_EXPANSION}"
    "param_substring_casemod"$'\t'"script"$'\t'"${PARAM_SUBSTRING_CASEMOD}"
    "param_defaults"$'\t'"script"$'\t'"${PARAM_DEFAULTS}"
    "param_nameref"$'\t'"script"$'\t'"${PARAM_NAMEREF}"
    "conditionals"$'\t'"script"$'\t'"${CONDITIONALS}"
    "case_patterns"$'\t'"script"$'\t'"${CASE_PATTERNS}"
    "regex_match"$'\t'"script"$'\t'"${REGEX_MATCH}"
    "extglob_match"$'\t'"script"$'\t'"${EXTGLOB_MATCH}"
    "word_splitting"$'\t'"script"$'\t'"${WORD_SPLITTING}"
    "brace_expansion"$'\t'"script"$'\t'"${BRACE_EXPANSION}"
    "command_subst"$'\t'"script"$'\t'"${COMMAND_SUBST}"
    "command_subst_nested"$'\t'"script"$'\t'"${COMMAND_SUBST_NESTED}"
    "here_strings_read"$'\t'"script"$'\t'"${HERE_STRINGS_READ}"
    "read_loop"$'\t'"script"$'\t'"${READ_LOOP}"
    "mapfile_load"$'\t'"script"$'\t'"${MAPFILE_LOAD}"
    "printf_builtin"$'\t'"script"$'\t'"${PRINTF_BUILTIN}"
    "test_builtin"$'\t'"script"$'\t'"${TEST_BUILTIN}"
    "command_lookup"$'\t'"script"$'\t'"${COMMAND_LOOKUP}"
    "compgen_words"$'\t'"script"$'\t'"${COMPGEN_WORDS}"
    "redirections"$'\t'"script"$'\t'"${REDIRECTIONS}"
    "redirection_groups"$'\t'"script"$'\t'"${REDIRECTION_GROUPS}"
    "heredoc_read"$'\t'"script"$'\t'"${HEREDOC_READ}"
    "eval_parse"$'\t'"script"$'\t'"${EVAL_PARSE}"
    "subshells"$'\t'"script"$'\t'"${SUBSHELLS}"
    "background_wait"$'\t'"script"$'\t'"${BACKGROUND_WAIT}"
    "pipeline_builtin"$'\t'"script"$'\t'"${PIPELINE_BUILTIN}"
    "pipeline_external"$'\t'"script"$'\t'"${PIPELINE_EXTERNAL}"
    "process_subst"$'\t'"script"$'\t'"${PROCESS_SUBST}"
    "external_commands"$'\t'"script"$'\t'"${EXTERNAL_COMMANDS}"
    "shopt_options"$'\t'"script"$'\t'"${SHOPT_OPTIONS}"
    "positional_params"$'\t'"script"$'\t'"${POSITIONAL_PARAMS}"
    "getopts_parse"$'\t'"script"$'\t'"${GETOPTS_PARSE}"
    "trap_set"$'\t'"script"$'\t'"${TRAP_SET}"
    "cd_pwd"$'\t'"script"$'\t'"${CD_PWD}"
    "source_many_functions"$'\t'"script"$'\t'"${SOURCE_MANY_FUNCTIONS}"
    "source_many_assignments"$'\t'"script"$'\t'"${SOURCE_MANY_ASSIGNMENTS}"
    "glob_scan"$'\t'"script"$'\t'"${GLOB_SCAN}"
    "glob_array_capture"$'\t'"script"$'\t'"${GLOB_ARRAY_CAPTURE}"
    "globstar_scan"$'\t'"script"$'\t'"${GLOBSTAR_SCAN}"
)

SHELL_LABELS=("cherubsh" "${ORACLE_LABEL}")
SHELL_PATHS=("${CHERUBSH}" "${ORACLE}")

CASE_COUNT=0
validate_cases() {
    local case_row case_name mode payload
    local -A seen_cases=()
    for case_row in "${CASES[@]}"; do
        IFS=$'\t' read -r case_name mode payload <<<"${case_row}"
        if [[ -z "${case_name}" || -z "${mode}" || -z "${payload}" ]]; then
            echo "error: malformed benchmark case row: ${case_row}" >&2
            exit 2
        fi
        if [[ -n "${seen_cases[${case_name}]+x}" ]]; then
            echo "error: duplicate benchmark case name: ${case_name}" >&2
            exit 2
        fi
        seen_cases["${case_name}"]=1
        CASE_COUNT=$((CASE_COUNT + 1))
    done
}
validate_cases

RAW="${WS_ROOT}/target/bench/raw.tsv"
SUMMARY="${WS_ROOT}/target/bench/summary.tsv"
mkdir -p "$(dirname "${RAW}")"
printf 'case\tshell\trun\telapsed_ns\n' >"${RAW}"
printf 'case\tshell\tmedian_ms\tmin_ms\tmax_ms\t%s\n' "${RATIO_COLUMN}" >"${SUMMARY}"

run_case() {
    local shell_path="$1"
    local mode="$2"
    local payload="$3"
    local stderr_path="$4"

    case "${mode}" in
        cmd)
            env -i PATH="/usr/bin:/bin" HOME="${TMP}/home" LANG=C LC_ALL=C \
                BENCH_LIB="${LIB}" BENCH_GLOB_DIR="${TMP}/glob" BENCH_TMP="${TMP}" \
                "${shell_path}" --noprofile --norc -c "${payload}" \
                </dev/null >/dev/null 2>"${stderr_path}"
            ;;
        interactive_rc)
            if command -v setsid >/dev/null 2>&1; then
                setsid env -i PATH="/usr/bin:/bin" HOME="${TMP}/home" LANG=C LC_ALL=C \
                    BENCH_LIB="${LIB}" BENCH_GLOB_DIR="${TMP}/glob" BENCH_TMP="${TMP}" \
                    "${shell_path}" --noprofile --rcfile "${RC}" -i -c "${payload}" \
                    </dev/null >/dev/null 2>"${stderr_path}"
            else
                env -i PATH="/usr/bin:/bin" HOME="${TMP}/home" LANG=C LC_ALL=C \
                    BENCH_LIB="${LIB}" BENCH_GLOB_DIR="${TMP}/glob" BENCH_TMP="${TMP}" \
                    "${shell_path}" --noprofile --rcfile "${RC}" -i -c "${payload}" \
                    </dev/null >/dev/null 2>"${stderr_path}"
            fi
            ;;
        script)
            env -i PATH="/usr/bin:/bin" HOME="${TMP}/home" LANG=C LC_ALL=C \
                BENCH_LIB="${LIB}" BENCH_GLOB_DIR="${TMP}/glob" BENCH_TMP="${TMP}" \
                "${shell_path}" --noprofile --norc "${payload}" \
                </dev/null >/dev/null 2>"${stderr_path}"
            ;;
        *)
            echo "error: unknown benchmark mode ${mode}" >&2
            exit 2
            ;;
    esac
}

median_ns_for=()
min_ns_for=()
max_ns_for=()

case_index=0
echo ">> benchmarking RUNS=${RUNS} WARMUPS=${WARMUPS}"
echo ">> benchmark cases: ${CASE_COUNT}"
echo ">> raw samples: ${RAW}"
echo ">> summary:     ${SUMMARY}"
echo

for case_row in "${CASES[@]}"; do
    IFS=$'\t' read -r case_name mode payload <<<"${case_row}"
    echo ">> ${case_name}"
    for idx in "${!SHELL_LABELS[@]}"; do
        label="${SHELL_LABELS[$idx]}"
        shell_path="${SHELL_PATHS[$idx]}"
        stderr_path="${TMP}/${case_name}.${label}.stderr"

        for ((i = 0; i < WARMUPS; i++)); do
            run_case "${shell_path}" "${mode}" "${payload}" "${stderr_path}" || {
                echo "error: warmup failed for ${case_name}/${label}" >&2
                sed -n '1,80p' "${stderr_path}" >&2
                exit 1
            }
        done

        samples=()
        for ((i = 1; i <= RUNS; i++)); do
            start_ns="$(date +%s%N)"
            run_case "${shell_path}" "${mode}" "${payload}" "${stderr_path}" || {
                echo "error: run failed for ${case_name}/${label}" >&2
                sed -n '1,80p' "${stderr_path}" >&2
                exit 1
            }
            end_ns="$(date +%s%N)"
            elapsed_ns=$((end_ns - start_ns))
            samples+=("${elapsed_ns}")
            printf '%s\t%s\t%d\t%d\n' "${case_name}" "${label}" "${i}" "${elapsed_ns}" >>"${RAW}"
        done

        mapfile -t sorted < <(printf '%s\n' "${samples[@]}" | sort -n)
        min_ns="${sorted[0]}"
        max_ns="${sorted[$((RUNS - 1))]}"
        if (( RUNS % 2 == 1 )); then
            median_ns="${sorted[$((RUNS / 2))]}"
        else
            median_ns=$(((sorted[$((RUNS / 2 - 1))] + sorted[$((RUNS / 2))]) / 2))
        fi
        median_ns_for+=("${case_name}|${label}|${median_ns}")
        min_ns_for+=("${case_name}|${label}|${min_ns}")
        max_ns_for+=("${case_name}|${label}|${max_ns}")
    done
    case_index=$((case_index + 1))
done

lookup_ns() {
    local name="$1"
    local label="$2"
    shift 2
    local item
    for item in "$@"; do
        IFS='|' read -r item_name item_label item_value <<<"${item}"
        if [[ "${item_name}" == "${name}" && "${item_label}" == "${label}" ]]; then
            printf '%s\n' "${item_value}"
            return 0
        fi
    done
    return 1
}

ns_to_ms() {
    awk -v ns="$1" 'BEGIN { printf "%.3f", ns / 1000000.0 }'
}

ratio() {
    awk -v ns="$1" -v base="$2" 'BEGIN { if (base > 0) printf "%.2f", ns / base; else printf "n/a" }'
}

for case_row in "${CASES[@]}"; do
    IFS=$'\t' read -r case_name _mode _payload <<<"${case_row}"
    base_ns="$(lookup_ns "${case_name}" "${ORACLE_LABEL}" "${median_ns_for[@]}")"
    for label in "${SHELL_LABELS[@]}"; do
        med="$(lookup_ns "${case_name}" "${label}" "${median_ns_for[@]}")"
        min="$(lookup_ns "${case_name}" "${label}" "${min_ns_for[@]}")"
        max="$(lookup_ns "${case_name}" "${label}" "${max_ns_for[@]}")"
        printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
            "${case_name}" \
            "${label}" \
            "$(ns_to_ms "${med}")" \
            "$(ns_to_ms "${min}")" \
            "$(ns_to_ms "${max}")" \
            "$(ratio "${med}" "${base_ns}")" >>"${SUMMARY}"
    done
done

echo
echo "Timing summary, lower is better. Ratio < 1.00 means faster than ${ORACLE_LABEL}."
column -t -s $'\t' "${SUMMARY}"

echo
echo "Other factors:"
printf 'shell\tpath\tversion\tbinary_bytes\tpeak_rss_kb_source_many_functions\n'
for idx in "${!SHELL_LABELS[@]}"; do
    label="${SHELL_LABELS[$idx]}"
    shell_path="${SHELL_PATHS[$idx]}"
    version="$("${shell_path}" --version 2>/dev/null | head -n1 | tr '\t' ' ')"
    bytes="$(stat -c '%s' "${shell_path}" 2>/dev/null || printf 'n/a')"
    rss="n/a"
    if command -v /usr/bin/time >/dev/null 2>&1; then
        time_out="${TMP}/${label}.time"
        if /usr/bin/time -f '%M' -o "${time_out}" \
            env -i PATH="/usr/bin:/bin" HOME="${TMP}/home" LANG=C LC_ALL=C \
                BENCH_LIB="${LIB}" BENCH_GLOB_DIR="${TMP}/glob" BENCH_TMP="${TMP}" \
                "${shell_path}" --noprofile --norc "${SOURCE_MANY_FUNCTIONS}" \
                >/dev/null 2>"${TMP}/${label}.rss.stderr"; then
            rss="$(tr -d '[:space:]' <"${time_out}")"
        fi
    fi
    printf '%s\t%s\t%s\t%s\t%s\n' "${label}" "${shell_path}" "${version}" "${bytes}" "${rss}"
done | column -t -s $'\t'
