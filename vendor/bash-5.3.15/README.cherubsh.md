# Vendored Bash tests

This directory keeps the Bash 5.3.15 files used by CherubSH's upstream parity suite.

- `tests/` contains the upstream test drivers, scripts, and expected output.
- `support/` has the C sources for test helpers.
- `examples/loadables/` has the loadable examples required by a few tests.
- The files at the directory root come from the Bash release and build trees.

Helper binaries are not checked in. The test harness builds `recho`, `zecho`, `printenv`, and `xcase` in a temporary directory when they are needed.
