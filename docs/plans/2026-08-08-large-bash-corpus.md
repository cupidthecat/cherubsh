# Large Bash corpus implementation plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a reproducible, data-only differential parser suite for ten substantial Bash projects, fix every CherubSH acceptance mismatch it exposes, and document the results.

**Architecture:** A checked-in TSV manifest pins one commit per project. A Python runner fetches those commits into Git object stores, reads regular blobs without a checkout, and sends selected shell files through standard input to Bash 5.3.15 and CherubSH in no-execution mode. The weekly hardening workflow runs the suite and preserves its deterministic report.

**Tech Stack:** Python 3 standard library, Git object plumbing, Bash 5.3.15, CherubSH, Rust integration tests, GitHub Actions, Markdown

---

### Task 1: Pin the real-world source manifest

**Files:**

- Create: `large-scripts.lock`
- Modify: `crates/shell/tests/hardening_tools.rs`

**Step 1: Write the failing manifest test**

Add a test that reads non-comment rows from `large-scripts.lock`, requires four tab-separated fields, checks every commit against `^[0-9a-f]{40}$`, accepts only `required` or `optional-safety`, and asserts this exact project order:

```rust
let expected = [
    "ble-sh", "rear", "bash-funk", "nb", "winetricks",
    "testssl-sh", "neofetch", "acme-sh", "distrobox", "bashtop",
];
assert_eq!(projects, expected);
```

Also assert that only `rear` uses `optional-safety`.

**Step 2: Run the focused test and verify RED**

Run:

```sh
BASH_ORACLE_PATH=/home/frank/cherubsh/target/oracle/bash-5.3.15/bash \
  cargo test -p cherubsh --test hardening_tools \
  large_script_manifest_pins_the_approved_corpus -- --exact --nocapture
```

Expected: FAIL because `large-scripts.lock` does not exist.

**Step 3: Create the manifest**

Use this schema and these immutable revisions:

```text
# name<TAB>repository<TAB>commit<TAB>policy
ble-sh	https://github.com/akinomyoga/ble.sh.git	d69e4d549a1881a37300fe6b4a05478bd9157dfc	required
rear	https://github.com/rear/rear.git	a2679d258d279465427ea38cbb9e20c64211bb43	optional-safety
bash-funk	https://github.com/vegardit/bash-funk.git	8f889d0702a05e3c90e6705314474ada34125e28	required
nb	https://github.com/xwmx/nb.git	8b7fe6fdd00bd2379e0442221b81411d3f536abd	required
winetricks	https://github.com/Winetricks/winetricks.git	5a59ea07513b24093bd90fad943ecf9543cf05bc	required
testssl-sh	https://github.com/testssl/testssl.sh.git	5296954a701dd00240bec32feadffaa7eacb2bba	required
neofetch	https://github.com/dylanaraps/neofetch.git	ccd5d9f52609bbdcd5d8fa78c4fdb0f12954125f	required
acme-sh	https://github.com/acmesh-official/acme.sh.git	2feb392bd0e3964d9bf68871ae804578d9d5ca80	required
distrobox	https://github.com/89luca89/distrobox.git	6aee0c552be48381715d3c5fcc5565c7cbc08c1c	required
bashtop	https://github.com/aristocratos/bashtop.git	60f95a1a74c8e7e589c02aa03d60141152df8337	required
```

**Step 4: Run the focused test and verify GREEN**

Run the command from Step 2. Expected: PASS.

**Step 5: Commit**

Stage both files and commit with:

```text
test(hardening): pin large Bash sources
```

### Task 2: Build the data-only corpus reader

**Files:**

- Create: `tools/large-script-parity.py`
- Modify: `crates/shell/tests/hardening_tools.rs`

**Step 1: Write the failing self-test integration test**

Add a Rust test that runs:

```rust
let output = Command::new("python3")
    .arg(workspace_root().join("tools/large-script-parity.py"))
    .arg("--self-test")
    .output()
    .expect("run large-script parity self-test");
assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
assert_eq!(
    String::from_utf8_lossy(&output.stdout),
    "large-script parity self-test: 7 checks passed\n"
);
```

**Step 2: Run the focused test and verify RED**

Run:

```sh
cargo test -p cherubsh --test hardening_tools \
  large_script_parity_has_a_deterministic_self_test -- --exact --nocapture
```

Expected: FAIL because the runner does not exist.

**Step 3: Implement manifest and Git tree handling**

Create dataclasses for `Project`, `TreeEntry`, `ShellResult`, and `ReportRow`. Implement these functions with explicit argument lists and no shell command strings:

```python
def load_manifest(path: Path) -> list[Project]: ...
def ensure_object_store(project: Project, cache: Path) -> Path: ...
def list_tree(repo: Path, commit: str) -> list[TreeEntry]: ...
def read_blob(repo: Path, object_id: str) -> bytes: ...
def is_shell_blob(path: bytes, data: bytes) -> bool: ...
```

`ensure_object_store` must initialize a bare repository, validate an HTTPS GitHub URL, fetch the exact commit with depth one, and verify `FETCH_HEAD^{commit}` against the manifest. It must never call `checkout`, `switch`, `submodule`, or a repository file.

Parse `git ls-tree -r -z --full-tree` as bytes. Only modes `100644` and `100755` with type `blob` can reach `read_blob`. Count symlinks, submodules, and unsupported modes without opening them.

Select case-sensitive `.sh` and `.bash` names. For extensionless files, accept only a first line whose interpreter basename is `bash`, including `/usr/bin/env bash` and `/usr/bin/env -S bash` forms.

**Step 4: Implement seven deterministic self-checks**

Use a temporary bare Git object store populated through `git hash-object`, `git mktree`, and `git commit-tree`. The checks must cover:

1. Valid manifest parsing.
2. Invalid commit rejection.
3. Stable tree ordering.
4. `.sh` and `.bash` selection.
5. Bash shebang selection.
6. Symlink and submodule rejection.
7. Paths remaining report labels rather than filesystem paths.

The self-test creates its own bytes and Git objects. It does not access the network.

**Step 5: Run the focused test and verify GREEN**

Run the command from Step 2. Expected: PASS with exactly seven checks.

**Step 6: Commit**

Stage the runner and test, then commit with:

```text
feat(hardening): read pinned Bash corpora safely
```

### Task 3: Add no-execution differential parsing

**Files:**

- Modify: `tools/large-script-parity.py`
- Modify: `crates/shell/tests/hardening_tools.rs`

**Step 1: Extend the self-test contract before production code**

Change the Rust expectation to `large-script parity self-test: 12 checks passed`. Add planned checks for matching acceptance, matching rejection, mismatched acceptance, timeout classification, and a no-execution canary.

**Step 2: Run the focused test and verify RED**

Run the Task 2 focused command. Expected: FAIL because the self-test still reports seven checks.

**Step 3: Implement the shell runner**

Add:

```python
def run_shell(binary: Path, kind: str, source: bytes, timeout: float, cwd: Path) -> ShellResult:
    if kind == "bash":
        argv = [str(binary), "--noprofile", "--norc", "-n", "-s"]
    else:
        argv = [str(binary), "--norc", "-n", "-s"]
    env = {
        "HOME": str(cwd),
        "PATH": "/usr/bin:/bin",
        "LC_ALL": "C",
        "BASH_ENV": "/dev/null",
        "ENV": "/dev/null",
    }
    ...
```

Use `subprocess.run` with `input=source`, `stdin` supplied only through that input, `cwd=cwd`, the exact environment above, captured output, and a timeout. Do not use `shell=True`. Classify results as `accept`, `reject`, or `timeout`.

Add `compare_results` so matching acceptance and matching rejection pass, any acceptance disagreement fails, and any timeout fails. The no-execution canary must parse source containing `touch`, command substitution, redirection, and `source`, then assert that no canary path was created.

**Step 4: Implement reporting and the main CLI**

Support these arguments:

```text
--manifest PATH
--cache-dir PATH
--report-dir PATH
--bash PATH
--cherub PATH
--timeout SECONDS
--project NAME
--self-test
```

Default to `large-scripts.lock`, `target/upstream/large-scripts`, `target/hardening/large-scripts`, the pinned oracle location, `target/debug/cherubsh`, and five seconds per shell. Write a sorted `report.tsv` with a header and columns for verdict, project, revision, path, Bash result, CherubSH result, and timeout flags. Print project and final tallies. Return nonzero on mismatch, timeout, fetch failure, or a required safety failure.

For `rear`, convert only a safety-boundary failure into a `SKIP` project row with the reason. Network errors, revision errors, parser mismatches, and timeouts remain failures.

**Step 5: Run the focused test and verify GREEN**

Run the Task 2 focused command. Expected: PASS with exactly twelve checks.

**Step 6: Run all hardening tool tests**

Run:

```sh
BASH_ORACLE_PATH=/home/frank/cherubsh/target/oracle/bash-5.3.15/bash \
  cargo test -p cherubsh --test hardening_tools -- --nocapture
```

Expected: PASS.

**Step 7: Commit**

Stage the runner and tests, then commit with:

```text
feat(hardening): compare large scripts without execution
```

### Task 4: Add the scheduled hardening gate

**Files:**

- Modify: `.github/workflows/hardening.yml`
- Modify: `crates/shell/tests/architecture_workflows.rs`

**Step 1: Write the failing workflow test**

Add a test that extracts a `large-bash-corpus` job and asserts it:

- Runs on `ubuntu-24.04` with a bounded timeout.
- Installs the packages needed for the Bash oracle.
- Runs `tools/fetch-upstream.sh` and `oracle/build-bash-5.3.15.sh` before the corpus command.
- Builds `cherubsh` with `cargo build --locked -p cherubsh`.
- Runs `python3 tools/large-script-parity.py` with explicit Bash and CherubSH paths.
- Uploads `target/hardening/large-scripts` on failure.
- Does not use `continue-on-error`.

**Step 2: Run the focused test and verify RED**

Run:

```sh
cargo test -p cherubsh --test architecture_workflows \
  hardening_workflow_runs_the_large_bash_corpus -- --exact --nocapture
```

Expected: FAIL because the job is absent.

**Step 3: Add the workflow job**

Add an independent `large-bash-corpus` job to the existing weekly and manually dispatched hardening workflow. Give it read-only repository permissions and a 30-minute timeout. Keep fetched objects and reports below `target/`.

**Step 4: Run the focused and complete workflow tests**

Run the command from Step 2, then:

```sh
cargo test -p cherubsh --test architecture_workflows -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

Stage the workflow and test, then commit with:

```text
ci(hardening): schedule large Bash parsing
```

### Task 5: Run the ten-project corpus and fix every mismatch

**Files:**

- Modify as evidence requires: `crates/lexer/src/*.rs`, `crates/parser/src/*.rs`, or `crates/shell/src/reader_loop/*.rs`
- Test: `crates/shell/tests/parser_parity.rs`

**Step 1: Build both shells and run the corpus**

Run:

```sh
cargo build --locked -p cherubsh
python3 tools/large-script-parity.py \
  --bash /home/frank/cherubsh/target/oracle/bash-5.3.15/bash \
  --cherub target/debug/cherubsh
```

Expected: a complete `target/hardening/large-scripts/report.tsv`. If every row passes, proceed to Task 6.

**Step 2: Investigate one mismatch at a time**

For each mismatch, preserve the exact project, commit, path, statuses, and diagnostics. Reduce the source until the acceptance disagreement still reproduces. Trace the parser or reader path that produces the difference and compare it with a nearby working Bash construct before proposing a fix.

**Step 3: Add a failing regression test**

Add the smallest source to `parser_parity.rs` or its existing fixture structure. Run only that test and confirm it fails because CherubSH disagrees with Bash, not because of test setup.

**Step 4: Implement the smallest parser fix**

Change only the component shown by the root-cause trace. Run the focused test until it passes, then run the parser crate and parser parity tests.

**Step 5: Repeat RED and GREEN for every distinct root cause**

Do not group unrelated syntax failures into one speculative patch. Commit each coherent parser fix with its regression test.

**Step 6: Re-run the complete corpus**

Run the command from Step 1. Expected: no `FAIL` or `TIMEOUT` rows. ReaR may have a documented safety `SKIP`, but it may not be skipped for a parser mismatch or operational convenience.

### Task 6: Document the suite and research later candidates

**Files:**

- Modify: `wiki/Testing.md`
- Modify: `crates/shell/tests/hardening_tools.rs`

**Step 1: Write the failing documentation contract test**

Assert that `wiki/Testing.md` names `tools/large-script-parity.py`, explains that fetched blobs are never checked out or executed, lists the report path, describes the weekly job, and includes a future-candidate section.

**Step 2: Run the focused test and verify RED**

Run the new test alone. Expected: FAIL because the guide has no real-world corpus section.

**Step 3: Research future candidates**

After the original ten-project report is green, search official repositories for substantial Bash programs that add syntax patterns not already represented. Record a short list with one concrete coverage reason per project. Do not add their revisions to `large-scripts.lock` in this change.

**Step 4: Update and humanize the testing guide**

Document the local command, immutable revisions, data-only safety model, file selection, acceptance rules, report format, scheduled job, current result, and future candidates. Run the humanizer draft, audit, and final pass internally. Preserve all commands and paths, and leave no em or en dashes in the final file.

**Step 5: Verify the documentation**

Run:

```sh
./tools/check-wiki-source.sh
cargo test -p cherubsh --test hardening_tools -- --nocapture
```

Expected: PASS.

**Step 6: Commit**

Stage the guide and test, then commit with:

```text
docs(testing): explain large Bash corpus
```

### Task 7: Run the complete verification gate

**Files:** None unless verification exposes a defect.

**Step 1: Check formatting and lint**

Run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 tools/large-script-parity.py --self-test
./tools/check-wiki-source.sh
```

Expected: every command passes without warnings.

**Step 2: Run workspace tests**

Run:

```sh
BASH_ORACLE_PATH=/home/frank/cherubsh/target/oracle/bash-5.3.15/bash \
  ./tools/run-workspace-tests.sh
```

Expected: PASS.

**Step 3: Run the real corpus from a warm cache**

Run the Task 5 corpus command again. Expected: all required projects pass, no parser mismatches, and no timeouts.

**Step 4: Review the final diff and repository state**

Run `git diff main...HEAD --check`, inspect every changed file, and confirm that `target/` content is untracked and ignored. Verify every commit author and committer resolves to the configured human identity.
