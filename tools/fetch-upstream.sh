#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WS_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CACHE_ROOT="${UPSTREAM_CACHE_DIR:-${WS_ROOT}/target/upstream}"
DOWNLOAD_DIR="${CACHE_ROOT}/downloads"

# shellcheck source=../upstream.lock
source "${WS_ROOT}/upstream.lock"

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: required command not found: $1" >&2
        exit 1
    fi
}

for command_name in git curl sha256sum patch gpgv; do
    require_command "${command_name}"
done

mkdir -p "${DOWNLOAD_DIR}"

GNU_DOWNLOAD_BASES=(
    "https://ftpmirror.gnu.org"
    "https://ftp.gnu.org/gnu"
    "https://mirrors.kernel.org/gnu"
)
GNU_DOWNLOAD_BASE=""

download() {
    local path=$1
    local name=$2
    local destination="${DOWNLOAD_DIR}/${name}"
    local temporary="${destination}.part"
    local base
    local -a candidates=("${GNU_DOWNLOAD_BASES[@]}")
    local -A tried=()

    if [[ -f "${destination}" ]]; then
        return
    fi

    if [[ -n "${GNU_DOWNLOAD_BASE}" ]]; then
        candidates=(
            "${GNU_DOWNLOAD_BASE}"
            "${GNU_DOWNLOAD_BASES[@]}"
        )
    fi

    for base in "${candidates[@]}"; do
        if [[ -n "${tried[${base}]+present}" ]]; then
            continue
        fi
        tried["${base}"]=1
        rm -f -- "${temporary}"
        printf 'downloading %s from %s\n' "${name}" "${base}"
        if curl \
            --fail \
            --location \
            --silent \
            --show-error \
            --retry 2 \
            --retry-all-errors \
            --retry-delay 2 \
            --connect-timeout 15 \
            --max-time 120 \
            --output "${temporary}" \
            "${base}/${path}"; then
            mv -f -- "${temporary}" "${destination}"
            GNU_DOWNLOAD_BASE="${base}"
            return
        fi
    done

    rm -f -- "${temporary}"
    echo "error: could not download ${name} from a GNU mirror" >&2
    return 1
}

for patch_number in $(seq 1 "${BASH_PATCHLEVEL}"); do
    patch_name="$(printf 'bash53-%03d' "${patch_number}")"
    download "bash/bash-5.3-patches/${patch_name}" "${patch_name}"
    download "bash/bash-5.3-patches/${patch_name}.sig" "${patch_name}.sig"
done

for patch_number in $(seq 1 "${READLINE_PATCHLEVEL}"); do
    patch_name="$(printf 'readline83-%03d' "${patch_number}")"
    download "readline/readline-8.3-patches/${patch_name}" "${patch_name}"
    download "readline/readline-8.3-patches/${patch_name}.sig" "${patch_name}.sig"
done

download "gnu-keyring.gpg" "gnu-keyring.gpg"

(
    cd "${DOWNLOAD_DIR}"
    sha256sum --check "${WS_ROOT}/upstream.sha256"
)

verify_signatures() {
    local prefix=$1
    local patch_level=$2
    local release_prefix=$3
    for patch_number in $(seq 1 "${patch_level}"); do
        patch_name="$(printf '%s-%03d' "${prefix}" "${patch_number}")"
        gpgv --keyring "${DOWNLOAD_DIR}/gnu-keyring.gpg" \
            "${DOWNLOAD_DIR}/${patch_name}.sig" \
            "${DOWNLOAD_DIR}/${patch_name}"
    done
    echo "verified ${release_prefix} patch signatures"
}

verify_signatures bash53 "${BASH_PATCHLEVEL}" "Bash 5.3"
verify_signatures readline83 "${READLINE_PATCHLEVEL}" "Readline 8.3"

clone_tag() {
    local repository=$1
    local tag=$2
    local expected_tag_object=$3
    local destination=$4

    if [[ -e "${destination}" ]]; then
        echo "error: cached source already exists without a valid marker: ${destination}" >&2
        exit 1
    fi

    git clone --filter=blob:none --no-checkout --depth 1 --branch "${tag}" \
        "${repository}" "${destination}"
    actual_tag_object="$(git -C "${destination}" rev-parse "refs/tags/${tag}")"
    if [[ "${actual_tag_object}" != "${expected_tag_object}" ]]; then
        echo "error: ${tag} resolved to ${actual_tag_object}, expected ${expected_tag_object}" >&2
        exit 1
    fi
    git -C "${destination}" checkout --detach "${tag}"
}

prepare_patched_source() {
    local name=$1
    local repository=$2
    local tag=$3
    local tag_object=$4
    local patch_prefix=$5
    local patch_level=$6
    local patchlevel_file=$7
    local patchlevel_pattern=$8
    local destination="${CACHE_ROOT}/${name}"
    local marker="${destination}/.cherubsh-upstream-revision"
    local expected_marker="${tag}+patch${patch_level}"

    if [[ -f "${marker}" ]] && [[ "$(<"${marker}")" == "${expected_marker}" ]]; then
        echo "ready: ${destination}"
        return
    fi

    clone_tag "${repository}" "${tag}" "${tag_object}" "${destination}"
    for patch_number in $(seq 1 "${patch_level}"); do
        patch_name="$(printf '%s-%03d' "${patch_prefix}" "${patch_number}")"
        (
            cd "${destination}"
            patch --batch --forward --strip=0 < "${DOWNLOAD_DIR}/${patch_name}"
        )
    done

    if ! grep -Eq "${patchlevel_pattern}" "${destination}/${patchlevel_file}"; then
        echo "error: ${name} does not report patch level ${patch_level}" >&2
        exit 1
    fi
    printf '%s\n' "${expected_marker}" > "${marker}"
    echo "ready: ${destination}"
}

prepare_patched_source \
    bash-5.3.15 \
    "${BASH_REPOSITORY}" \
    "${BASH_TAG}" \
    "${BASH_TAG_OBJECT}" \
    bash53 \
    "${BASH_PATCHLEVEL}" \
    patchlevel.h \
    '^#define PATCHLEVEL 15$'

prepare_patched_source \
    readline-8.3-patch3 \
    "${READLINE_REPOSITORY}" \
    "${READLINE_TAG}" \
    "${READLINE_TAG_OBJECT}" \
    readline83 \
    "${READLINE_PATCHLEVEL}" \
    patchlevel \
    '^3$'

BASH_COMPLETION_DIR="${CACHE_ROOT}/bash-completion-2.18.0"
BASH_COMPLETION_MARKER="${BASH_COMPLETION_DIR}/.cherubsh-upstream-revision"
if [[ ! -f "${BASH_COMPLETION_MARKER}" ]]; then
    clone_tag \
        "${BASH_COMPLETION_REPOSITORY}" \
        "${BASH_COMPLETION_TAG}" \
        "${BASH_COMPLETION_TAG_OBJECT}" \
        "${BASH_COMPLETION_DIR}"
    printf '%s\n' "${BASH_COMPLETION_TAG}" > "${BASH_COMPLETION_MARKER}"
fi
echo "ready: ${BASH_COMPLETION_DIR}"
