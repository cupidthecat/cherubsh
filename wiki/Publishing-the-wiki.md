# Publishing the wiki

`wiki/` is the canonical source for the GitHub Wiki. Contributors edit these versioned Markdown files, review them in pull requests, and merge them to `main`. A GitHub Actions workflow validates the source and mirrors it to `cupidthecat/cherubsh.wiki.git` after a relevant push to `main`.

The workflow does not publish documentation from a feature branch. It validates those changes on pull requests, then publishes the reviewed version once it reaches the default branch. This keeps the public wiki aligned with the repository's published source.

## One-time GitHub setup

The repository currently has GitHub Wikis disabled. Enable it in the repository's Settings, under Features, and create an initial wiki page. GitHub creates the separate wiki Git repository when the wiki exists.

Create an SSH deploy key that is dedicated to this repository. Do not reuse a personal key or print the private key into a terminal log.

```sh
ssh-keygen -t ed25519 -C 'cherubsh wiki publisher' -f ./cherubsh-wiki-deploy-key
```

Add `cherubsh-wiki-deploy-key.pub` to the CherubSH repository's Deploy keys with write access. Add the private-key file's content to the repository Actions secret named `WIKI_DEPLOY_KEY`. Delete the local private-key file after GitHub has accepted the secret. Keep the public key only if you need to identify or rotate the deploy key later.

The workflow uses a deploy key rather than assuming that `GITHUB_TOKEN` can write to the separate wiki repository. Its normal repository permission remains read-only.

## Local contributor workflow

Install the versioned pre-commit hook once in each clone:

```sh
./tools/install-git-hooks.sh
```

When a commit stages a change below `wiki/`, the hook extracts the staged wiki into a temporary directory and runs the source check. It prevents a malformed navigation page, a missing required page, a missing top-level title, CRLF line endings, or an em/en dash from entering the commit.

Edit and verify pages as follows:

```sh
./tools/check-wiki-source.sh
git add wiki
git commit -m 'docs: update wiki'
git push origin main
```

The hook checks commits locally. GitHub Actions handles pushes. There is no GitHub Action event for an unpushed local commit.

## Remote publish workflow

`.github/workflows/wiki.yml` runs on pull requests that touch wiki source and on pushes to `main` that touch wiki source, the checker, publisher, hook installer, or workflow file.

The validation job runs `tools/check-wiki-source.sh`. On a `main` push, the publish job then configures the `WIKI_DEPLOY_KEY`, runs `tools/publish-wiki.sh`, and commits only when the checked-out wiki differs from `wiki/`. The publisher copies with `rsync --delete`, so removed source pages are also removed from the rendered wiki. It uses a workflow-wide concurrency group to prevent overlapping runs from racing on the wiki Git branch.

You can use the workflow's `workflow_dispatch` button to rerun validation and synchronization after setting up or rotating the key.

## Test the publisher without GitHub

`tools/publish-wiki.sh` accepts a repository URL through `WIKI_REPOSITORY`. A local bare repository can exercise the copy, commit, and push behavior without a network credential:

```sh
tmp_dir=$(mktemp -d)
git init --bare "$tmp_dir/wiki.git"
WIKI_REPOSITORY="$tmp_dir/wiki.git" \
  ./tools/publish-wiki.sh wiki "$tmp_dir/checkout"
git --git-dir="$tmp_dir/wiki.git" log --oneline
```

Remove the temporary directory when you are done. Do not use a shared or production wiki repository for this test.

## Recovery and rotation

If a publish fails, inspect the workflow log first. Missing or invalid `WIKI_DEPLOY_KEY`, a disabled Wiki, and a removed deploy key are the usual setup failures. After correcting GitHub configuration, run the workflow manually or push another source change.

To rotate the credential, add a new deploy key, replace the `WIKI_DEPLOY_KEY` secret, run a manual publish, and then remove the old deploy key. The source pages stay in the main repository, so a successful rerun restores the complete wiki state.
