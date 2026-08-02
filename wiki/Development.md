# Development

Work from the repository root. Keep changes narrow enough that the relevant test can explain what passed.

## Day-to-day loop

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
./tools/run-workspace-tests.sh
```

When a change affects shell behavior, add or update the smallest fixture that proves it. Then use the appropriate focused oracle command before the full parity run. [Testing](Testing) has the filters for upstream and Brush cases.

## Source layout

Use the crate boundaries as the first place to look:

- Token or syntax behavior belongs in `crates/lexer` or `crates/parser`.
- Word and parameter behavior belongs in `crates/expander`.
- Process, redirection, control-flow, function, or trap behavior belongs in `crates/exec`.
- Builtin behavior belongs in `crates/builtins`.
- Shell startup, prompts, completion, interactive loops, and job-control setup belong in `crates/shell`.
- Terminal editing belongs in `crates/lineedit`.
- Public C-library behavior belongs in the FFI crates and the `include/readline` headers.

Avoid putting a compatibility workaround in an unrelated layer. It may make a narrow fixture pass while breaking a different public path.

## Pinned upstream material

Run `./tools/fetch-upstream.sh` instead of downloading test sources by hand. It checks the recorded tag objects, SHA-256 hashes, and GNU patch signatures before preparing Bash and Readline. The source cache and oracle builds belong under `target/`; they are generated output.

The checked-in Bash tests, Readline source, and Brush corpus are test inputs. Do not regenerate or edit expected output to hide a behavior difference without showing why the reference itself changed.

## Code style

Use the Rust formatter. The CI uses `cargo clippy --workspace --all-targets --locked -- -D warnings`, so a new warning fails the lint stage. Prefer explicit state transitions and behavior tests over broad refactors while closing a parity gap.

## Documentation changes

The Markdown files in `wiki/` are versioned documentation. Run:

```sh
./tools/check-wiki-source.sh
```

Install the repository hook once per clone to run that check automatically before commits that stage wiki changes:

```sh
./tools/install-git-hooks.sh
```

The hook checks the staged wiki, not an accidental mixture of staged and unstaged files. A push of a valid wiki change to `main` publishes the pages after CI workflow validation. [Publishing the wiki](Publishing-the-wiki) covers the remote setup.
