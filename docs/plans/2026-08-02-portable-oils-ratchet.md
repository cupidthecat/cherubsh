# Portable Oils Ratchet Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Keep Oils parity strict across Linux hosts while accepting a small, explicit set of outcomes for documented nondeterministic cases.

**Architecture:** Parse the ratchet `fields` column as one or more exact field sets separated by `|`. Classification matches an observation against those structured variants before checking exact or variable fingerprints. Generic rows cover host-sensitive values, while architecture rows remain available for genuine x86_64 and aarch64 differences.

**Tech Stack:** Rust 2024, Cargo integration tests, TSV fixtures, GitHub Actions

---

### Task 1: Parse and classify explicit field variants

**Files:**

- Modify: `crates/test-harness/src/oils.rs`
- Test: `crates/test-harness/tests/oils_ratchet.rs`

**Step 1: Write the failing tests**

Add a fixture row whose `fields` value is `stderr|stdout,stderr`. Load it for x86_64 and assert that both structured variants are present. Add classification assertions showing that either listed set is `KNOWN` and an unlisted `status` difference is `DRIFT`. Add loader assertions for empty, duplicate, and unknown field variants.

**Step 2: Run the tests and confirm the failure**

Run:

```sh
cargo test -p cherubsh-test-harness --test oils_ratchet
```

Expected: the field-variant assertions fail because the loader still treats `|` as part of a field name.

**Step 3: Implement structured variants**

Change `OilsKnownMismatch` from one field list to a list of field lists:

```rust
pub struct OilsKnownMismatch {
    pub id: String,
    pub field_variants: Vec<Vec<String>>,
    pub oracle_fingerprint: String,
    pub candidate_fingerprint: String,
}
```

Split the TSV value on `|`, split each variant on `,`, and validate every field against the existing allowlist. Reject empty and duplicate variants. In `classify_oils_outcome`, require this exact membership check before comparing fingerprints:

```rust
known.field_variants.iter().any(|variant| variant == &fields)
```

**Step 4: Run the focused tests**

Run:

```sh
cargo test -p cherubsh-test-harness --test oils_ratchet
```

Expected: all ratchet tests pass.

### Task 2: Make environment-sensitive rows portable

**Files:**

- Modify: `crates/test-harness/oils-known-mismatches.tsv`
- Modify: `crates/test-harness/oils-nondeterministic-cases.txt`
- Test: `crates/test-harness/tests/oils_parser.rs`
- Test: `crates/test-harness/tests/oils_ratchet.rs`

**Step 1: Write the failing fixture assertions**

Assert that the checked-in ratchet has these generic entries:

```text
process-sub.test.sh::002::Process sub from shell to stdin
prompt.test.sh::027::\j for number of jobs
shell-bugs.test.sh::000::./configure idiom
vars-bash.test.sh::000::$SHELL is set to what is in /etc/passwd
vars-special.test.sh::007::HOSTNAME OSTYPE can be changed
```

Assert that the prompt entry accepts `stderr` and `stdout,stderr`, and that only the Bash help entry still needs an aarch64 override among the runner-sensitive cases.

**Step 2: Run the tests and confirm the failure**

Run:

```sh
cargo test -p cherubsh-test-harness --test oils_parser --test oils_ratchet
```

Expected: the generic-entry and prompt-variant assertions fail against the current architecture-specific data.

**Step 3: Update the manifest and ratchet**

Keep all variable fingerprints restricted to `oils-nondeterministic-cases.txt`. Use the following field and fingerprint policy:

- Process substitution: `stdout`, exact Bash, variable CherubSH.
- Prompt jobs: `stderr|stdout,stderr`, variable Bash and CherubSH.
- Configure timing: `stderr`, variable Bash and CherubSH.
- Login shell: `stderr`, variable Bash and CherubSH.
- Hostname and OSTYPE: `stdout`, exact Bash and variable CherubSH.

Use `*` for these host-sensitive rows. Retain the aarch64 Bash help override.

**Step 4: Run the focused suites**

Run:

```sh
cargo test -p cherubsh-test-harness --locked
```

Expected: all test-harness unit, parser, ratchet, and runner tests pass.

### Task 3: Update contributor documentation

**Files:**

- Modify: `README.md`
- Modify: `wiki/Testing.md`

**Step 1: Humanize the documentation edits**

Use `humanizer:humanizer`. Explain that named cases may list exact alternate mismatch-field sets and that the gate does not accept field combinations absent from the ratchet. Preserve the architecture-scoped observation guidance.

**Step 2: Validate the documentation**

Run:

```sh
./tools/check-wiki-source.sh
git diff --check
```

Expected: wiki source validation passes and the diff has no whitespace errors.

### Task 4: Verify, review, commit, and push

**Files:**

- Review every file changed by Tasks 1 through 3.

**Step 1: Run local verification**

Run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
./tools/run-parity.sh
```

Expected: the workspace, PTY, Oils, upstream Bash, Brush, and Readline gates pass.

**Step 2: Request code review**

Use `superpowers:requesting-code-review`. Resolve every Critical or Important finding and rerun the affected checks.

**Step 3: Commit with the requested identity**

Stage only the portable-ratchet implementation and documentation. Commit with author and committer `Frank <101807917+frankischilling@users.noreply.github.com>` using a concise Conventional Commit message.

**Step 4: Push and monitor CI**

Push `feat/oils-parity-ratchet`. Watch the new parity workflow until `parity-x86_64`, `parity-aarch64`, and `linux` all pass. If a job fails, inspect its raw report artifact before changing the ratchet.
