# Coverage-guided fuzzing

CherubSH keeps five cargo-fuzz targets in this directory. `lexer` scans valid
UTF-8 in several shell modes. `parser` builds independent token streams so it
can check states the lexer would normally reject. `expansion` exercises raw
quote input, structurally bounded brace lists, and small numeric sequences.
`line_input` feeds arbitrary chunk boundaries through the same incremental
terminal decoder used by interactive sessions. `readline_ffi` uses valid
pointers and matching allocator ownership while it changes and queries History
state.

Install the pinned toolchain and cargo-fuzz version used by CI:

```sh
rustup toolchain install nightly-2026-07-30 --profile minimal
cargo +nightly-2026-07-30 install cargo-fuzz --version 0.13.2 --locked
```

Every pull request that changes the fuzzing surface replays the committed seed
corpora:

```sh
./tools/run-fuzz-corpus.sh
```

Run one target for five minutes with its normal corpus:

```sh
cargo +nightly-2026-07-30 fuzz run --fuzz-dir fuzz lexer fuzz/corpus/lexer -- -max_total_time=300 -rss_limit_mb=2048
```

When libFuzzer writes a failure, minimize it, inspect the printable form, and
replay it directly:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-07-30 cargo fuzz tmin --fuzz-dir fuzz lexer fuzz/artifacts/lexer/crash-ID
RUSTUP_TOOLCHAIN=nightly-2026-07-30 cargo fuzz fmt --fuzz-dir fuzz lexer fuzz/artifacts/lexer/minimized-ID
RUSTUP_TOOLCHAIN=nightly-2026-07-30 cargo fuzz run --fuzz-dir fuzz lexer fuzz/artifacts/lexer/minimized-ID
```

Move a minimized input into `fuzz/corpus/<target>/` with a descriptive name.
Then add an ordinary unit or integration test when the failure has a stable,
small assertion. This keeps the regression useful even on systems that do not
have cargo-fuzz installed.
