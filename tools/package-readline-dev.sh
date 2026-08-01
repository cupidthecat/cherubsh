#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VERSION=""
INPUT_ROOT="${WORKSPACE_ROOT}/target/readline"
OUTPUT_DIR="${WORKSPACE_ROOT}/dist"

usage() {
    cat <<'EOF'
usage: tools/package-readline-dev.sh --version VERSION [--input DIR] [--output DIR]

Build a development archive with Readline and History headers, shared and
static libraries, pkg-config metadata, component manifests, and the installer.
EOF
}

while (($#)); do
    case "$1" in
        --version)
            (($# >= 2)) || { echo "error: --version needs a value" >&2; exit 2; }
            VERSION="$2"
            shift 2
            ;;
        --input)
            (($# >= 2)) || { echo "error: --input needs a directory" >&2; exit 2; }
            INPUT_ROOT="$2"
            shift 2
            ;;
        --output)
            (($# >= 2)) || { echo "error: --output needs a directory" >&2; exit 2; }
            OUTPUT_DIR="$2"
            shift 2
            ;;
        -h|--help)
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

SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "${WORKSPACE_ROOT}" log -1 --format=%ct)}"
if [[ ! "${SOURCE_DATE_EPOCH}" =~ ^[0-9]+$ ]]; then
    echo "error: SOURCE_DATE_EPOCH must be a Unix timestamp" >&2
    exit 2
fi
case "$(uname -m)" in
    x86_64) PLATFORM="x86_64-unknown-linux-gnu" ;;
    aarch64) PLATFORM="aarch64-unknown-linux-gnu" ;;
    *) echo "error: unsupported release architecture: $(uname -m)" >&2; exit 2 ;;
esac

PACKAGE_NAME="cherubsh-readline-dev-${VERSION}-${PLATFORM}"
ARCHIVE_NAME="${PACKAGE_NAME}.tar.gz"
mkdir -p "${OUTPUT_DIR}"
OUTPUT_DIR="$(cd "${OUTPUT_DIR}" && pwd -P)"
ARCHIVE_PATH="${OUTPUT_DIR}/${ARCHIVE_NAME}"
if [[ -e "${ARCHIVE_PATH}" ]]; then
    echo "error: development archive already exists: ${ARCHIVE_PATH}" >&2
    exit 1
fi

TEMPORARY_DIRECTORY="$(mktemp -d "${TMPDIR:-/tmp}/cherubsh-readline-package.XXXXXX")"
cleanup() {
    rm -rf "${TEMPORARY_DIRECTORY}"
}
trap cleanup EXIT

PACKAGE_DIRECTORY="${TEMPORARY_DIRECTORY}/${PACKAGE_NAME}"
mkdir -p "${PACKAGE_DIRECTORY}/examples" "${PACKAGE_DIRECTORY}/tools" "${PACKAGE_DIRECTORY}/manifests"

validate_manifest_entry() {
    local source_path="$1"
    local destination_path="$2"
    if [[ -z "${source_path}" || -z "${destination_path}" ||
          "${source_path}" == /* || "${destination_path}" == /* ||
          "/${source_path}/" == *"/../"* || "/${destination_path}/" == *"/../"* ]]; then
        echo "error: unsafe package manifest entry: ${source_path} ${destination_path}" >&2
        exit 2
    fi
}

copy_manifest_payload() {
    local manifest="$1"
    local source_path destination_path source destination
    while read -r source_path destination_path; do
        [[ -n "${source_path}" ]] || continue
        validate_manifest_entry "${source_path}" "${destination_path}"
        if [[ "${source_path}" == "LICENSE" ]]; then
            source="${WORKSPACE_ROOT}/LICENSE"
        else
            source="${INPUT_ROOT}/${source_path}"
        fi
        if [[ ! -f "${source}" && ! -L "${source}" ]]; then
            echo "error: development input is missing ${source_path}" >&2
            exit 1
        fi
        destination="${PACKAGE_DIRECTORY}/${source_path}"
        if [[ -e "${destination}" || -L "${destination}" ]]; then
            continue
        fi
        mkdir -p "$(dirname "${destination}")"
        if [[ -L "${source}" ]]; then
            cp -a "${source}" "${destination}"
        else
            case "${source_path}" in
                lib/*.so.*) install -m 0755 "${source}" "${destination}" ;;
                *) install -m 0644 "${source}" "${destination}" ;;
            esac
        fi
    done <"${manifest}"
}

copy_manifest_payload "${WORKSPACE_ROOT}/packaging/readline.files"
copy_manifest_payload "${WORKSPACE_ROOT}/packaging/history.files"
install -m 0644 "${WORKSPACE_ROOT}/examples/readline-client.c" "${PACKAGE_DIRECTORY}/examples/readline-client.c"
install -m 0755 "${WORKSPACE_ROOT}/tools/install-readline-dev.sh" "${PACKAGE_DIRECTORY}/tools/install-readline-dev.sh"
install -m 0644 "${WORKSPACE_ROOT}/packaging/readline.files" "${PACKAGE_DIRECTORY}/manifests/readline.files"
install -m 0644 "${WORKSPACE_ROOT}/packaging/history.files" "${PACKAGE_DIRECTORY}/manifests/history.files"

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

printf 'Created %s\n' "${ARCHIVE_PATH}"
