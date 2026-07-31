#!/usr/bin/env cherubsh
set -euo pipefail

complete -W 'build test clean' cherub-task

printf 'completion candidates for "t":\n'
compgen -W 'build test clean' -- t

history -c
history -s 'cherub-task test'
history -s 'cherub-task build'

printf 'history entries:\n'
history | sed -E 's/^[[:space:]]*[0-9]+[[:space:]]*//'
