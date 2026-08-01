#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ACTION=""
COMPONENT="all"
PREFIX="/usr/local"
STAGING_ROOT="${DESTDIR:-}"

usage() {
    cat <<'EOF'
usage: tools/install-readline-dev.sh ACTION [OPTIONS]

Install or uninstall the Readline-compatible development files from this
archive. ACTION must be install or uninstall.

Options:
  --component NAME  install readline, history, or all files (default: all)
  --prefix PATH     installation prefix (default: /usr/local)
  --destdir PATH    staging root prepended to the prefix
  -h, --help        show this help
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
        --component)
            (($# >= 2)) || { echo "error: --component needs a value" >&2; exit 2; }
            COMPONENT="$2"
            shift 2
            ;;
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
case "${COMPONENT}" in
    readline|history) COMPONENTS=("${COMPONENT}") ;;
    all) COMPONENTS=(readline history) ;;
    *) echo "error: --component must be readline, history, or all" >&2; exit 2 ;;
esac
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
OWNERSHIP_DIRECTORY="${INSTALL_ROOT%/}/share/cherubsh/readline-dev"

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

preflight_component() {
    local component="$1"
    local package_manifest="${PACKAGE_ROOT}/manifests/${component}.files"
    local ownership_manifest="${OWNERSHIP_DIRECTORY}/${component}.manifest"
    local source_path destination_path source destination

    [[ -f "${package_manifest}" ]] || { echo "error: missing ${package_manifest}" >&2; exit 1; }
    while read -r source_path destination_path; do
        [[ -n "${source_path}" ]] || continue
        validate_manifest_entry "${source_path}" "${destination_path}"
        source="${PACKAGE_ROOT}/${source_path}"
        destination="${INSTALL_ROOT%/}/${destination_path}"
        [[ -e "${source}" || -L "${source}" ]] || {
            echo "error: package file is missing: ${source_path}" >&2
            exit 1
        }
        if [[ -e "${destination}" || -L "${destination}" ]]; then
            if [[ ! -f "${ownership_manifest}" ]] ||
               ! grep -Fqx -- "${destination_path}" "${ownership_manifest}"; then
                echo "error: refusing to replace unowned file: ${destination}" >&2
                exit 1
            fi
        fi
    done <"${package_manifest}"
}

install_component() {
    local component="$1"
    local package_manifest="${PACKAGE_ROOT}/manifests/${component}.files"
    local ownership_manifest="${OWNERSHIP_DIRECTORY}/${component}.manifest"
    local temporary_manifest
    local source_path destination_path source destination

    mkdir -p "${OWNERSHIP_DIRECTORY}"
    temporary_manifest="$(mktemp "${ownership_manifest}.tmp.XXXXXX")"
    while read -r source_path destination_path; do
        [[ -n "${source_path}" ]] || continue
        validate_manifest_entry "${source_path}" "${destination_path}"
        source="${PACKAGE_ROOT}/${source_path}"
        destination="${INSTALL_ROOT%/}/${destination_path}"
        mkdir -p "$(dirname "${destination}")"
        if [[ -L "${source}" ]]; then
            ln -sfn "$(readlink "${source}")" "${destination}"
        else
            case "${source_path}" in
                lib/*.so.*) install -m 0755 "${source}" "${destination}" ;;
                *) install -m 0644 "${source}" "${destination}" ;;
            esac
        fi
        printf '%s\n' "${destination_path}" >>"${temporary_manifest}"
    done <"${package_manifest}"
    install -m 0644 "${temporary_manifest}" "${ownership_manifest}"
    rm -f "${temporary_manifest}"
    printf 'Installed %s development files under %s\n' "${component}" "${INSTALL_ROOT}"
}

uninstall_component() {
    local component="$1"
    local ownership_manifest="${OWNERSHIP_DIRECTORY}/${component}.manifest"
    local -a directories=()

    if [[ ! -f "${ownership_manifest}" ]]; then
        printf 'No installed %s development files found under %s\n' "${component}" "${INSTALL_ROOT}"
        return
    fi
    while read -r destination_path; do
        [[ -n "${destination_path}" ]] || continue
        validate_manifest_entry "owned" "${destination_path}"
        rm -f -- "${INSTALL_ROOT%/}/${destination_path}"
        directories+=("$(dirname "${INSTALL_ROOT%/}/${destination_path}")")
    done <"${ownership_manifest}"
    rm -f -- "${ownership_manifest}"
    directories+=("${OWNERSHIP_DIRECTORY}")
    if ((${#directories[@]})); then
        printf '%s\n' "${directories[@]}" | sort -ru | while read -r directory; do
            rmdir --ignore-fail-on-non-empty -- "${directory}" 2>/dev/null || true
        done
    fi
    printf 'Uninstalled %s development files from %s\n' "${component}" "${INSTALL_ROOT}"
}

if [[ "${ACTION}" == "install" ]]; then
    for component in "${COMPONENTS[@]}"; do
        preflight_component "${component}"
    done
fi
for component in "${COMPONENTS[@]}"; do
    "${ACTION}_component" "${component}"
done
