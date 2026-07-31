# CherubSH

CherubSH (`cherubsh`), formerly cupidshell, is a Rust implementation of Bash 5.3 behavior. It includes its own UTF-8 line editor and C-compatible GNU Readline and History libraries. Compatibility is checked against pinned builds of Bash 5.3.15 and Readline 8.3 patch 3, plus the Brush shell test corpus.

CherubSH is developed and tested on Linux. The same build and test commands work under WSL.

## Documentation

The repository keeps the source for its GitHub Wiki in [`wiki/`](wiki/). Read the [wiki home page](wiki/Home.md) from the checkout, or use the GitHub Wiki once it has been enabled and published. Contributors should edit the versioned source pages rather than the rendered wiki; [Publishing the wiki](wiki/Publishing-the-wiki.md) explains the validation and publication workflow.

## Compatibility status

The v0.3.0 parity gates currently report:

| Suite | Result |
| --- | ---: |
| Upstream Bash 5.3 `run-*` drivers | 86 / 86 passing |
| CherubSH differential fixtures | 99 / 99 passing |
| Runnable Brush compatibility cases | 2,077 / 2,077 passing |
| Brush cases skipped by their metadata or Bash version | 28 |

The upstream Bash gate uses the original `.right` files. The Brush gate runs each case once with Bash 5.3.15 and once with CherubSH, then compares status, output, and files left behind. Readline tests compile the same C fixtures and upstream examples against GNU Readline and the CherubSH library.

Bash is used only as a test oracle. CherubSH does not call Bash to parse commands, print syntax trees, extract translation strings, expand completions, or run loadable builtins.

## What works

- Bash parsing and execution for functions, pipelines, subshells, coprocesses, background jobs, traps, redirections, here-documents, and here-strings.
- Parameter, arithmetic, command, process, brace, pathname, and tilde expansion, including indexed arrays, associative arrays, namerefs, quoting, word splitting, and pattern operators.
- Bash shell options and `shopt` settings covered by the Bash and Brush compatibility suites.
- Native `--pretty-print`, `--dump-strings`, `--dump-po-strings`, and `-D` invocation modes for files, standard input, and `-c` command strings.
- The standard builtins used by the upstream tests, including job control, history, programmable completion, `read`, `mapfile`, `printf`, `test`, `source`, and `wait -n -p`.
- Bash 5.3.15 loadable builtins through `enable -f`, including scalar, indexed-array, associative-array, input, and child-shell ABI calls used by Bash's example modules.
- Interactive Emacs and Vi keymaps, UTF-8 cursor movement, undo, kill/yank behavior, history search, terminal-width-aware redisplay, and programmable completion.
- Programmable completion resources, callbacks, filters, option ordering, and lazy-loaded Git completion from bash-completion 2.18.
- Separate Readline 8.3 and History libraries with public headers, C symbols, custom callbacks, macros, keymaps, versioned shared-library names, static archives, and `pkg-config` files.

## Build and run

The repository pins Rust 1.93.1 in `rust-toolchain.toml`.

```sh
cargo build --release -p cherubsh
target/release/cherubsh
target/release/cherubsh examples/01-basics.sh
```

You can also run it through Cargo:

```sh
cargo run -p cherubsh
cargo run -p cherubsh -- -c 'printf "%s\n" "$BASH_VERSION"'
```

Parser output and translation extraction run without executing the input:

```sh
target/release/cherubsh --pretty-print -c 'for name in one two; do echo "$name"; done'
target/release/cherubsh --dump-strings messages.sh
target/release/cherubsh --dump-po-strings messages.sh
```

## Interactive setup

CherubSH reads `~/.cherubrc` for an interactive non-login shell. It does not silently fall back to `~/.bashrc`. Use `--norc` to skip the file or `--rcfile path` to choose another one.

A small starting file might look like this:

```sh
# ~/.cherubrc
export EDITOR=vi
set -o vi
alias ll='ls -alF'

if [[ -r /usr/share/bash-completion/bash_completion ]]; then
    source /usr/share/bash-completion/bash_completion
fi
```

The pinned bash-completion source used by development tests is fetched to `target/upstream/bash-completion-2.18.0` by `tools/fetch-upstream.sh`.

## Bash loadable builtins

The parity driver builds the example modules shipped with Bash 5.3.15 and checks every builtin it finds. Each one must load, print help, run, and unload the same way under Bash and CherubSH. Additional C fixtures check data exchange through scalars and arrays, line input, subscript parsing, and the child shell started by the `push` example.

After the oracle modules have been built, you can load one directly:

```sh
BASH_LOADABLES_PATH=target/oracle/bash-5.3.15/examples/loadables \
  target/release/cherubsh --norc -c '
    enable -f printenv printenv
    printenv HOME
    enable -d printenv
  '
```

## Readline and History libraries

Build and stage the compatibility libraries with:

```sh
./tools/build-readline.sh
```

The staged files are split between headers and libraries:

```text
target/readline/include/readline/  readline.h, history.h, keymaps.h, tilde.h, ...
target/readline/lib/               libreadline.so.8.3, libhistory.so.8.3, static archives
target/readline/lib/pkgconfig/     readline.pc, history.pc
```

For example, a C program can link against the staged shared library like this:

```sh
cc example.c \
  -Itarget/readline/include \
  -Ltarget/readline/lib \
  -Wl,-rpath,"$PWD/target/readline/lib" \
  -lreadline -ltermcap
```

The staged directory also works with `pkg-config`:

```sh
export PKG_CONFIG_PATH="$PWD/target/readline/lib/pkgconfig"
pkg-config --cflags --libs readline
pkg-config --cflags --libs history
```

Run the GNU differential gate with:

```sh
./tools/run-readline-parity.sh
```

That command builds GNU Readline 8.3 patch 3, checks public symbol coverage and library names, and compiles the same C fixtures against both implementations. The fixtures exercise a pseudo-terminal Readline loop, user-defined C callbacks, macros, bare keymaps, and History behavior. It also builds every upstream Readline example and compares deterministic output byte for byte. Reports are kept under `target/parity/readline`.

## Testing

Run the ordinary Rust test suite first:

```sh
cargo test --workspace --locked
```

The full parity gate needs common build tools, `bison`, `texinfo`, `gpgv`, ncurses development headers, Perl, Python 3, and util-linux. On Debian or Ubuntu:

```sh
sudo apt-get install \
  autoconf bison build-essential curl git gpgv \
  libncurses-dev patch perl python3 texinfo util-linux
```

Fetch and verify the pinned sources, then run every gate:

```sh
./tools/fetch-upstream.sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

`tools/fetch-upstream.sh` checks the recorded tag objects, SHA-256 hashes, and GNU patch signatures before preparing Bash 5.3.15 and Readline 8.3 patch 3. The exact references live in `upstream.lock` and `upstream.sha256`.

The main driver builds a Bash 5.3.15 oracle under `target/oracle`, runs the Rust workspace and upstream Bash suites, and finishes with the Readline gate. Add `RUN_BRUSH_PARITY=1` to include all 2,105 Brush cases, as shown above.

Useful focused commands:

```sh
# One or more upstream Bash drivers
RUN_UPSTREAM_PARITY=1 \
UPSTREAM_PARITY_FILTER='history,jobs' \
cargo test -p cherubsh --test upstream_parity -- --nocapture

# Brush cases whose qualified names contain this text
RUN_BRUSH_PARITY=1 \
BRUSH_PARITY_FILTER='Builtins: wait' \
cargo test -p cherubsh --test brush_parity -- --nocapture

# Readline and History only
./tools/run-readline-parity.sh
```

Parity reports are written below `target/parity`.

## Reference source trees

- `vendor/readline-8.3` contains the user-supplied Readline 8.3 source used to build the GNU oracle.
- `vendor/bash-5.3.15/tests` contains the checked-in Bash test corpus and expected output files.
- `vendor/brush` contains the Brush compatibility YAML at commit `5a50c12ed59e610dae038db9acf642286c585e2d`.
- `target/upstream` holds verified source caches created by `tools/fetch-upstream.sh`.
- `target/oracle` holds local GNU builds. It is generated and ignored by Git.

The normal upstream Bash suite excludes `tests/misc`, matching Bash's own `make tests` and `tests/run-all` targets. That directory contains manual, network, performance, signal-timing, and terminal-dependent scripts.

## Examples

The scripts under `examples/` cover everyday shell use, expansion and redirection, and process management:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
cargo run -p cherubsh -- examples/02-expansion-and-redirection.sh
cargo run -p cherubsh -- examples/03-traps-coproc-and-jobs.sh
cargo run -p cherubsh -- examples/04-log-summary.sh
cargo run -p cherubsh -- examples/05-parallel-checks.sh
```

## Installing as a shell

Install the release binary somewhere on `PATH`:

```sh
sudo install -m 0755 target/release/cherubsh /usr/local/bin/cherubsh
```

Test your scripts and dotfiles before making it your login shell. Keep the system Bash package installed.

```sh
command -v cherubsh | sudo tee -a /etc/shells
chsh -s "$(command -v cherubsh)"
```

To switch back:

```sh
chsh -s /bin/bash
```

Do not replace `/bin/bash`. Distribution scripts may depend on that exact path and on build-time options from the packaged Bash.

## Credits and licenses

CherubSH is an independent implementation. Its behavior and tests draw on several upstream projects:

- [GNU Bash](https://www.gnu.org/software/bash/) provides the compatibility target and upstream test corpus. Its source and tests are GPL-3.0-or-later.
- [GNU Readline](https://www.gnu.org/software/readline/) provides the C API and behavioral reference for the compatible Readline and History libraries. Its source and headers are GPL-3.0-or-later.
- [Brush](https://github.com/reubeno/brush) provides the MIT-licensed shell compatibility cases under `vendor/brush`.
- [bash-completion](https://github.com/scop/bash-completion) provides the GPL-2.0-or-later completion corpus used for compatibility testing.
- [shellgei/rusty_bash](https://github.com/shellgei/rusty_bash) is useful related work on a Bash-compatible shell in Rust.

CherubSH itself is licensed under GPL-3.0-or-later. See `LICENSE` and the license files kept with each vendored source tree.
