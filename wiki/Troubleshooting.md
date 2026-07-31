# Troubleshooting

Start by reducing the problem to a command that can run twice. State which binary you used, whether input was interactive, and whether a startup file was loaded.

## The binary is missing or old

Build a fresh release binary and check the path:

```sh
cargo build --release -p cherubsh
command -v cherubsh
cherubsh --version
```

When testing the repository checkout, invoke `target/release/cherubsh` directly. That avoids accidentally running an older installed copy.

## A script differs from Bash

Run the smallest script under the pinned Bash oracle and CherubSH. Record the exit status, standard output, standard error, and files left behind. Do not compare only visible terminal text if redirections, jobs, traps, or files are involved.

Use the focused test filters when the difference belongs to a known area. See [Testing](Testing).

## Interactive input behaves strangely

Remove startup-file variables first:

```sh
cherubsh --norc
```

Then try a plain prompt:

```sh
PS1='$ '
```

If the problem disappears, add your RC-file lines back in small groups. Prompt command substitutions, terminal settings, and sourced completion scripts can all change the interactive path.

Use `--noediting` to separate shell input processing from editor behavior. If a key or redraw problem needs a test, reproduce the full terminal state with a pseudo-terminal rather than relying only on a pipe capture.

## Completion is absent or incomplete

Confirm that the relevant completion file was sourced and that it can be read. The development fetch command places the pinned bash-completion tree at `target/upstream/bash-completion-2.18.0`; a normal user setup often sources the distribution file at `/usr/share/bash-completion/bash_completion`.

Completion depends on the command line and current shell state. Keep a report to one completion function, one command line, and the expected candidate list where possible.

## History is in the wrong file

Check the variables in the shell that is running:

```sh
printf '%s\n' "$HISTFILE" "$HISTSIZE" "$HISTFILESIZE" "$HISTCONTROL"
```

Without an explicit `HISTFILE`, CherubSH uses `~/.bash_history` for interactive history. Set a separate `HISTFILE` if you do not want CherubSH and Bash to share it. An empty `HISTFILE` disables file history.

## A C program finds the wrong Readline library

Inspect the compiler flags from the staged pkg-config metadata and then inspect the runtime loader path. A system header or `libreadline` chosen by mistake can look like an ABI problem.

```sh
export PKG_CONFIG_PATH="$PWD/target/readline/lib/pkgconfig"
pkg-config --cflags --libs readline
```

Rebuild the staged libraries with `./tools/build-readline.sh` before comparing C behavior.

## The full parity gate fails before testing

Check the required packages, network access to the pinned sources, available disk space, and the generated reports. Run `./tools/fetch-upstream.sh` by itself first. The command validates source identities and signatures, so a fetch failure should be investigated rather than bypassed.
