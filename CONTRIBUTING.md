# Contributing

Good contributions make a behavior claim easy to inspect. Keep the code change, its test, and the supporting output close together.

## Before opening a change

1. Find the crate or tool that owns the behavior.
2. Reproduce the current result with the smallest command or fixture you can.
3. Add or update a focused test that describes the expected result.
4. Make the change and run that focused test again.
5. Run formatting, clippy, and the broader gate for the public boundary you changed.

For a shell mismatch, include results from the pinned Bash 5.3.15 build and CherubSH. Give the exact input, output, exit status, and files left behind. That is much easier to verify than a general statement that the shells differ.

## Tests to run

Rust changes should pass these commands:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run the focused upstream, Brush, PTY, Readline, packaging, or fuzz check for the area you touched. The [testing guide](wiki/Testing.md) lists the available commands and filters. Changes to a public compatibility boundary should also pass:

```sh
RUN_BRUSH_PARITY=1 ./tools/run-parity.sh
```

## Issues and pull requests

Use the compatibility issue form when CherubSH and the pinned Bash build produce different observable results. Use the bug form when the problem is specific to CherubSH, its packaging, or its tools. Feature requests should describe the user problem before proposing an interface.

Keep a pull request focused on one issue. Explain what changed, name the public behavior under test, and list the commands you ran. Draft pull requests are welcome when the remaining work is clearly marked.

## Scope and generated files

Do not commit `target/`, `parity.log`, downloaded upstream caches, fuzz artifacts, or local benchmark output. Commit intentional source, fixtures, headers, documentation, and workflows.

Keep unrelated formatting and vendored-source changes out of a behavioral fix. Reference comparisons are easier to review when the diff stays narrow.

## Documentation

The repository root contains release and contributor material. User and developer guides live in `wiki/`. Check wiki changes with:

```sh
./tools/check-wiki-source.sh
```

Install the local hook once per clone with `./tools/install-git-hooks.sh`. After a documentation change reaches `main`, the publish workflow copies the versioned Markdown pages to the GitHub Wiki. Edit the files in this repository rather than the rendered wiki.

## License

CherubSH is GPL-3.0-or-later. Contributions must use a compatible license. Keep notices and license files for vendored material intact.
