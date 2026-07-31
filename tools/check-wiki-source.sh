#!/usr/bin/env bash
set -euo pipefail

wiki_dir=${1:-wiki}
required_pages=(
  _Sidebar
  Home
  Getting-started
  Using-CherubSH
  Interactive-shell
  Command-line-reference
  Readline-and-History
  Compatibility
  Testing
  Architecture
  Development
  Troubleshooting
  Contributing
  Publishing-the-wiki
)

if [[ ! -d "$wiki_dir" ]]; then
  printf 'wiki source directory not found: %s\n' "$wiki_dir" >&2
  exit 1
fi

for page in "${required_pages[@]}"; do
  page_path="$wiki_dir/$page.md"
  if [[ ! -s "$page_path" ]]; then
    printf 'required wiki page is missing or empty: %s\n' "$page_path" >&2
    exit 1
  fi
done

for page in "${required_pages[@]:1}"; do
  page_path="$wiki_dir/$page.md"
  if ! head -n 1 "$page_path" | grep -q '^# '; then
    printf 'wiki page needs a top-level title: %s\n' "$page_path" >&2
    exit 1
  fi
done

sidebar_targets=(
  Home
  Getting-started
  Using-CherubSH
  Interactive-shell
  Command-line-reference
  Readline-and-History
  Compatibility
  Testing
  Architecture
  Development
  Troubleshooting
  Contributing
  Publishing-the-wiki
)

for target in "${sidebar_targets[@]}"; do
  if ! grep -Fq "]($target)" "$wiki_dir/_Sidebar.md"; then
    printf 'sidebar is missing its expected page link: %s\n' "$target" >&2
    exit 1
  fi
done

if grep -RIn --include='*.md' $'\r' "$wiki_dir"; then
  printf 'wiki source must use LF line endings\n' >&2
  exit 1
fi

if grep -RIn --include='*.md' -E '—|–' "$wiki_dir"; then
  printf 'wiki source must not contain em or en dashes\n' >&2
  exit 1
fi

printf 'wiki source is valid: %s\n' "$wiki_dir"
