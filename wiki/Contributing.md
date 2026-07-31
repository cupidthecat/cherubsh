# Contributing

Good contributions make a behavior claim easy to inspect. Keep the code change, the test, and the evidence close together.

## Before opening a change

1. Find the crate that owns the behavior.
2. Reproduce the current result with the smallest command or fixture.
3. Add or update the focused test that demonstrates the expected result.
4. Run formatting, clippy, and the relevant test command.
5. Run a broader parity gate when the change reaches a public compatibility boundary.

For a shell mismatch, include the Bash 5.3.15 result and the CherubSH result. A vague statement that something is unlike Bash is hard to verify. Exact input, output, status, and remaining files make the report useful.

## Tests to run

At minimum for Rust changes:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run a focused upstream, Brush, or Readline parity check for the affected area. [Testing](Testing) shows the supported commands and filters.

## Scope and generated files

Do not commit `target/`, `parity.log`, downloaded upstream caches, or local profiling output. Do commit intentional source, fixture, header, documentation, and workflow changes.

Keep unrelated formatting or vendored-source changes out of a behavioral fix. They make the reference comparison harder to review.

## Documentation

Edit `wiki/` for user and contributor documentation. The repository validates it with `./tools/check-wiki-source.sh`. Install the local hook once per clone:

```sh
./tools/install-git-hooks.sh
```

After a valid documentation change lands on `main`, the publish workflow mirrors the Markdown pages to the GitHub Wiki. Do not edit the rendered GitHub Wiki directly; the next publish removes changes that are not represented in `wiki/`.

## License

CherubSH is GPL-3.0-or-later. Contributions must be compatible with that license. Keep notices and license files for vendored material intact.
