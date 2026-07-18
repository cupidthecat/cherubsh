# CherubSH Examples

These are regular Bash-style shell scripts that run under CherubSH.

From the repository root:

```sh
cargo run -p cherubsh -- examples/01-basics.sh
cargo run -p cherubsh -- examples/02-expansion-and-redirection.sh
cargo run -p cherubsh -- examples/03-traps-coproc-and-jobs.sh
```

After installing CherubSH:

```sh
cherubsh examples/01-basics.sh
```

Each script uses `#!/usr/bin/env cherubsh`. Once `cherubsh` is on `PATH`, you can also run a script directly.
