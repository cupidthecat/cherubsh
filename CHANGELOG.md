# Changelog

This file records user-visible changes. Dates for published versions match their GitHub releases.

## Unreleased

### Added

- Oils OSH parity coverage for 2,804 cases, with sandboxed execution and a reviewed mismatch ratchet.
- Safe parsing checks for 6,044 shell files from 88 pinned real-world projects.
- A Bubblewrap smoke matrix that compares one reviewed entrypoint fixture from each pinned project with Bash 5.3.15.

### Changed

- Workspace tests now provision and require the Bash 5.3.15 oracle instead of comparing against a different system Bash.

### Fixed

- Parser handling for nested shell delimiters, multiline arithmetic, extended globs, case patterns, and large no-execution inputs.
- Real-world compatibility gaps in quoted pathname prefixes, continued control operators, adjacent arithmetic parentheses, literal command-substitution text, deblanked here-documents, noninteractive `bind`, and repeated `read` calls.
- The scheduled AddressSanitizer job now prepares its pinned Bash oracle and links installed-library test clients with the matching sanitizer runtime.
- Oils sandbox tests no longer leak their fixture path allocations.
- Several nondeterministic Oils comparisons involving signals, background jobs, pipelines, here-documents, and scheduler timing.

## 0.4.0 - 2026-08-01

### Added

- Differential PTY coverage for editing, terminal resizing, job control, EOF, paste, completion, and interrupt recovery.
- Runnable classifications for the previously skipped Brush background-job, coprocess, trap, and `read` cases, plus deterministic Bash `tests/misc` coverage.
- Native x86-64 and AArch64 parity and release jobs with target-derived shell identity.
- Installable Readline and History development archives with headers, shared and static libraries, pkg-config files, and component uninstall.
- Readline ABI and behavior checks for layouts, ownership, callbacks, redisplay, streams, completion, inputrc, and History state.
- GNU Readline completion display behavior for repeated Tab presses and the `show-all-if-ambiguous` and `show-all-if-unmodified` inputrc settings.
- Separate CherubSH package and Bash compatibility versions.
- Persistent fuzz targets and seed corpora for parsing, expansion, line input, and the Readline FFI boundary.
- Weekly benchmark reports with raw samples, summaries, and run provenance.
- Pinned dependency review and RustSec checks, automated dependency updates, CycloneDX SBOMs, and signed release attestations.
- Manual pages, Bash-compatible command completion, contribution guidance, and repository templates.

### Changed

- Release archives now include a prefix-aware installer, manuals, and command completion.
- Platform support remains Linux and WSL while parity evidence is collected for other Unix targets.

## 0.3.0 - 2026-05-21

- Moved the default compatibility target to Bash 5.3.
- Added the Bash 5.3 test corpus and kept all 86 standalone upstream drivers passing.
- Kept every runnable Brush compatibility case passing against the Bash 5.3 oracle.

## 0.2.0 - 2026-05-20

- Added the vendored Brush compatibility suite against Bash 5.2.21.
- Kept the upstream Bash 5.2.21 and CherubSH differential suites passing.

## 0.1.0 - 2026-05-20

- Published the first CherubSH release with Bash 5.2.21 parsing, expansion, execution, state, and builtin coverage.
