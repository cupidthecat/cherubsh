#!/usr/bin/env cherubsh
set -euo pipefail

name=${1:-cherub}
printf 'upper-ish label: %s\n' "${name^^}"
printf 'default fallback: %s\n' "${MISSING_VALUE:-fallback}"
printf 'substring: %s\n' "${name:0:4}"

printf 'brace expansion:'
for path in src/{lexer,parser,expander}; do
    printf ' %s' "$path"
done
printf '\n'

captured=$(
    printf 'command substitution works for %s\n' "$name"
)
printf '%s\n' "$captured"

tmp=${TMPDIR:-/tmp}/cherubsh-example-$$.txt
trap 'rm -f "$tmp"' EXIT

cat >"$tmp" <<EOF
name=$name
shell=${SHELL:-unknown}
pid=$$
EOF

printf 'here-document file:\n'
sed 's/^/  /' "$tmp"

printf 'process substitution diff:\n'
if diff <(printf 'same\n') <(printf 'same\n') >/dev/null; then
    printf '  process substitution matched\n'
fi
