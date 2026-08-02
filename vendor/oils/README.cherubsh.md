# Vendored Oils specs

This directory contains the OSH spec corpus from Oils:

- Source: https://github.com/oils-for-unix/oils/tree/master/spec
- Commit: `15de8fd779569e6e3a9f5fcbfc00e7df0ebe0380`
- License: Apache-2.0; see `LICENSE.txt`

CherubSH selects the 135 files whose `compare_shells` metadata includes Bash. Those files contain 2,804 cases. The native Rust harness runs every selected case against Bash 5.3.15 and CherubSH inside separate Bubblewrap sandboxes, then compares raw output and process status.

Refresh the corpus with:

```sh
./tools/vendor-oils.sh
```

The refresh script fetches the exact commit from `upstream.lock`, applies `tools/oils-python3.patch`, and checks the file and case counts before replacing the vendored tree. The patch only updates helper calls needed to run these specs with Python 3.

Run the full Oils gate with:

```sh
RUN_OILS_PARITY=1 cargo test -p cherubsh --test oils_parity -- --nocapture
```

Use `OILS_PARITY_FILTER` to select cases by stable ID. Reports and raw mismatch artifacts are written below `target/parity/oils` unless `OILS_PARITY_REPORT_DIR` points elsewhere.
