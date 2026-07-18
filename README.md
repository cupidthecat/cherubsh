# CherubSH

CherubSH (`cherubsh`), formerly cupidshell, is a Rust implementation of Bash 5.3. It aims to match Bash behavior, rather than provide a Bash-like language. The project tests parsing, expansion, redirection, builtins, job control, traps, history, completion, and process behavior against Bash 5.3 and the vendored Brush compatibility corpus.

## Features

- A Bash 5.3-compatible parser, lexer, expansion engine, execution model, and shell state.
- All 86 upstream Bash 5.3 `run-*` drivers pass.
- The upstream Bash 5.3 `tests` tree is vendored and runs through the parity harness.
- All 2,077 runnable cases in the vendored Brush compatibility corpus pass against the Bash 5.3 oracle. Brush metadata or Bash-version constraints skip 28 cases.
- A differential fixture harness that compares CherubSH directly with a Bash 5.3 oracle.
- Bash-compatible parameter expansion, arithmetic expansion, command substitution, process substitution, brace expansion, globbing, word splitting, quote removal, arrays, associative arrays, and namerefs.
- Core Bash builtins including `alias`, `bind`, `break`, `builtin`, `caller`, `cd`, `command`, `complete`, `compgen`, `compopt`, `continue`, `declare`, `dirs`, `disown`, `echo`, `enable`, `eval`, `exec`, `exit`, `export`, `fc`, `fg`, `bg`, `getopts`, `hash`, `help`, `history`, `jobs`, `kill`, `let`, `local`, `mapfile`, `popd`, `printf`, `pushd`, `pwd`, `read`, `readonly`, `return`, `set`, `shift`, `shopt`, `source`, `suspend`, `test`, `times`, `trap`, `type`, `ulimit`, `umask`, `unalias`, `unset`, and `wait`.
- Parity coverage for job control, pipelines, subshells, coprocesses, redirections, here-documents, here-strings, traps, `set -e`, `pipefail`, `lastpipe`, and POSIX mode.
- Prompt decoding, history expansion, line-editing scaffolding, completion registration, and completion generation.
- Vendored Bash 5.3 test corpus, so users can run upstream parity without separately checking out Bash source.

## Bash 5.3 Parity

CherubSH passes every standalone `run-*` driver that Bash runs through `tests/run-all` or `make tests`.

Current parity sweep:

| Suite | Result |
| --- | ---: |
| Upstream Bash 5.3 `run-*` drivers | 86 / 86 passing |
| CherubSH fixture parity tests | 99 / 99 passing |
| Brush compatibility corpus runnable cases | 2,077 / 2,077 passing |
| Combined Bash, fixture, and Brush runnable parity | 2,262 / 2,262 passing |

The harness uses the unmodified Bash 5.3 expected outputs. It runs Bash's test drivers against `cherubsh`, records artifacts for each test, and fails on unexpected `FAIL`, `TIMEOUT`, or `XPASS` results.

Like [shellgei/rusty_bash](https://github.com/shellgei/rusty_bash), CherubSH is a Rust Bash clone measured against Bash behavior.

## Vendored Bash Tests

The Bash 5.3 test corpus is vendored in this repository:

- `vendor/bash-5.3/tests`: upstream Bash 5.3 tests, `.right` files, `run-*` drivers, and misc/manual tests.
- `vendor/bash-5.3/support`: upstream C sources for required test helpers (`recho`, `zecho`, `printenv`, `xcase`).
- `vendor/bash-5.3/examples/loadables`: upstream loadable examples used by selected tests.
- `vendor/bash-5.3/y.tab.c`, `bashansi.h`, `config.h`, `version.h`: upstream build-tree inputs used by heredoc and parser-sensitive tests.
- `vendor/bash-5.3/COPYING`, `README`, `AUTHORS`, `NEWS`: upstream metadata and license context.

You do not need a separate Bash source checkout to run the upstream test corpus. By default, the harness uses the vendored tests and compiles the small upstream helper programs in a temporary directory. Set `BASH_53_TESTS_DIR=/path/to/bash-5.3/tests` only when you want to compare against another Bash test tree.

A legacy Bash 5.2.21 vendor tree and oracle builder are retained for regression comparisons. Set `BASH_ORACLE_VERSION=5.2.21` when intentionally running that older gate.

The vendored `tests/misc` files are not part of Bash's normal `make tests` or `tests/run-all` target. They include manual, network, TTY, signal-timing, and performance scripts. The standard parity gate therefore runs the same suite that Bash runs by default.

## Vendored Brush Tests

The brush compatibility corpus is vendored under `vendor/brush` from `brush-shell/tests/cases`. The active CherubSH gate runs `vendor/brush/brush-shell/tests/cases/compat` against the same Bash 5.3 oracle used by the main parity harness; brush-specific CLI cases under `cases/brush` are retained as source context only.

Current Brush parity status for v0.3.0:

| Brush result | Count |
| --- | ---: |
| Passing runnable compat cases | 2,077 |
| Failing runnable compat cases | 0 |
| Skipped by Brush metadata | 27 |
| Skipped by Bash-version constraints | 1 |

These skips are not CherubSH-versus-Bash failures. The harness excludes them before execution because the Brush metadata marks them as skipped or because they need an oracle version outside the pinned Bash 5.3 gate.

The brush sweep is opt-in because it runs thousands of shell invocations:

```sh
RUN_BRUSH_PARITY=1 cargo test -p cherubsh --test brush_parity -- --nocapture
```

To run a focused slice:

```sh
RUN_BRUSH_PARITY=1 BRUSH_PARITY_FILTER='Builtins: printf' cargo test -p cherubsh --test brush_parity -- --nocapture
```

Reports are written to `target/parity/brush/report.tsv`. The combined driver also supports the same gate:

```sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

## Other Tests

The parity driver also runs CherubSH's differential fixtures against the Bash 5.3 oracle. They cover lifecycle behavior, parser acceptance, expansions, arrays, assignments, builtins, redirections, process substitution, functions, `set -e`, `set -x`, jobs, traps, `read`, `source`, `type`, history, completion, and coprocess behavior.

Regular Cargo unit and integration tests run as part of the same workspace sweep.

## Switching From Bash

CherubSH runs Bash-compatible scripts directly, but your shell is a critical system component. Keep `/bin/bash` installed and test your dotfiles and scripts before changing your login shell.

Build CherubSH:

```sh
cargo build --release -p cherubsh
```

Run it without installing:

```sh
target/release/cherubsh
target/release/cherubsh examples/01-basics.sh
```

Install it somewhere on `PATH`:

```sh
sudo install -m 0755 target/release/cherubsh /usr/local/bin/cherubsh
```

Run existing Bash scripts with CherubSH:

```sh
cherubsh ./script.sh
cherubsh -c 'printf "%s\n" "${BASH_VERSION:-compatible}"'
```

Use CherubSH for new scripts:

```sh
#!/usr/bin/env cherubsh
set -euo pipefail

name=${1:-world}
printf 'hello %s\n' "$name"
```

Try it as your current interactive shell:

```sh
exec cherubsh
```

Make it your login shell after testing:

```sh
command -v cherubsh | sudo tee -a /etc/shells
chsh -s "$(command -v cherubsh)"
```

Switch back to Bash if needed:

```sh
chsh -s /bin/bash
```

Do not replace `/bin/bash` with CherubSH on a system install. System scripts often depend on the exact path and build options of the distro-provided Bash. Prefer explicit script shebangs, `SHELL=/usr/local/bin/cherubsh` for user tools, or `chsh` for your own login shell.

## Running

```sh
cargo run -p cherubsh
```

Run a script:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
```

## Examples

Runnable examples live in `examples/`:

- `examples/01-basics.sh`: functions, arrays, associative arrays, loops, and `case`.
- `examples/02-expansion-and-redirection.sh`: parameter expansion, command substitution, brace expansion, here-documents, and process substitution.
- `examples/03-traps-coproc-and-jobs.sh`: traps, background jobs, `wait`, and named coprocess file descriptors.

Run them from the repository root:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
cargo run -p cherubsh -- examples/02-expansion-and-redirection.sh
cargo run -p cherubsh -- examples/03-traps-coproc-and-jobs.sh
```

## Testing

```sh
./tools/run-parity.sh
```

`tools/run-parity.sh` runs the full workspace test suite, the CherubSH fixture parity suite, and the vendored upstream Bash 5.3 suite. Set `RUN_BRUSH_PARITY=1` to add the vendored brush compatibility corpus to the same sweep.

The default parity oracle must be Bash 5.3:

- Set `BASH_53_PATH=/path/to/bash-5.3` to use an existing oracle binary.
- Or let `oracle/build-bash-5.3.sh` build one under `target/oracle/bash-5.3`.

The oracle builder can download the Bash 5.3 release tarball when no local source tree is supplied. Set `BASH_SRC=/path/to/bash-5.3` to build from an existing source tree instead.

## Benchmarking

Use the benchmark harness to compare CherubSH against the Bash 5.3 oracle:

```sh
./tools/bench.sh
```

The benchmark covers startup, `-c` parsing, large scripts, arithmetic and control flow, functions, aliases, variables, indexed and associative arrays, parameter expansion, pattern matching, word splitting, brace expansion, command substitution, `read`, `mapfile`, `printf`, `test`, command lookup, completion generation, redirections, here-documents, `eval`, subshells, background `wait`, pipelines, process substitution, external commands, shell options, positional parameters, `getopts`, traps, directory changes, sourcing, and glob scanning.

Useful knobs:

```sh
RUNS=30 WARMUPS=5 ./tools/bench.sh
BENCH_BUILD=0 BASH_53_PATH=/path/to/bash-5.3 ./tools/bench.sh
```

Results are printed as median/min/max milliseconds with a ratio against Bash 5.3. Raw samples are written to `target/bench/raw.tsv`, and the summarized table is written to `target/bench/summary.tsv`.

Benchmarks are not parity tests. Run them on an otherwise idle machine, compare medians across multiple runs, and treat external pipeline cases as measurements of the shell and system together, not just interpreter speed.


## Credits

CherubSH is an independent Rust implementation, but its compatibility target and validation corpus are grounded in upstream shell work:

- [GNU Bash](https://www.gnu.org/software/bash/) is the behavioral oracle and source of the vendored Bash 5.3 and legacy 5.2.21 test corpora, helper sources, and reference metadata. Bash's upstream author records credit Brian Fox, Chet Ramey, the GNU Project, and many other contributors; see `vendor/bash-5-3/AUTHORS` and `vendor/bash-5.2.21/AUTHORS`.
- [brush](https://github.com/reubeno/brush) by Reuben Olinsky provides the vendored shell compatibility cases used by the optional Brush parity sweep; see `vendor/brush/README.cherubsh.md` and `vendor/brush/LICENSE`.
- [GNU Readline](https://www.gnu.org/software/readline/) and the GNU History library shape Bash's interactive editing, `bind`, completion, and history behavior, which CherubSH reimplements for compatibility rather than vendoring directly.
- [shellgei/rusty_bash](https://github.com/shellgei/rusty_bash) is useful adjacent work in the Rust Bash-clone space and is referenced above as a comparison point; CherubSH does not vendor code from it.
