# Large Bash corpus design

## Purpose

Issue 59 asks CherubSH to parse substantial Bash programs and turn any incompatibilities into focused regression tests. The corpus will cover these repositories at immutable commits:

- `akinomyoga/ble.sh`
- `rear/rear`
- `vegardit/bash-funk`
- `xwmx/nb`
- `Winetricks/winetricks`
- `testssl/testssl.sh`
- `dylanaraps/neofetch`
- `acmesh-official/acme.sh`
- `89luca89/distrobox`
- `aristocratos/bashtop`

The suite checks syntax acceptance only. It does not claim that CherubSH can run each application or reproduce its behavior in a real operating environment.

## Safety boundary

Fetched repositories are untrusted data. The tooling must never source, execute, install, or build their contents.

Each repository is fetched into a Git object store at its pinned commit. The harness reads blobs directly from the Git tree without checking out files or initializing submodules. It accepts only regular-file modes and ignores symlinks, submodules, and unusual object types. Candidate blobs are sent through standard input to Bash and CherubSH in no-execution syntax mode, with an isolated environment and a per-file timeout.

The harness must stop if it cannot preserve these rules. ReaR may be skipped with a clear report entry if it cannot be processed within the same data-only boundary. The harness must not weaken the boundary to include it.

## Source manifest and fetching

A checked-in manifest will record the canonical repository URL and exact commit for every project. The fetch step will:

1. Create or reuse a cache below `target/`.
2. Fetch only the pinned commit without checking it out.
3. Verify that the fetched commit matches the manifest.
4. Leave source objects and reports in ignored directories.

Revision mismatches, corrupt object stores, and failed fetches are errors. A moving branch or tag is never accepted as the test identity.

## File discovery

The runner will enumerate the pinned tree in a stable order. It will select regular blobs whose names end in `.sh` or `.bash`, plus regular text blobs with a Bash shebang. The shebang check reads only a small prefix of the blob. Repository paths are labels in the report and are never used as local output paths.

Files that do not meet those rules are outside the corpus. Expected exclusions, including symlinks and non-shell files, are counted so that a changed inventory is visible.

## Differential result

For each selected blob, the runner records the project, revision, repository path, Bash result, CherubSH result, and timeout state. Both shells receive the same bytes through standard input.

The acceptance rules are:

- Both shells accept the file: pass.
- Both shells reject the file: pass for acceptance parity, with both statuses recorded.
- One shell accepts and the other rejects: fail.
- Either shell times out: fail.

The report is a deterministic TSV file below `target/`. Failure output will identify the exact project, commit, and path needed to reproduce the mismatch. Any CherubSH mismatch found during the initial sweep must first become a small failing regression test. The production fix follows only after that test fails for the expected reason.

## Tests and automation

The local runner will have deterministic self-tests for manifest parsing, tree enumeration, file selection, symlink rejection, timeouts, and result classification. Repository tests will also assert that the scheduled workflow uses the pinned runner and preserves failure reports.

The real corpus will run in the weekly hardening workflow and through manual workflow dispatch. It will not run in ordinary pull request jobs because those jobs should not depend on ten external fetches. Developers can run the corpus directly when changing parser behavior or refreshing a pinned revision.

## Documentation and later expansion

The testing guide will explain the command, safety model, revision policy, report format, and current corpus result. Documentation will describe the suite as it exists rather than narrating the change that added it.

After the original corpus passes, the implementation work will survey other substantial Bash programs. A short candidate list will record why each project would add useful syntax coverage. Those projects will not enter the required gate until they receive an immutable revision and pass the same safety review.
