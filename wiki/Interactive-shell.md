# Interactive shell

Run `cherubsh` without a script or `-c` command to start an interactive shell. CherubSH has a UTF-8 line editor with Emacs and Vi keymaps, undo, kill and yank operations, history search, multiline redisplay, terminal-width handling, and programmable completion.

```sh
cherubsh
```

## Startup files

For an interactive non-login shell, CherubSH reads `~/.cherubrc`. It does not silently use `~/.bashrc`.

Use `--norc` to skip that file. Use `--rcfile path` or `--init-file path` to load a specific file instead:

```sh
cherubsh --norc
cherubsh --rcfile "$HOME/.config/cherubsh/rc"
```

A small `~/.cherubrc` can set editing mode, aliases, and completion support:

```sh
export EDITOR=vi
set -o vi
alias ll='ls -alF'

if [[ -r /usr/share/bash-completion/bash_completion ]]; then
    source /usr/share/bash-completion/bash_completion
fi
```

Use `--noprofile` when you need to skip login startup processing. `--login` requests login-shell behavior.

## Editing mode

The shell supports the familiar Emacs and Vi modes. Select one through shell options in the startup file or at the prompt:

```sh
set -o emacs
set -o vi
```

`--noediting` starts an interactive shell without line editing. This can help narrow down a terminal or key-sequence problem.

## Completion

CherubSH implements programmable completion resources, callbacks, filters, option ordering, and lazy loading of Git completion from bash-completion 2.18. The development fetch step places that pinned completion source below `target/upstream/bash-completion-2.18.0`.

An ambiguous Tab inserts any longer shared prefix and rings the terminal bell. If a Tab leaves the text unchanged, press Tab again to list the candidates. Set `show-all-if-ambiguous on` in inputrc to list ambiguous matches on the first Tab. Set `show-all-if-unmodified on` to list them on the first Tab only when completion leaves the input unchanged. The boolean parser treats an empty value, `on` (case-insensitive), and `1` as on. Every other value is off.

Completion depends on the commands and files visible in the current shell. Start with the distribution's `bash_completion` file if it is available, then reduce any mismatch to a small function and command line before reporting it.

## History

Interactive history defaults to the usual Bash location, `~/.bash_history`, when `HISTFILE` is not set. CherubSH recognizes `HISTFILE`, `HISTSIZE`, `HISTFILESIZE`, and `HISTCONTROL`. Set `HISTFILE` to an empty value to disable file history.

```sh
export HISTFILE="$HOME/.cherub_history"
export HISTSIZE=2000
export HISTFILESIZE=4000
export HISTCONTROL=ignoredups
```

The history builtin and the editor's history navigation are separate pieces of the same shell history table. [Readline and History](Readline-and-History) covers the standalone C libraries.

## Prompts

Prompt expansion follows the Bash-compatible variables and escape handling implemented by the shell. Keep prompt commands simple while debugging. A slow or blocking command substitution in `PS1` can make the editor look unresponsive even when line editing is working.

For a clean session, start with no user startup file and a plain prompt:

```sh
cherubsh --norc
PS1='$ '
```
