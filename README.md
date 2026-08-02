# CherubSH

CherubSH (`cherubsh`), formerly cupidshell, is a Rust implementation of Bash 5.3 behavior. It includes its own UTF-8 line editor and C-compatible GNU Readline and History libraries. Compatibility is checked against pinned builds of Bash 5.3.15 and Readline 8.3 patch 3, plus the Brush and Oils shell test corpora.

CherubSH is developed and tested on Linux. The same build and test commands work under WSL.

## Documentation

The repository keeps the source for its GitHub Wiki in [`wiki/`](wiki/). Read the [wiki home page](wiki/Home.md) from the checkout, or use the GitHub Wiki once it has been enabled and published. [CONTRIBUTING.md](CONTRIBUTING.md) explains the test and pull request workflow, and [CHANGELOG.md](CHANGELOG.md) records user-visible release changes. Contributors should edit the versioned source pages rather than the rendered wiki; [Publishing the wiki](wiki/Publishing-the-wiki.md) explains the validation and publication workflow.

## Compatibility status

The v0.4.0 parity gates report:

| Suite | Result |
| --- | ---: |
| Upstream Bash 5.3 `run-*` drivers | 86 / 86 passing |
| CherubSH differential fixtures | 99 / 99 passing |
| Oils OSH cases compared with Bash 5.3.15 | 2,332 exact matches; 472 tracked differences |
| Runnable Brush compatibility cases | 2,104 / 2,104 passing |
| Brush cases skipped by their metadata or Bash version | 1 |

The upstream Bash gate uses the original `.right` files. The Oils gate runs 2,804 OSH spec cases in a Bubblewrap sandbox and compares raw status, standard output, standard error, and timeout state with Bash 5.3.15. Its checked-in ratchet records the mismatch fields and, by default, exact Bash and CherubSH fingerprints for each known difference. A changed failure, a new failure, or an unexpected pass stops CI. A short manifest limits variable fingerprints to cases affected by timing, process data, output ordering, or `$RANDOM`. The Bash fingerprint, the CherubSH fingerprint, or both may be variable for those named cases. Their mismatch fields stay exact, and a case with a variable CherubSH fingerprint may either match or differ on a given run. The Brush gate compares status, output, and files left behind. Two `read -t 0` pipeline cases are labeled `ported-nondeterministic` because Bash can report either readiness result depending on process scheduling; only those documented results are accepted. Readline tests compile the same C fixtures and upstream examples against GNU Readline and the CherubSH library.

Bash is used only as a test oracle. CherubSH does not call Bash to parse commands, print syntax trees, extract translation strings, expand completions, or run loadable builtins.

## What works

- Bash parsing and execution for functions, pipelines, subshells, coprocesses, background jobs, traps, redirections, here-documents, and here-strings.
- Parameter, arithmetic, command, process, brace, pathname, and tilde expansion, including indexed arrays, associative arrays, namerefs, quoting, word splitting, and pattern operators.
- Bash shell options and `shopt` settings covered by the Bash and Brush compatibility suites.
- Native `--pretty-print`, `--dump-strings`, `--dump-po-strings`, and `-D` invocation modes for files, standard input, and `-c` command strings.
- The standard builtins used by the upstream tests, including job control, history, programmable completion, `read`, `mapfile`, `printf`, `test`, `source`, and `wait -n -p`.
- Bash 5.3.15 loadable builtins through `enable -f`, including scalar, indexed-array, associative-array, input, and child-shell ABI calls used by Bash's example modules.
- Interactive Emacs and Vi keymaps, UTF-8 cursor movement, undo, kill/yank behavior, history search, terminal-width-aware redisplay, and programmable completion. Repeated Tab presses and the `show-all-if-ambiguous` and `show-all-if-unmodified` inputrc settings follow GNU Readline's completion display rules.
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

`cherubsh --version` prints the CherubSH package version first, followed by the Bash compatibility version and target. `BASH_VERSION` stays at `5.3.15(1)-release` for scripts that inspect Bash compatibility.

Parser output and translation extraction run without executing the input:

```sh
target/release/cherubsh --pretty-print -c 'for name in one two; do echo "$name"; done'
target/release/cherubsh --dump-strings messages.sh
target/release/cherubsh --dump-po-strings messages.sh
```

## Interactive setup

CherubSH reads `~/.cherubrc` for an interactive non-login shell. It does not silently fall back to `~/.bashrc`. Use `--norc` to skip the file or `--rcfile path` to choose another one.

A starter file lives at `examples/cherubrc`. Copy it when you do not already have a configuration:

```sh
./tools/install-cherubrc.sh
```

The installer refuses to replace an existing file. Pass `--path FILE` when you want to place the starter file somewhere else.

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
  -lreadline
```

The staged directory also works with `pkg-config`:

```sh
export PKG_CONFIG_PATH="$PWD/target/readline/lib/pkgconfig"
pkg-config --cflags --libs readline
pkg-config --cflags --libs history
```

`examples/readline-client.c` is a small interactive client that uses both libraries. Build it after staging the compatibility files:

```sh
cc examples/readline-client.c \
  -Itarget/readline/include \
  -Ltarget/readline/lib \
  -Wl,-rpath,"$PWD/target/readline/lib" \
  -lreadline -lhistory \
  -o target/readline-client
target/readline-client
```

Release builds also provide a separate `cherubsh-readline-dev` archive for each supported architecture. Extract that archive and install both components under a prefix with:

```sh
sudo ./tools/install-readline-dev.sh install --component all --prefix /usr/local
```

Package builders can stage the same installation without writing to the live prefix:

```sh
DESTDIR="$PWD/package-root" \
  ./tools/install-readline-dev.sh install --component all --prefix /usr
```

The installer records the files owned by each component. Use those records to remove Readline, History, or both while leaving unrelated files alone:

```sh
sudo ./tools/install-readline-dev.sh uninstall --component readline --prefix /usr/local
sudo ./tools/install-readline-dev.sh uninstall --component history --prefix /usr/local
```

To create the development archive from a checkout, build the libraries first, then run the development packager:

```sh
./tools/build-readline.sh
./tools/package-readline-dev.sh --version 0.4.0
```

Run the GNU differential gate with:

```sh
./tools/run-readline-parity.sh
```

That command builds GNU Readline 8.3 patch 3, checks public symbol coverage and library names, and compiles the same C fixtures against both implementations. The C tests compare structure layout, constants, allocator ownership, callback setup and teardown, redisplay hooks, custom streams, completion display policy, inputrc conditionals, and saved History state. Ownership-sensitive fixtures also run under AddressSanitizer. The gate still covers the pseudo-terminal Readline loop, custom bindings, macros, bare keymaps, and every deterministic upstream example. Reports are kept under `target/parity/readline`.

## Testing

Run the ordinary Rust test suite first:

```sh
./tools/run-workspace-tests.sh
```

The runner verifies or builds the pinned Bash 5.3.15 oracle under `target/oracle`
before invoking `cargo test --workspace --locked`. It rejects an explicit
`BASH_ORACLE_PATH` that does not report that exact patch version, so local
differential tests cannot silently use a distribution Bash with different
semantics.

The first run also needs the build dependencies listed below.

The full parity gate needs common build tools, `bison`, `texinfo`, `gpgv`, ncurses development headers, Perl, Python 3, util-linux, and Bubblewrap. On Debian or Ubuntu:

```sh
sudo apt-get install \
  autoconf bison bubblewrap build-essential curl git gpgv \
  libncurses-dev patch perl python3 texinfo util-linux
```

Fetch and verify the pinned sources, then run every gate:

```sh
./tools/fetch-upstream.sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

`tools/fetch-upstream.sh` checks the recorded tag objects, SHA-256 hashes, and GNU patch signatures before preparing Bash 5.3.15 and Readline 8.3 patch 3. The exact references live in `upstream.lock` and `upstream.sha256`.

The main driver builds a Bash 5.3.15 oracle under `target/oracle`, runs the Rust workspace, Oils, and upstream Bash suites, and finishes with the Readline gate. Add `RUN_BRUSH_PARITY=1` to include all 2,105 Brush cases, as shown above.

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

# Oils cases whose stable ID contains this text
RUN_OILS_PARITY=1 \
OILS_PARITY_FILTER='command-sub.test.sh' \
cargo test -p cherubsh --test oils_parity -- --nocapture

# Bash loadable ABI only, after building the oracle modules
oracle/build-bash-5.3.15-loadables.sh
RUN_LOADABLE_PARITY=1 \
cargo test -p cherubsh --test loadable_abi -- --nocapture

# Readline and History only
./tools/run-readline-parity.sh
```

Parity reports are written below `target/parity`.

## Hardening checks

The generated differential fuzzer creates small, bounded shell programs and compares their status, standard output, and standard error with the pinned Bash oracle. Run the full parity driver first so the oracle exists, then run a local batch:

```sh
FUZZ_CASES=250 ./tools/run-fuzz-smoke.sh
```

Failures can be saved for inspection without changing the checked-in corpus:

```sh
FUZZ_CASES=250 \
FUZZ_ARTIFACT_DIR=target/hardening/fuzz \
./tools/run-fuzz-smoke.sh
```

The PTY differential runner opens isolated interactive sessions under the pinned Bash oracle and CherubSH. Its scenarios cover resize handling, job control, pipelines, EOF, Unicode editing, bracketed paste, Vi and Emacs modes, completion, and interrupt recovery. It also exercises Bash's manual `read -n` and `/dev/tty` redirection cases. The JSON report records the values and state that matter for each scenario. Raw and normalized transcripts remain available when a terminal failure needs closer inspection.

```sh
python3 tools/pty-differential.py \
  --bash target/oracle/bash-5.3.15/bash \
  --cherub target/debug/cherubsh \
  --report-dir target/hardening/pty
```

Pass `--list` to print the scenario names or repeat `--scenario NAME` to run a subset. `tools/pty-stress.py` repeats the interrupt-recovery scenario:

```sh
python3 tools/pty-stress.py \
  --bash target/oracle/bash-5.3.15/bash \
  --cherub target/debug/cherubsh \
  --rounds 20
```

The scheduled `hardening` workflow runs the generated comparisons, the PTY matrix, the repeated interrupt check, and an AddressSanitizer build. To run the sanitizer check locally, install nightly Rust with `rust-src` and use the same command as CI:

```sh
rustup toolchain install nightly --component rust-src
CHERUBSH_C_SANITIZER=address \
CC=clang \
RUSTFLAGS='-Zsanitizer=address' \
cargo +nightly test -Zbuild-std \
  --target x86_64-unknown-linux-gnu \
  --workspace --locked
```

## Reference source trees

- `vendor/readline-8.3` contains the user-supplied Readline 8.3 source used to build the GNU oracle.
- `vendor/bash-5.3.15/tests` contains the checked-in Bash test corpus and expected output files.
- `vendor/brush` contains the Brush compatibility YAML at commit `5a50c12ed59e610dae038db9acf642286c585e2d`.
- `target/upstream` holds verified source caches created by `tools/fetch-upstream.sh`.
- `target/oracle` holds local GNU builds. It is generated and ignored by Git.

Bash's `make tests` and `tests/run-all` targets do not enter `tests/misc`, so CherubSH tracks that directory separately in `crates/test-harness/bash-misc-cases.txt`. Ten deterministic scripts run through the Rust or PTY parity tests. The signal cases use shorter sleeps, and `wait-bg.tests` removes its four-second delay. `/dev/tcp` tests talk only to a loopback server and cover numeric ports, host and service lookup, bidirectional I/O, assigned descriptors, and connection errors.

`perf-script` and `perftest` are benchmark inputs, not correctness tests. The benchmark driver runs fixed copies in its temporary workspace. In particular, `perftest` scans a generated directory instead of the runner's `/usr/lib` tree.

The weekly `benchmarks` workflow keeps each run's raw samples, summary, and metadata for 90 days. The metadata records the commit, runner and CPU, Rust toolchain, Bash oracle version, and the hashes of both Cargo lockfiles. These reports establish a baseline before the project chooses performance limits. There is no performance pass or fail threshold for v0.4.0.

Run the same two upstream-derived cases locally with one measured sample:

```sh
RUNS=1 WARMUPS=0 \
BENCH_CASES=bash_perf_script,bash_perftest \
./tools/bench.sh
```

By default, reports are written under `target/bench`. Set `BENCH_OUTPUT_DIR` to keep a separate run.

```sh
cargo test -p cherubsh --test misc_parity
cargo test -p cherubsh --test phase6_tcp
```

## Examples

The scripts under `examples/` cover everyday shell use, expansion and redirection, and process management:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
cargo run -p cherubsh -- examples/02-expansion-and-redirection.sh
cargo run -p cherubsh -- examples/03-traps-coproc-and-jobs.sh
cargo run -p cherubsh -- examples/04-log-summary.sh
cargo run -p cherubsh -- examples/05-parallel-checks.sh
cargo run -p cherubsh -- examples/06-completion-and-history.sh
```

Check every non-interactive example with the debug binary:

```sh
./tools/check-examples.sh
```

## Installing as a shell

CherubSH supports Linux and WSL. The v0.4.0 shell archive includes an installer for the binary, manuals, and Bash-compatible command completion. Replace `VERSION` and `TARGET` below with the names on the release asset:

```sh
tar -xzf cherubsh-VERSION-TARGET.tar.gz
cd cherubsh-VERSION-TARGET
sudo ./tools/install-cherubsh.sh install --prefix /usr/local
cherubsh --version
man cherubsh
```

The same command works under WSL. For a staged package build, pass `--destdir PATH`. To remove the installed files, replace `install` with `uninstall` and use the same prefix and DESTDIR. The installer refuses to replace a file it does not own.

The archive installs `cherubsh(1)`, `cherubsh-readline(3)`, and `cherubshrc(5)`. Its completion file is placed at `share/bash-completion/completions/cherubsh` below the selected prefix.

For a versioned Linux archive, build the release binary and package it with a version of your choice:

```sh
cargo build --release --locked -p cherubsh
./tools/package-release.sh --version 0.4.0
sha256sum --check dist/SHA256SUMS
```

The release workflow runs when a `v*` tag is pushed. The tag must match the workspace package version, so package version `0.4.0` is released from tag `v0.4.0`. The workflow checks this before testing or building. Each supported architecture gets a shell archive and a Readline development archive. The release also includes CycloneDX SBOMs and one checksum file for every archive and SBOM. The shell binary remains at the top level of its archive, so it can run without installation.

After downloading an asset and `SHA256SUMS`, verify both the checksum and the GitHub build provenance:

```sh
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify cherubsh-VERSION-TARGET.tar.gz \
  --repo cupidthecat/cherubsh \
  --signer-workflow cupidthecat/cherubsh/.github/workflows/release.yml
```

Shell archives also carry the CycloneDX SBOM attestation. Verify that claim separately:

```sh
gh attestation verify cherubsh-VERSION-TARGET.tar.gz \
  --repo cupidthecat/cherubsh \
  --signer-workflow cupidthecat/cherubsh/.github/workflows/release.yml \
  --predicate-type https://cyclonedx.org/bom
```

See [SECURITY.md](SECURITY.md) for the supported release line and private reporting instructions.

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
- [Oils](https://github.com/oils-for-unix/oils) provides the Apache-2.0 OSH spec cases under `vendor/oils`.
- [bash-completion](https://github.com/scop/bash-completion) provides the GPL-2.0-or-later completion corpus used for compatibility testing.
- [shellgei/rusty_bash](https://github.com/shellgei/rusty_bash) is useful related work on a Bash-compatible shell in Rust.

CherubSH itself is licensed under GPL-3.0-or-later. See `LICENSE` and the license files kept with each vendored source tree.
