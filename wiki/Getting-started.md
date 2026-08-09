# Getting started

CherubSH is developed and tested on Linux. The same repository commands work under WSL. Official release archives target `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, and they require glibc. On a different Linux ABI, build from source; those environments are not part of release testing. CherubSH is not a drop-in replacement for `/bin/bash` on a system that depends on a distribution Bash build, so begin with a local binary and a test script.

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
./tools/run-workspace-tests.sh
```

The runner builds the verified Bash 5.3.15 oracle under `target/oracle` when it
is missing, then runs the ordinary Cargo workspace tests. It never replaces
the distribution's `/bin/bash`.

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
./tools/package-release.sh --version 0.4.0
sha256sum --check dist/SHA256SUMS
```

The v0.4.0 shell archive contains the binary, manuals, Bash-compatible command completion, license, README, starter configuration, and installers. A tagged release publishes that archive and a separate Readline development archive for both x86-64 and AArch64. These archives use the GNU ABI and require glibc. One checksum file covers all four archives. The development archive includes its own prefix-aware installer and component uninstaller; [Readline and History](Readline-and-History) has the commands.

On Linux or WSL, extract the shell archive and install its public files under an absolute prefix:

```sh
tar -xzf cherubsh-VERSION-TARGET.tar.gz
cd cherubsh-VERSION-TARGET
sudo ./tools/install-cherubsh.sh install --prefix /usr/local
cherubsh --version
man cherubsh
```

Replace `VERSION` and `TARGET` with the names on the release asset.

Pass `--destdir PATH` when building a staged package. Run the same command with `uninstall` to remove files owned by that prefix installation. The installer leaves unrelated files alone. It installs `cherubsh(1)`, `cherubsh-readline(3)`, `cherubshrc(5)`, and the completion file under `share/bash-completion/completions`.

## Next steps

Read [Using CherubSH](Using-CherubSH) for regular script execution. Read [Interactive shell](Interactive-shell) before creating a `~/.cherubrc` file or changing your login shell.
