# Vendored brush tests

This directory vendors the YAML shell compatibility cases from the brush
project:

- Source: https://github.com/reubeno/brush/tree/main/brush-shell/tests/cases
- Commit: `5a50c12ed59e610dae038db9acf642286c585e2d`
- Date: 2026-05-17
- License: MIT; see `LICENSE`

CherubSH runs the `brush-shell/tests/cases/compat` corpus against the local
Bash 5.2.21 oracle. The `brush-shell/tests/cases/brush` files are retained as
source context, but they exercise brush-specific CLI behavior and are not part
of the Bash-compatibility parity gate.

Run the compat sweep with:

```sh
RUN_BRUSH_PARITY=1 cargo test -p cherubsh --test brush_parity -- --nocapture
```

Use `BRUSH_PARITY_FILTER` to run a subset by qualified case-name substring.
