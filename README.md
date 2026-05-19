# CherubSH

CherubSH (`cherubsh`), formerly known as cupidshell, is a strict Bash 5.2.21-compatible shell implementation written in Rust. The target is behavioral parity with Bash, not a Bash-like language: parsing, expansion, redirection, builtins, job control, traps, history, completion, and process behavior are tested against a real Bash 5.2.21 oracle.

## Features

- Bash 5.2.21-compatible parser, lexer, expansion engine, execution model, and shell state.
- Full standard upstream Bash 5.2.21 test-suite parity: 83 / 83 upstream `run-*` drivers passing.
- Differential fixture harness that compares CherubSH behavior directly against a Bash 5.2.21 oracle.
- Bash-compatible parameter expansion, arithmetic expansion, command substitution, process substitution, brace expansion, globbing, word splitting, quote removal, arrays, associative arrays, and namerefs.
- Core Bash builtins including `alias`, `bind`, `break`, `builtin`, `caller`, `cd`, `command`, `complete`, `compgen`, `compopt`, `continue`, `declare`, `dirs`, `disown`, `echo`, `enable`, `eval`, `exec`, `exit`, `export`, `fc`, `fg`, `bg`, `getopts`, `hash`, `help`, `history`, `jobs`, `kill`, `let`, `local`, `mapfile`, `popd`, `printf`, `pushd`, `pwd`, `read`, `readonly`, `return`, `set`, `shift`, `shopt`, `source`, `suspend`, `test`, `times`, `trap`, `type`, `ulimit`, `umask`, `unalias`, `unset`, and `wait`.
- Job control, pipelines, subshells, coprocesses, redirections, here-documents, here-strings, traps, `set -e`, `pipefail`, `lastpipe`, and POSIX-mode behavior covered by parity tests.
- Interactive pieces including prompt decoding, history expansion, line editing scaffolding, completion registration, and completion generation.
- Vendored Bash 5.2.21 test corpus, so users can run upstream parity without separately checking out Bash source.

## Bash 5.2.21 Parity

CherubSH passes the full standard upstream Bash 5.2.21 test suite: every standalone `run-*` driver that Bash runs through `tests/run-all` / `make tests`.

Current parity sweep:

| Suite | Result |
| --- | ---: |
| Upstream Bash 5.2.21 `run-*` drivers | 83 / 83 passing |
| CherubSH fixture parity tests | 99 / 99 passing |
| Combined parity driver | 182 / 182 passing |

The upstream tests are run with unmodified Bash 5.2.21 expected outputs. The harness points Bash's own test drivers at `cherubsh`, records per-test artifacts, and fails on any unexpected `FAIL`, `TIMEOUT`, or `XPASS`.

This puts CherubSH in the same category as projects like [shellgei/rusty_bash](https://github.com/shellgei/rusty_bash): a Rust Bash clone measured against Bash behavior. CherubSH's bar is stricter: full Bash 5.2.21 standard-suite parity is the baseline, not a feature checklist or altered expected-output set.

## Vendored Bash Tests

The Bash 5.2.21 test corpus is vendored in this repository:

- `vendor/bash-5.2.21/tests`: upstream Bash 5.2.21 tests, `.right` files, `run-*` drivers, and misc/manual tests.
- `vendor/bash-5.2.21/support`: upstream C sources for required test helpers (`recho`, `zecho`, `printenv`, `xcase`).
- `vendor/bash-5.2.21/y.tab.c`, `config.h`, `version.h`: upstream build-tree inputs used by heredoc size tests.
- `vendor/bash-5.2.21/COPYING`, `README`, `AUTHORS`, `NEWS`: upstream metadata and license context.

Users do not need a separate Bash source checkout to run the upstream test corpus. The harness defaults to the vendored tests and compiles the small upstream helper programs into a temporary directory at test time. Set `BASH_521_TESTS_DIR=/path/to/bash-5.2.21/tests` only when intentionally comparing against another Bash test tree.

The files under `tests/misc` are vendored too, but they are not part of Bash's normal `make tests` / `tests/run-all` target. They include manual, network, TTY, signal-timing, and performance scripts, so the standard parity gate focuses on the same suite Bash itself runs by default.

## Other Tests

The parity driver also runs CherubSH's own differential fixtures against the Bash 5.2.21 oracle. These cover lifecycle behavior, parser acceptance, expansions, arrays, assignments, builtins, redirections, process substitution, functions, `set -e`, `set -x`, jobs, traps, `read`, `source`, `type`, history, completion, and coprocess behavior.

Regular Cargo unit and integration tests run as part of the same workspace sweep.

## Running

```sh
cargo run -p cherubsh
```

## Testing

```sh
./tools/run-parity.sh
```

`tools/run-parity.sh` runs the full workspace test suite, the CherubSH fixture parity suite, and the vendored upstream Bash 5.2.21 suite.

The parity oracle must be Bash 5.2.21:

- Set `BASH_521_PATH=/path/to/bash-5.2.21` to use an existing oracle binary.
- Or let `oracle/build-bash-5.2.21.sh` build one under `target/oracle/bash-5.2.21`.

The oracle builder can download the Bash 5.2.21 release tarball when no local source tree is supplied. Set `BASH_SRC=/path/to/bash-5.2.21` to build from an existing source tree instead.
