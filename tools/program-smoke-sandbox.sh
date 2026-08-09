#!/bin/sh
set -eu

revision=$1
mode=$2
entrypoint=$3
shift 3

mkdir -p /tmp/source /tmp/work/home
git --git-dir=/mnt archive "$revision" | tar -xf - -C /tmp/source

export BASH_IT=/tmp/source
export CI=1
export GVM_ROOT=/tmp/source
export HOME=/tmp/work/home
export LANG=C
export LC_ALL=C
export NO_COLOR=1
export NVM_DIR=/tmp/source
export OMB=/tmp/source
export OSH=/tmp/source
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export SDKMAN_DIR=/tmp/source
export SHELL=/bin/bash
export TERM=dumb

cd /tmp/work
trace_flag=
if [ "${SMOKE_TRACE:-0}" = 1 ]; then
  trace_flag=-x
  # shellcheck disable=SC2016 # Expand PATH when the generated environment file is sourced.
  printf '%s\n' 'set -x' 'printf "SMOKE_CHILD_PATH=%s\n" "$PATH" >&2' > /tmp/work/smoke-bash-env
  export BASH_ENV=/tmp/work/smoke-bash-env
fi
case "$mode" in
  command)
    exec /bin/bash --noprofile --norc $trace_flag "/tmp/source/$entrypoint" "$@"
    ;;
  interactive)
    if timeout --signal=KILL 5 \
      script -qefc "/bin/bash --noprofile --norc $trace_flag /tmp/source/$entrypoint" /dev/null \
      2>/dev/null
    then
      exit 0
    else
      status=$?
    fi
    if [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then
      exit 0
    fi
    exit "$status"
    ;;
  source)
    exec /bin/bash --noprofile --norc $trace_flag -c 'source "$1"' smoke "/tmp/source/$entrypoint"
    ;;
  *)
    printf 'unsupported smoke mode: %s\n' "$mode" >&2
    exit 64
    ;;
esac
