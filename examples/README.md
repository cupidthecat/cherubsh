# CherubSH examples

These scripts are small enough to read before running. Together they cover functions and arrays, expansion and redirection, and process management.

From the repository root:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
cargo run -p cherubsh -- examples/02-expansion-and-redirection.sh
cargo run -p cherubsh -- examples/03-traps-coproc-and-jobs.sh
cargo run -p cherubsh -- examples/04-log-summary.sh
cargo run -p cherubsh -- examples/05-parallel-checks.sh
```

After installing CherubSH:

```sh
cherubsh examples/01-basics.sh
```

Each script uses `#!/usr/bin/env cherubsh`. Once `cherubsh` is on `PATH`, you can also run a script directly.

- `01-basics.sh` uses functions, indexed and associative arrays, loops, and `case`.
- `02-expansion-and-redirection.sh` uses parameter and command expansion, here-documents, and process substitution.
- `03-traps-coproc-and-jobs.sh` uses traps, background jobs, `wait`, and a named coprocess.
- `04-log-summary.sh` turns an access log into stable status, path, and latency summaries. Pass another log path to analyze your own data.
- `05-parallel-checks.sh` tracks concurrent checks by PID and collects them with `wait -n -p`. Its sample includes one failed check, so it exits with status 1 after printing the report.
