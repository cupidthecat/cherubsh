#!/usr/bin/env bash
# Copy the starter CherubSH configuration without replacing an existing file.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEMPLATE="${WORKSPACE_ROOT}/examples/cherubrc"
TARGET="${HOME:-}/.cherubrc"

usage() {
    cat <<'EOF'
usage: tools/install-cherubrc.sh [--path FILE]

Copies the starter CherubSH configuration to FILE. The default is ~/.cherubrc.
The command stops if the destination already exists.
EOF
}

while (($#)); do
    case "$1" in
        --path)
            if (($# < 2)); then
                echo "error: --path needs a file name" >&2
                exit 2
            fi
            TARGET="$2"
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

if [[ -z "${TARGET}" || "${TARGET}" == "/.cherubrc" ]]; then
    echo "error: HOME is not set; pass --path FILE instead" >&2
    exit 2
fi

if [[ -e "${TARGET}" || -L "${TARGET}" ]]; then
    echo "error: ${TARGET} already exists; it was not changed" >&2
    exit 1
fi

PARENT="$(dirname "${TARGET}")"
if [[ ! -d "${PARENT}" ]]; then
    echo "error: parent directory does not exist: ${PARENT}" >&2
    exit 1
fi

TEMPORARY="$(mktemp "${PARENT}/.cherubsh-install.XXXXXX")"
cleanup() {
    rm -f -- "${TEMPORARY}"
}
trap cleanup EXIT

install -m 0644 "${TEMPLATE}" "${TEMPORARY}"
if ! ln -T -- "${TEMPORARY}" "${TARGET}"; then
    if [[ -e "${TARGET}" || -L "${TARGET}" ]]; then
        echo "error: ${TARGET} already exists; it was not changed" >&2
    else
        echo "error: could not create ${TARGET} without replacing another file" >&2
    fi
    exit 1
fi
rm -f -- "${TEMPORARY}"
trap - EXIT

printf 'Created %s from %s\n' "${TARGET}" "${TEMPLATE}"
