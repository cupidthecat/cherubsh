# Testing

Start with the smallest command that can prove the work. Run the full parity driver when a change affects behavior that it covers, or before a release boundary.

## Fast workspace checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The repository CI runs these checks as part of its parity job. Format and clippy failures are worth fixing before a large oracle run.

## Full parity gate

Fetch and verify the external source material first:

```sh
./tools/fetch-upstream.sh
```

Then run the main driver. Set `RUN_BRUSH_PARITY=1` to include all eligible Brush cases:

```sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

The driver builds a Bash 5.3.15 oracle under `target/oracle`, runs the Rust workspace tests and upstream Bash suites, and finishes with the Readline gate. It writes reports below `target/parity`; the CI job also preserves those reports as an artifact.

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

Run the C-library compatibility checks only:

```sh
./tools/run-readline-parity.sh
```

## Test results and generated files

The fetch, build, and parity commands create generated content below `target/`, which Git ignores. `target/upstream` holds verified source caches. `target/oracle` holds local GNU builds. `target/parity` holds reports.

Do not delete vendored source or expected-output material just because a local run generates an additional copy elsewhere. The vendored files are part of the test oracle checked into the repository.

## A useful failure report

For shell behavior, include the invocation, the minimal input, the expected Bash result, the CherubSH result, exit statuses, standard output, standard error, and any files that remain. For terminal cases, say whether a pseudo-terminal was involved. For Readline, include the compiler command and the headers and shared library resolved at build and runtime.
