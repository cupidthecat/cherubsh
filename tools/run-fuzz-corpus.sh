#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGETS=(lexer parser expansion line_input readline_ffi)

if ! cargo +nightly-2026-07-30 fuzz --help >/dev/null 2>&1; then
    echo "error: cargo-fuzz is required; install version 0.13.2" >&2
    exit 1
fi

for target in "${TARGETS[@]}"; do
    corpus="${WORKSPACE_ROOT}/fuzz/corpus/${target}"
    if [[ ! -d "${corpus}" ]]; then
        echo "error: missing corpus directory: ${corpus}" >&2
        exit 1
    fi
    found=0
    while IFS= read -r -d '' seed; do
        found=1
        cargo +nightly-2026-07-30 fuzz run \
            --fuzz-dir "${WORKSPACE_ROOT}/fuzz" \
            "${target}" \
            "${seed}" \
            -- \
            -runs=1 \
            -timeout=10 \
            -rss_limit_mb=2048
    done < <(find "${corpus}" -maxdepth 1 -type f -print0 | sort -z)
    if ((found == 0)); then
        echo "error: empty corpus directory: ${corpus}" >&2
        exit 1
    fi
done
