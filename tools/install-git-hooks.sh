#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"
git config core.hooksPath .githooks
printf 'installed repository hooks from %s/.githooks\n' "$repo_root"
