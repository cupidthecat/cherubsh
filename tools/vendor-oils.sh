#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# shellcheck source=../upstream.lock
source "${WS_ROOT}/upstream.lock"

EXPECTED_FILES=135
EXPECTED_CASES=2804

for command_name in git patch rg rsync; do
    if ! command -v "${command_name}" >/dev/null 2>&1; then
        echo "error: required command not found: ${command_name}" >&2
        exit 1
    fi
done

STAGING_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/cherubsh-oils.XXXXXX")"
trap 'rm -rf -- "${STAGING_ROOT}"' EXIT

CLONE_DIR="${STAGING_ROOT}/source"
VENDOR_DIR="${STAGING_ROOT}/vendor"

git clone --filter=blob:none --no-checkout "${OILS_REPOSITORY}" "${CLONE_DIR}"
git -C "${CLONE_DIR}" fetch --depth 1 origin "${OILS_COMMIT}"
ACTUAL_COMMIT="$(git -C "${CLONE_DIR}" rev-parse FETCH_HEAD)"
if [[ "${ACTUAL_COMMIT}" != "${OILS_COMMIT}" ]]; then
    echo "error: Oils resolved to ${ACTUAL_COMMIT}, expected ${OILS_COMMIT}" >&2
    exit 1
fi
git -C "${CLONE_DIR}" checkout --detach "${ACTUAL_COMMIT}"

mkdir -p "${VENDOR_DIR}"
rsync -a "${CLONE_DIR}/spec/" "${VENDOR_DIR}/spec/"
cp "${CLONE_DIR}/LICENSE.txt" "${VENDOR_DIR}/LICENSE.txt"
patch --batch --forward --directory "${VENDOR_DIR}" --strip 1 \
    < "${WS_ROOT}/tools/oils-python3.patch"

mapfile -t SELECTED_FILES < <(
    rg --files-with-matches '^## compare_shells:.*bash' \
        "${VENDOR_DIR}/spec" --glob '*.test.sh' | sort
)
FILE_COUNT=${#SELECTED_FILES[@]}
CASE_COUNT=0
for file in "${SELECTED_FILES[@]}"; do
    COUNT="$(rg --count '^#### ' "${file}" || true)"
    CASE_COUNT=$((CASE_COUNT + COUNT))
done

if [[ ${FILE_COUNT} -ne ${EXPECTED_FILES} ]] || [[ ${CASE_COUNT} -ne ${EXPECTED_CASES} ]]; then
    echo "error: Oils inventory is ${FILE_COUNT} files and ${CASE_COUNT} cases; expected ${EXPECTED_FILES} and ${EXPECTED_CASES}" >&2
    exit 1
fi

mkdir -p "${WS_ROOT}/vendor/oils"
rsync -a --delete "${VENDOR_DIR}/spec/" "${WS_ROOT}/vendor/oils/spec/"
cp "${VENDOR_DIR}/LICENSE.txt" "${WS_ROOT}/vendor/oils/LICENSE.txt"
echo "vendored Oils ${OILS_COMMIT}: ${FILE_COUNT} Bash files, ${CASE_COUNT} cases"
