# Getting started

CherubSH is developed and tested on Linux. The same repository commands work under WSL. It is not a drop-in replacement for `/bin/bash` on a system that depends on a distribution Bash build, so begin with a local binary and a test script.

## Prerequisites

The pinned toolchain is Rust 1.93.1. The ordinary Rust build needs a C compiler and the usual Rust build prerequisites. The full parity gate also builds the upstream Bash and Readline references.

On Debian or Ubuntu, install the full-gate prerequisites with:

```sh
sudo apt-get install \
  autoconf bison build-essential curl git gpgv \
  libncurses-dev patch perl python3 texinfo util-linux
```

Install Rust through your normal Rust toolchain manager. The repository selects its required version through `rust-toolchain.toml`.

## Clone and build

```sh
git clone https://github.com/cupidthecat/cherubsh.git
cd cherubsh
cargo build --release -p cherubsh
```

The binary is `target/release/cherubsh`.

## Run a command and a script

Use `-c` for a command string:

```sh
target/release/cherubsh -c 'printf "%s\n" "$BASH_VERSION"'
```

Run a file by passing its path:

```sh
target/release/cherubsh examples/01-basics.sh
```

The repository has six small examples. Run them through Cargo while you are developing:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
cargo run -p cherubsh -- examples/02-expansion-and-redirection.sh
cargo run -p cherubsh -- examples/03-traps-coproc-and-jobs.sh
cargo run -p cherubsh -- examples/04-log-summary.sh
cargo run -p cherubsh -- examples/05-parallel-checks.sh
cargo run -p cherubsh -- examples/06-completion-and-history.sh
```

The last example intentionally includes a failed child check and exits with status 1 after its report. That result is part of the example.

Run every non-interactive example with the debug binary:

```sh
./tools/check-examples.sh
```

## Set up an interactive configuration

Interactive, non-login CherubSH shells read `~/.cherubrc`. They do not source `~/.bashrc` as a fallback. Copy the starter configuration only when you do not already have a CherubSH configuration:

```sh
./tools/install-cherubrc.sh
```

The installer never replaces a file that already exists. Use `--path FILE` to copy the starter configuration to another location.

## Check the local build

Run the normal workspace tests before trying the slower compatibility gate:

```sh
cargo test --workspace --locked
```

When a generated Bash oracle is absent, local comparison tests use `BASH_PATH` or `/bin/bash`. The full parity driver sets `RUN_PARITY_TESTS=1` and still requires the pinned Bash build.

For the full sequence, fetch and verify the pinned sources, then run the parity driver:

```sh
./tools/fetch-upstream.sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

The full gate builds upstream programs and can take much longer than the workspace tests. [Testing](Testing) lists focused alternatives.

## Build a release archive

Build the release binary, then create a versioned Linux archive and checksum file:

```sh
cargo build --release --locked -p cherubsh
./tools/package-release.sh --version 0.3.0
sha256sum --check dist/SHA256SUMS
```

The shell archive contains the binary, license, README, starter configuration, and configuration installer. A tagged release publishes that archive and a separate Readline development archive for both x86-64 and AArch64. One checksum file covers all four archives. The development archive includes its own prefix-aware installer and component uninstaller; [Readline and History](Readline-and-History) has the commands.

## Next steps

Read [Using CherubSH](Using-CherubSH) for regular script execution. Read [Interactive shell](Interactive-shell) before creating a `~/.cherubrc` file or changing your login shell.
