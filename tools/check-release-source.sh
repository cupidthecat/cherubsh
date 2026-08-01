#!/usr/bin/env bash

set -euo pipefail

RELEASE_REPOSITORY_VALUE="${RELEASE_REPOSITORY:-${PWD}}"
PROTECTED_MAIN_REF="${RELEASE_MAIN_REF:-refs/remotes/origin/main}"
RELEASE_COMMIT_REF="${1:-HEAD}"

if ! RELEASE_COMMIT="$(git -C "${RELEASE_REPOSITORY_VALUE}" rev-parse --verify "${RELEASE_COMMIT_REF}^{commit}" 2>/dev/null)"; then
    echo "error: release commit ${RELEASE_COMMIT_REF} does not exist" >&2
    exit 2
fi

if ! MAIN_COMMIT="$(git -C "${RELEASE_REPOSITORY_VALUE}" rev-parse --verify "${PROTECTED_MAIN_REF}^{commit}" 2>/dev/null)"; then
    echo "error: protected main reference ${PROTECTED_MAIN_REF} does not exist" >&2
    exit 2
fi

if ! git -C "${RELEASE_REPOSITORY_VALUE}" merge-base --is-ancestor "${RELEASE_COMMIT}" "${MAIN_COMMIT}"; then
    echo "error: release commit ${RELEASE_COMMIT} is not reachable from protected main ${MAIN_COMMIT}" >&2
    exit 1
fi

printf 'release commit %s is reachable from protected main %s\n' \
    "${RELEASE_COMMIT}" "${MAIN_COMMIT}"
