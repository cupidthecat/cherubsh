#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RELEASE_TAG_VALUE="${1:-${RELEASE_TAG:-${GITHUB_REF_NAME:-}}}"

if [[ ! "${RELEASE_TAG_VALUE}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: release tag must use vX.Y.Z" >&2
    exit 2
fi

PACKAGE_VERSION="$({
    cd "${WS_ROOT}"
    cargo metadata --locked --no-deps --format-version 1
} | python3 -c '
import json
import sys

packages = [
    package for package in json.load(sys.stdin)["packages"]
    if package["name"] == "cherubsh"
]
if len(packages) != 1:
    raise SystemExit("error: Cargo metadata must contain exactly one cherubsh package")
print(packages[0]["version"])
')"
EXPECTED_TAG="v${PACKAGE_VERSION}"

if [[ "${RELEASE_TAG_VALUE}" != "${EXPECTED_TAG}" ]]; then
    echo "error: release tag ${RELEASE_TAG_VALUE} does not match Cargo ${PACKAGE_VERSION}" >&2
    exit 1
fi

printf 'release tag %s matches Cargo %s\n' "${RELEASE_TAG_VALUE}" "${PACKAGE_VERSION}"
