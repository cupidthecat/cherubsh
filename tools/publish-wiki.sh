#!/usr/bin/env bash
set -euo pipefail

source_dir=${1:-wiki}
target_dir=${2:?usage: tools/publish-wiki.sh SOURCE_DIR TARGET_DIR}
wiki_repository=${WIKI_REPOSITORY:?WIKI_REPOSITORY must name the wiki Git repository}
commit_name=${WIKI_COMMIT_NAME:-cherubsh wiki publisher}
commit_email=${WIKI_COMMIT_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}
commit_message=${WIKI_COMMIT_MESSAGE:-"docs: sync wiki from ${GITHUB_SHA:-local source}"}

if [[ ! -d "$source_dir" ]]; then
  printf 'wiki source directory not found: %s\n' "$source_dir" >&2
  exit 1
fi

if [[ -e "$target_dir" ]]; then
  printf 'wiki checkout directory already exists: %s\n' "$target_dir" >&2
  exit 1
fi

source_dir=$(cd "$source_dir" && pwd)
git clone --quiet "$wiki_repository" "$target_dir"

rsync --archive --delete --exclude='.git' "$source_dir/" "$target_dir/"

git -C "$target_dir" add --all
if git -C "$target_dir" diff --cached --quiet; then
  printf 'wiki is already current\n'
  exit 0
fi

git -C "$target_dir" config user.name "$commit_name"
git -C "$target_dir" config user.email "$commit_email"
git -C "$target_dir" commit -m "$commit_message"
git -C "$target_dir" push origin HEAD:master
