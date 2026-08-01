#!/usr/bin/env bash
# Create a portable CherubSH archive and its SHA-256 checksum.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VERSION=""
BINARY="${WORKSPACE_ROOT}/target/release/cherubsh"
OUTPUT_DIR="${WORKSPACE_ROOT}/dist"

usage() {
    cat <<'EOF'
usage: tools/package-release.sh --version VERSION [--binary FILE] [--output DIR]

Builds a tarball containing CherubSH, its manuals, command completion, license,
README, starter configuration, and prefix-aware installer. The output directory
also receives a SHA256SUMS file.
EOF
}

while (($#)); do
    case "$1" in
        --version)
            if (($# < 2)); then
                echo "error: --version needs a value" >&2
                exit 2
            fi
            VERSION="$2"
            shift 2
            ;;
        --binary)
            if (($# < 2)); then
                echo "error: --binary needs a file name" >&2
                exit 2
            fi
            BINARY="$2"
            shift 2
            ;;
        --output)
            if (($# < 2)); then
                echo "error: --output needs a directory name" >&2
                exit 2
            fi
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ ! "${VERSION}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
    echo "error: --version must use letters, digits, dots, underscores, or hyphens" >&2
    exit 2
fi
if [[ ! -x "${BINARY}" ]]; then
    echo "error: CherubSH binary is not executable: ${BINARY}" >&2
    exit 2
fi

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "${WORKSPACE_ROOT}" log -1 --format=%ct)}"
if [[ ! "${SOURCE_DATE_EPOCH}" =~ ^[0-9]+$ ]]; then
    echo "error: SOURCE_DATE_EPOCH must be a Unix timestamp" >&2
    exit 2
fi

case "$(uname -m)" in
    x86_64) PLATFORM="x86_64-unknown-linux-gnu" ;;
    aarch64) PLATFORM="aarch64-unknown-linux-gnu" ;;
    *)
        echo "error: unsupported release architecture: $(uname -m)" >&2
        exit 2
        ;;
esac

PACKAGE_NAME="cherubsh-${VERSION}-${PLATFORM}"
ARCHIVE_NAME="${PACKAGE_NAME}.tar.gz"
mkdir -p "${OUTPUT_DIR}"
OUTPUT_DIR="$(cd "${OUTPUT_DIR}" && pwd -P)"
ARCHIVE_PATH="${OUTPUT_DIR}/${ARCHIVE_NAME}"
CHECKSUM_PATH="${OUTPUT_DIR}/SHA256SUMS"
if [[ -e "${ARCHIVE_PATH}" || -e "${CHECKSUM_PATH}" ]]; then
    echo "error: release files already exist in ${OUTPUT_DIR}; choose an empty directory" >&2
    exit 1
fi

TEMPORARY_DIRECTORY="$(mktemp -d "${TMPDIR:-/tmp}/cherubsh-package.XXXXXX")"
cleanup() {
    rm -rf "${TEMPORARY_DIRECTORY}"
}
trap cleanup EXIT

PACKAGE_DIRECTORY="${TEMPORARY_DIRECTORY}/${PACKAGE_NAME}"
mkdir -p \
    "${PACKAGE_DIRECTORY}/completions" \
    "${PACKAGE_DIRECTORY}/examples" \
    "${PACKAGE_DIRECTORY}/man" \
    "${PACKAGE_DIRECTORY}/manifests" \
    "${PACKAGE_DIRECTORY}/tools"
install -m 0755 "${BINARY}" "${PACKAGE_DIRECTORY}/cherubsh"
install -m 0644 "${WORKSPACE_ROOT}/LICENSE" "${PACKAGE_DIRECTORY}/LICENSE"
install -m 0644 "${WORKSPACE_ROOT}/README.md" "${PACKAGE_DIRECTORY}/README.md"
install -m 0644 "${WORKSPACE_ROOT}/man/cherubsh.1" "${PACKAGE_DIRECTORY}/man/cherubsh.1"
install -m 0644 "${WORKSPACE_ROOT}/man/cherubsh-readline.3" "${PACKAGE_DIRECTORY}/man/cherubsh-readline.3"
install -m 0644 "${WORKSPACE_ROOT}/man/cherubshrc.5" "${PACKAGE_DIRECTORY}/man/cherubshrc.5"
install -m 0644 "${WORKSPACE_ROOT}/completions/cherubsh" "${PACKAGE_DIRECTORY}/completions/cherubsh"
install -m 0644 "${WORKSPACE_ROOT}/packaging/cherubsh.files" "${PACKAGE_DIRECTORY}/manifests/cherubsh.files"
install -m 0644 "${WORKSPACE_ROOT}/examples/cherubrc" "${PACKAGE_DIRECTORY}/examples/cherubrc"
install -m 0755 "${WORKSPACE_ROOT}/tools/install-cherubsh.sh" "${PACKAGE_DIRECTORY}/tools/install-cherubsh.sh"
install -m 0755 "${WORKSPACE_ROOT}/tools/install-cherubrc.sh" "${PACKAGE_DIRECTORY}/tools/install-cherubrc.sh"

(
    cd "${TEMPORARY_DIRECTORY}"
    tar \
        --sort=name \
        --mtime="@${SOURCE_DATE_EPOCH}" \
        --owner=0 \
        --group=0 \
        --numeric-owner \
        --format=posix \
        --pax-option=delete=atime,delete=ctime \
        -cf - "${PACKAGE_NAME}" | gzip -n >"${ARCHIVE_PATH}"
)
(
    cd "${OUTPUT_DIR}"
    sha256sum "${ARCHIVE_NAME}" >"$(basename "${CHECKSUM_PATH}")"
)

printf 'Created %s\n' "${ARCHIVE_PATH}"
printf 'Created %s\n' "${CHECKSUM_PATH}"
