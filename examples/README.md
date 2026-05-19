# CherubSH Examples

These scripts are ordinary Bash-style shell scripts that run under CherubSH.

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

Each script uses `#!/usr/bin/env cherubsh`, so it can also be executed directly once `cherubsh` is on `PATH`.
