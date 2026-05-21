# Bash 5.3 Test Vendor

This directory vendors the Bash 5.3 test suite for CherubSH parity testing.

- `tests/` is the upstream Bash 5.3 `tests` tree.
- `support/` contains the upstream C sources for helper programs needed by the tests.
- `examples/loadables/` contains the upstream loadable examples needed by selected tests.
- `bashansi.h`, `y.tab.c`, `config.h`, `version.h`, `COPYING`, `README`, `AUTHORS`, and `NEWS` come from the Bash 5.3 release/build tree.

Built helper binaries are intentionally not checked in. The CherubSH test harness compiles `recho`, `zecho`, `printenv`, and `xcase` into a temporary directory when the vendored test tree does not already provide executable helpers.
