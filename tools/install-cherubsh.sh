#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ACTION=""
PREFIX="/usr/local"
STAGING_ROOT="${DESTDIR:-}"

usage() {
    cat <<'EOF'
usage: tools/install-cherubsh.sh ACTION [OPTIONS]

Install or uninstall CherubSH and the user-facing files from its release
archive. ACTION must be install or uninstall.

Options:
  --prefix PATH   installation prefix (default: /usr/local)
  --destdir PATH  staging root prepended to the prefix
  -h, --help      show this help
EOF
}

if (($#)); then
    case "$1" in
        install|uninstall)
            ACTION="$1"
            shift
            ;;
    esac
fi

while (($#)); do
    case "$1" in
        --prefix)
            (($# >= 2)) || { echo "error: --prefix needs a path" >&2; exit 2; }
            PREFIX="$2"
            shift 2
            ;;
        --destdir)
            (($# >= 2)) || { echo "error: --destdir needs a path" >&2; exit 2; }
            STAGING_ROOT="$2"
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

if [[ -z "${ACTION}" ]]; then
    echo "error: ACTION must be install or uninstall" >&2
    usage >&2
    exit 2
fi
if [[ "${PREFIX}" != /* || "/${PREFIX#/}/" == *"/../"* ]]; then
    echo "error: --prefix must be an absolute path without .. components" >&2
    exit 2
fi
if [[ -n "${STAGING_ROOT}" &&
      ("${STAGING_ROOT}" != /* || "/${STAGING_ROOT#/}/" == *"/../"*) ]]; then
    echo "error: --destdir must be an absolute path without .. components" >&2
    exit 2
fi

if [[ "${PREFIX}" != "/" ]]; then
    PREFIX="${PREFIX%/}"
fi
STAGING_ROOT="${STAGING_ROOT%/}"
INSTALL_ROOT="${STAGING_ROOT}${PREFIX}"
PACKAGE_MANIFEST="${PACKAGE_ROOT}/manifests/cherubsh.files"
OWNERSHIP_DIRECTORY="${INSTALL_ROOT%/}/share/cherubsh/package"
OWNERSHIP_MANIFEST="${OWNERSHIP_DIRECTORY}/cherubsh.manifest"
TRANSACTION_DIRECTORY=""
TRANSACTION_ACTIVE=0
declare -a TRANSACTION_DESTINATIONS=()
declare -a TRANSACTION_BACKUPS=()
declare -a TRANSACTION_DIRECTORIES=()

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

preflight_install() {
    local source_path destination_path source destination
    [[ -f "${PACKAGE_MANIFEST}" ]] || {
        echo "error: missing ${PACKAGE_MANIFEST}" >&2
        exit 1
    }
    while read -r source_path destination_path; do
        [[ -n "${source_path}" ]] || continue
        validate_manifest_entry "${source_path}" "${destination_path}"
        source="${PACKAGE_ROOT}/${source_path}"
        destination="${INSTALL_ROOT%/}/${destination_path}"
        [[ -f "${source}" ]] || {
            echo "error: package file is missing: ${source_path}" >&2
            exit 1
        }
        if [[ -e "${destination}" || -L "${destination}" ]]; then
            if [[ ! -f "${OWNERSHIP_MANIFEST}" ]] ||
               ! grep -Fqx -- "${destination_path}" "${OWNERSHIP_MANIFEST}"; then
                echo "error: refusing to replace unowned file: ${destination}" >&2
                exit 1
            fi
        fi
    done <"${PACKAGE_MANIFEST}"
}

record_missing_directories() {
    local directory="$1"
    while [[ "${directory}" != "${INSTALL_ROOT}" &&
             "${directory}" == "${INSTALL_ROOT%/}/"* &&
             ! -e "${directory}" && ! -L "${directory}" ]]; do
        TRANSACTION_DIRECTORIES+=("${directory}")
        directory="$(dirname "${directory}")"
    done
}

track_destination() {
    local destination="$1"
    local backup=""
    local index="${#TRANSACTION_DESTINATIONS[@]}"
    if [[ -e "${destination}" || -L "${destination}" ]]; then
        backup="${TRANSACTION_DIRECTORY}/backups/${index}"
        mkdir -p "$(dirname "${backup}")"
        cp -a -- "${destination}" "${backup}"
    fi
    TRANSACTION_DESTINATIONS+=("${destination}")
    TRANSACTION_BACKUPS+=("${backup}")
}

rollback_install() {
    local index destination backup directory
    set +e
    for ((index = ${#TRANSACTION_DESTINATIONS[@]} - 1; index >= 0; index--)); do
        destination="${TRANSACTION_DESTINATIONS[index]}"
        backup="${TRANSACTION_BACKUPS[index]}"
        rm -f -- "${destination}"
        if [[ -n "${backup}" ]]; then
            cp -a -- "${backup}" "${destination}"
        fi
    done
    if ((${#TRANSACTION_DIRECTORIES[@]})); then
        printf '%s\n' "${TRANSACTION_DIRECTORIES[@]}" | sort -ru | while read -r directory; do
            rmdir --ignore-fail-on-non-empty -- "${directory}" 2>/dev/null || true
        done
    fi
    if [[ -n "${TRANSACTION_DIRECTORY}" && -d "${TRANSACTION_DIRECTORY}" ]]; then
        rm -rf -- "${TRANSACTION_DIRECTORY}"
    fi
}

rollback_on_exit() {
    local status=$?
    trap - EXIT INT TERM
    if ((TRANSACTION_ACTIVE)); then
        rollback_install
    fi
    exit "${status}"
}

install_package() {
    local temporary_manifest source_path destination_path source destination
    TRANSACTION_DIRECTORY="$(mktemp -d)"
    TRANSACTION_ACTIVE=1
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap rollback_on_exit EXIT
    record_missing_directories "${OWNERSHIP_DIRECTORY}"
    mkdir -p "${OWNERSHIP_DIRECTORY}"
    temporary_manifest="${TRANSACTION_DIRECTORY}/cherubsh.manifest"
    while read -r source_path destination_path; do
        [[ -n "${source_path}" ]] || continue
        source="${PACKAGE_ROOT}/${source_path}"
        destination="${INSTALL_ROOT%/}/${destination_path}"
        record_missing_directories "$(dirname "${destination}")"
        mkdir -p "$(dirname "${destination}")"
        track_destination "${destination}"
        case "${destination_path}" in
            bin/*) install -m 0755 "${source}" "${destination}" ;;
            *) install -m 0644 "${source}" "${destination}" ;;
        esac
        printf '%s\n' "${destination_path}" >>"${temporary_manifest}"
    done <"${PACKAGE_MANIFEST}"
    track_destination "${OWNERSHIP_MANIFEST}"
    install -m 0644 "${temporary_manifest}" "${OWNERSHIP_MANIFEST}"
    TRANSACTION_ACTIVE=0
    trap - EXIT INT TERM
    rm -rf -- "${TRANSACTION_DIRECTORY}"
    printf 'Installed CherubSH under %s\n' "${INSTALL_ROOT}"
}

uninstall_package() {
    local destination_path
    local -a directories=()
    if [[ ! -f "${OWNERSHIP_MANIFEST}" ]]; then
        printf 'No installed CherubSH files found under %s\n' "${INSTALL_ROOT}"
        return
    fi
    while read -r destination_path; do
        [[ -n "${destination_path}" ]] || continue
        validate_manifest_entry owned "${destination_path}"
        rm -f -- "${INSTALL_ROOT%/}/${destination_path}"
        directories+=("$(dirname "${INSTALL_ROOT%/}/${destination_path}")")
    done <"${OWNERSHIP_MANIFEST}"
    rm -f -- "${OWNERSHIP_MANIFEST}"
    directories+=("${OWNERSHIP_DIRECTORY}")
    printf '%s\n' "${directories[@]}" | sort -ru | while read -r directory; do
        rmdir --ignore-fail-on-non-empty -- "${directory}" 2>/dev/null || true
    done
    printf 'Uninstalled CherubSH from %s\n' "${INSTALL_ROOT}"
}

if [[ "${ACTION}" == "install" ]]; then
    preflight_install
    install_package
else
    uninstall_package
fi
