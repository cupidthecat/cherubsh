# Vendored Brush tests

This directory contains the YAML compatibility cases from Brush:

- Source: https://github.com/reubeno/brush/tree/main/brush-shell/tests/cases
- Commit: `5a50c12ed59e610dae038db9acf642286c585e2d`
- Date: 2026-05-17
- License: MIT; see `LICENSE`

CherubSH runs `brush-shell/tests/cases/compat` against the pinned Bash 5.3.15 oracle. The files under `brush-shell/tests/cases/brush` remain here for source context, but they test Brush-specific command-line behavior and are not part of CherubSH's Bash parity gate.

Run the compat sweep with:

```sh
RUN_BRUSH_PARITY=1 cargo test -p cherubsh --test brush_parity -- --nocapture
```

Use `BRUSH_PARITY_FILTER` to run a subset by qualified case-name substring.

The full corpus currently has 2,105 cases: 2,077 pass, 28 are skipped by Brush metadata or Bash-version constraints, and none fail.
