# Testing

Start with the smallest command that can prove the work. Run the full parity driver when a change affects behavior that it covers, or before a release boundary.

## Fast workspace checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
./tools/run-workspace-tests.sh
```

The repository CI runs these checks as part of its parity job. Format and clippy failures are worth fixing before a large oracle run.

The workspace runner verifies or builds Bash 5.3.15 under `target/oracle` and
then invokes `cargo test --workspace --locked`. Live comparisons require that
exact oracle; they do not fall back to the system Bash.

## Full parity gate

Fetch and verify the external source material first:

```sh
./tools/fetch-upstream.sh
```

Then run the main driver. Set `RUN_BRUSH_PARITY=1` to include all eligible Brush cases:

```sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

The driver builds a Bash 5.3.15 oracle under `target/oracle`, runs the Rust workspace tests, the Oils OSH corpus, and the upstream Bash suites, then finishes with the Readline gate. It writes reports below `target/parity`; the CI job also preserves those reports as an artifact. Bubblewrap is required for the Oils sandbox.

## Focused checks

Run selected upstream Bash drivers by name:

```sh
RUN_UPSTREAM_PARITY=1 \
UPSTREAM_PARITY_FILTER='history,jobs' \
cargo test -p cherubsh --test upstream_parity -- --nocapture
```

Run Brush cases whose qualified names contain a string:

```sh
RUN_BRUSH_PARITY=1 \
BRUSH_PARITY_FILTER='Builtins: wait' \
cargo test -p cherubsh --test brush_parity -- --nocapture
```

Run Oils cases whose stable ID contains a string:

```sh
RUN_OILS_PARITY=1 \
OILS_PARITY_FILTER='command-sub.test.sh' \
cargo test -p cherubsh --test oils_parity -- --nocapture
```

Set `OILS_PARITY_JOBS` to cap worker threads or `OILS_PARITY_REPORT_DIR` to move the report. The default report directory is `target/parity/oils`. `report.tsv` records every verdict, and `failures/` keeps raw Bash and CherubSH streams for each mismatch. `observed-ratchet-<arch>.tsv` contains the current architecture's mismatch observations. Review those rows and merge intentional changes into the checked-in ratchet; the file is not a complete cross-architecture replacement.

The checked-in ratchet tracks known observations. It normally stores exact Bash and CherubSH fingerprints. Cases named in `crates/test-harness/oils-nondeterministic-cases.txt` may use a variable fingerprint for Bash, CherubSH, or both when timing, process data, random values, or output ordering changes between runs. Their mismatch fields remain exact. The gate fails on a new mismatch (`FAIL`), a changed known mismatch (`DRIFT`), a fixed known mismatch (`XPASS`), or an entry with no matching case (`STALE`). A case with a variable CherubSH fingerprint may also pass on a given run without becoming an `XPASS`. Remove other `XPASS` entries instead of leaving resolved behavior in the baseline.

Run the C-library compatibility checks only:

```sh
./tools/run-readline-parity.sh
```

Run the Bash loadable ABI suite after building the Bash 5.3.15 modules:

```sh
oracle/build-bash-5.3.15-loadables.sh
RUN_LOADABLE_PARITY=1 \
cargo test -p cherubsh --test loadable_abi -- --nocapture
```

## Hardening checks

The generated fuzzer runs small shell programs against CherubSH and the pinned Bash oracle. First build the debug binary and the oracle, then run a local batch:

```sh
cargo build --locked -p cherubsh
FUZZ_CASES=250 ./tools/run-fuzz-smoke.sh
```

Save a failing case and its Bash and CherubSH output with `FUZZ_ARTIFACT_DIR`:

```sh
FUZZ_CASES=250 \
FUZZ_ARTIFACT_DIR=target/hardening/fuzz \
./tools/run-fuzz-smoke.sh
```

The PTY differential runner starts isolated interactive Bash and CherubSH sessions. Its scenarios cover resize handling, job control, pipelines, EOF, Unicode editing, bracketed paste, Vi and Emacs modes, completion, and interrupt recovery. The report compares scenario-specific observations while keeping raw and normalized transcripts for redraw analysis.

```sh
python3 tools/pty-differential.py \
  --bash target/oracle/bash-5.3.15/bash \
  --cherub target/debug/cherubsh \
  --report-dir target/hardening/pty
```

Use `--list` to see the scenario catalog or repeat `--scenario NAME` to select cases. The compatibility wrapper repeats the interrupt-recovery case:

```sh
python3 tools/pty-stress.py \
  --bash target/oracle/bash-5.3.15/bash \
  --cherub target/debug/cherubsh \
  --rounds 20
```

The scheduled hardening workflow runs the generated comparisons, the PTY matrix, the repeated interrupt check, and an AddressSanitizer workspace test. Run the sanitizer check locally with nightly Rust and the `rust-src` component:

```sh
rustup toolchain install nightly --component rust-src
CHERUBSH_C_SANITIZER=address \
CC=clang \
RUSTFLAGS='-Zsanitizer=address' \
cargo +nightly test -Zbuild-std \
  --target x86_64-unknown-linux-gnu \
  --workspace --locked
```

## Test results and generated files

The fetch, build, and parity commands create generated content below `target/`, which Git ignores. `target/upstream` holds verified source caches. `target/oracle` holds local GNU builds. `target/parity` holds reports.

Do not delete vendored source or expected-output material just because a local run generates an additional copy elsewhere. The vendored files are part of the test oracle checked into the repository.

## A useful failure report

For shell behavior, include the invocation, the minimal input, the expected Bash result, the CherubSH result, exit statuses, standard output, standard error, and any files that remain. For terminal cases, say whether a pseudo-terminal was involved. For Readline, include the compiler command and the headers and shared library resolved at build and runtime.
