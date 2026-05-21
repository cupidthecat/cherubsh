# CherubSH

CherubSH (`cherubsh`), formerly known as cupidshell, is a strict Bash 5.3-compatible shell implementation written in Rust. The target is behavioral parity with Bash, not a Bash-like language: parsing, expansion, redirection, builtins, job control, traps, history, completion, and process behavior are tested against a real Bash 5.3 oracle and the vendored Brush compatibility corpus.

## Features

- Bash 5.3-compatible parser, lexer, expansion engine, execution model, and shell state.
- Full standard upstream Bash 5.3 test-suite parity: 86 / 86 upstream `run-*` drivers passing.
- Vendored Brush compatibility corpus parity: 2,077 / 2,077 runnable compat cases passing against the Bash 5.3 oracle, with 28 cases skipped by Brush metadata or Bash-version constraints.
- Differential fixture harness that compares CherubSH behavior directly against a Bash 5.3 oracle.
- Bash-compatible parameter expansion, arithmetic expansion, command substitution, process substitution, brace expansion, globbing, word splitting, quote removal, arrays, associative arrays, and namerefs.
- Core Bash builtins including `alias`, `bind`, `break`, `builtin`, `caller`, `cd`, `command`, `complete`, `compgen`, `compopt`, `continue`, `declare`, `dirs`, `disown`, `echo`, `enable`, `eval`, `exec`, `exit`, `export`, `fc`, `fg`, `bg`, `getopts`, `hash`, `help`, `history`, `jobs`, `kill`, `let`, `local`, `mapfile`, `popd`, `printf`, `pushd`, `pwd`, `read`, `readonly`, `return`, `set`, `shift`, `shopt`, `source`, `suspend`, `test`, `times`, `trap`, `type`, `ulimit`, `umask`, `unalias`, `unset`, and `wait`.
- Job control, pipelines, subshells, coprocesses, redirections, here-documents, here-strings, traps, `set -e`, `pipefail`, `lastpipe`, and POSIX-mode behavior covered by parity tests.
- Interactive pieces including prompt decoding, history expansion, line editing scaffolding, completion registration, and completion generation.
- Vendored Bash 5.3 test corpus, so users can run upstream parity without separately checking out Bash source.

## Bash 5.3 Parity

CherubSH passes the full standard upstream Bash 5.3 test suite: every standalone `run-*` driver that Bash runs through `tests/run-all` / `make tests`.

Current parity sweep:

| Suite | Result |
| --- | ---: |
| Upstream Bash 5.3 `run-*` drivers | 86 / 86 passing |
| CherubSH fixture parity tests | 99 / 99 passing |
| Brush compatibility corpus runnable cases | 2,077 / 2,077 passing |
| Combined Bash, fixture, and Brush runnable parity | 2,262 / 2,262 passing |

The upstream tests are run with unmodified Bash 5.3 expected outputs. The harness points Bash's own test drivers at `cherubsh`, records per-test artifacts, and fails on any unexpected `FAIL`, `TIMEOUT`, or `XPASS`.

This puts CherubSH in the same category as projects like [shellgei/rusty_bash](https://github.com/shellgei/rusty_bash): a Rust Bash clone measured against Bash behavior. CherubSH's bar is stricter: full Bash 5.3 standard-suite parity is the baseline, not a feature checklist or altered expected-output set.

## Vendored Bash Tests

The Bash 5.3 test corpus is vendored in this repository:

- `vendor/bash-5.3/tests`: upstream Bash 5.3 tests, `.right` files, `run-*` drivers, and misc/manual tests.
- `vendor/bash-5.3/support`: upstream C sources for required test helpers (`recho`, `zecho`, `printenv`, `xcase`).
- `vendor/bash-5.3/examples/loadables`: upstream loadable examples used by selected tests.
- `vendor/bash-5.3/y.tab.c`, `bashansi.h`, `config.h`, `version.h`: upstream build-tree inputs used by heredoc and parser-sensitive tests.
- `vendor/bash-5.3/COPYING`, `README`, `AUTHORS`, `NEWS`: upstream metadata and license context.

Users do not need a separate Bash source checkout to run the upstream test corpus. The harness defaults to the vendored tests and compiles the small upstream helper programs into a temporary directory at test time. Set `BASH_53_TESTS_DIR=/path/to/bash-5.3/tests` only when intentionally comparing against another Bash test tree.

A legacy Bash 5.2.21 vendor tree and oracle builder are retained for regression comparisons. Set `BASH_ORACLE_VERSION=5.2.21` when intentionally running that older gate.

The files under `tests/misc` are vendored too, but they are not part of Bash's normal `make tests` / `tests/run-all` target. They include manual, network, TTY, signal-timing, and performance scripts, so the standard parity gate focuses on the same suite Bash itself runs by default.

## Vendored Brush Tests

The brush compatibility corpus is vendored under `vendor/brush` from `brush-shell/tests/cases`. The active CherubSH gate runs `vendor/brush/brush-shell/tests/cases/compat` against the same Bash 5.3 oracle used by the main parity harness; brush-specific CLI cases under `cases/brush` are retained as source context only.

Current Brush parity status for v0.2.0:

| Brush result | Count |
| --- | ---: |
| Passing runnable compat cases | 2,077 |
| Failing runnable compat cases | 0 |
| Skipped by Brush metadata | 27 |
| Skipped by Bash-version constraints | 1 |

The skipped cases are not CherubSH-vs-Bash failures. They are excluded before execution because the vendored Brush case metadata marks them as skipped, or because the case requires an oracle version outside the pinned Bash 5.3 gate.

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

The parity driver also runs CherubSH's own differential fixtures against the Bash 5.3 oracle. These cover lifecycle behavior, parser acceptance, expansions, arrays, assignments, builtins, redirections, process substitution, functions, `set -e`, `set -x`, jobs, traps, `read`, `source`, `type`, history, completion, and coprocess behavior.

Regular Cargo unit and integration tests run as part of the same workspace sweep.

## Switching From Bash

CherubSH is meant to run Bash-compatible scripts directly, but a shell is a critical part of a system. Keep `/bin/bash` installed and test your dotfiles and scripts before changing your login shell.

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

The benchmark spans startup, `-c` parsing, large script parsing, arithmetic and control flow, functions, aliases, variables, indexed and associative arrays, parameter expansion, pattern matching, word splitting, brace expansion, command substitution, `read`, `mapfile`, `printf`, `test`, command lookup, completion generation, redirections, here-documents, `eval`, subshells, background `wait`, pipelines, process substitution, external commands, shell options, positional parameters, `getopts`, traps, directory changes, sourcing, and glob scanning.

Useful knobs:

```sh
RUNS=30 WARMUPS=5 ./tools/bench.sh
BENCH_BUILD=0 BASH_53_PATH=/path/to/bash-5.3 ./tools/bench.sh
```

Results are printed as median/min/max milliseconds with a ratio against Bash 5.3. Raw samples are written to `target/bench/raw.tsv`, and the summarized table is written to `target/bench/summary.tsv`.

Benchmarks are not parity tests. Run them on an otherwise idle machine, compare medians over multiple runs, and treat external pipeline cases as shell-plus-system measurements rather than pure interpreter speed.
