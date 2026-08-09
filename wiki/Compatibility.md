# Compatibility

CherubSH aims to reproduce Bash 5.3 behavior and GNU Readline and History behavior at their public boundaries. The repository pins its reference material so that the expected behavior does not move between runs.

## Reference versions

| Component | Pinned reference |
| --- | --- |
| Bash | 5.3.15 |
| GNU Readline | 8.3 patch 3 |
| bash-completion | 2.18.0 |
| Brush fixtures | Commit `5a50c12ed59e610dae038db9acf642286c585e2d` |
| Real-world shell projects | 88 commits recorded in `large-scripts.lock` |

`upstream.lock` records the repositories, tags, tag objects, and patch levels. `upstream.sha256` records the hashes for release patches, signatures, and the GNU keyring used by the fetch script.

## What the suites compare

The upstream Bash gate runs the original `run-*` drivers and their `.right` expected-output files. CherubSH differential fixtures compare shell behavior against the pinned Bash build. The Brush gate runs each eligible case once with Bash and once with CherubSH, then compares status, output, and files left behind.

The Readline gate compiles shared fixtures and upstream examples against GNU Readline and against the CherubSH libraries. It checks observable C-library behavior, symbols, names, and deterministic output.

The ordinary Rust test suite catches component-level regressions. It is necessary, but it does not replace a behavior comparison against the pinned reference.

## Real-world program coverage

The hardening corpus pins 88 projects used as shells, sourced startup code, command-line tools, test frameworks, version managers, and administration programs. Its no-execution gate selects 6,044 regular shell files and checks their parse result against Bash 5.3.15. Extensionless files with Bash, `sh`, or `dash` shebangs are included.

The companion smoke matrix runs one reviewed fixture per project. Command fixtures use safe help, version, or validation paths. Startup packages and version-manager modules are sourced. Bashtop uses a short pseudo-terminal startup fixture because it has no noninteractive help path. Every fixture runs twice in a fresh Bubblewrap namespace, once with the pinned Bash oracle and once with CherubSH. The comparison includes status, standard output, standard error, timeout state, and deterministic files.

The namespace has no network access. Its home and work directories are temporary, and the fetched Git tree is read-only. This keeps installers and system tools away from the host. The matrix does not exercise live package installation, certificates, VPN setup, backups, containers, clusters, or deployments. Those jobs still belong in a disposable VM with their external dependencies installed.

## What Bash is and is not used for

Bash is a test oracle only. CherubSH does not delegate parsing, syntax-tree printing, translation-string extraction, completion expansion, or loadable-builtin execution to Bash. A test may build and run Bash beside CherubSH, then compare outputs. The CherubSH binary still exercises its own code path.

## Loadable builtins

The parity driver builds the example loadable modules shipped with Bash 5.3.15. It checks that each detected builtin can load, print help, run, and unload under CherubSH. Extra C fixtures cover scalar and array data exchange, line input, subscript parsing, and the child shell started by Bash's `push` example.

After the oracle modules are available, load one manually:

```sh
BASH_LOADABLES_PATH=target/oracle/bash-5.3.15/examples/loadables \
  target/release/cherubsh --norc -c '
    enable -f printenv printenv
    printenv HOME
    enable -d printenv
  '
```

## Limits of a passing result

A passing test suite describes the cases that ran on that revision and host. Terminal behavior, kernel signals, locale, filesystem timing, and dependencies can expose cases outside the suite. Reproduce a difference with the smallest script you can, run it under the pinned Bash oracle and CherubSH, and keep the exact status, standard output, standard error, and remaining files.

Bash's own aggregate test targets skip `vendor/bash-5.3.15/tests/misc`. CherubSH classifies every file in that directory and automates the ten deterministic scripts with bounded substitutions for long sleeps and external input. The terminal cases run in isolated PTYs. The network case uses a local TCP server, never the public hosts named by the original script.

`perf-script` and `perftest` remain benchmark inputs. They run with the scheduled performance suite instead of the correctness gate.
