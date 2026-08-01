use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_test_harness::{cherub_path, default_bash_path, oracle_available, oracle_bash_path};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run_pty_matrix() {
    let Some(bash) = pinned_bash() else {
        return;
    };
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-differential.py"))
        .args([
            "--bash",
            bash.to_str().expect("UTF-8 pinned Bash path"),
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
        ])
        .output()
        .expect("run PTY differential scenario");

    assert!(
        output.status.success(),
        "PTY matrix failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn pinned_bash() -> Option<PathBuf> {
    if oracle_available() {
        return Some(oracle_bash_path());
    }
    assert!(
        std::env::var_os("RUN_PTY_PARITY").is_none(),
        "Bash 5.3.15 oracle is required at {}",
        oracle_bash_path().display()
    );
    eprintln!(
        "skip: Bash 5.3.15 oracle is not available at {}",
        oracle_bash_path().display()
    );
    None
}

fn temporary_directory(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cherubsh-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn differential_fuzzer_has_a_deterministic_self_test() {
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/fuzz-differential.py"))
        .arg("--self-test")
        .output()
        .expect("run differential fuzzer self-test");

    assert!(
        output.status.success(),
        "fuzzer self-test failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn differential_fuzzer_accepts_a_relative_cherub_binary_path() {
    let bash = default_bash_path();
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/fuzz-differential.py"))
        .args(["--cherub", "target/debug/cherubsh"])
        .args(["--bash", bash.to_str().expect("UTF-8 default Bash path")])
        .args(["--cases", "3", "--seed", "20260731"])
        .current_dir(workspace_root())
        .output()
        .expect("run differential fuzzer with a relative CherubSH path");

    assert!(
        output.status.success(),
        "differential fuzzer failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pty_stress_probe_recovers_after_an_interrupt() {
    let Some(bash) = pinned_bash() else {
        return;
    };
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-stress.py"))
        .args([
            "--bash",
            bash.to_str().expect("UTF-8 pinned Bash path"),
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
        ])
        .args(["--rounds", "1"])
        .output()
        .expect("run PTY stress probe");

    assert!(
        output.status.success(),
        "PTY stress probe failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pty_stress_probe_accepts_an_explicit_reference_shell() {
    let Some(bash) = pinned_bash() else {
        return;
    };
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-stress.py"))
        .args([
            "--bash",
            bash.to_str().expect("UTF-8 pinned Bash path"),
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
            "--rounds",
            "1",
        ])
        .output()
        .expect("run PTY stress probe with a reference shell");

    assert!(
        output.status.success(),
        "PTY stress probe failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pty_differential_harness_has_a_deterministic_self_test() {
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-differential.py"))
        .arg("--self-test")
        .output()
        .expect("run PTY differential self-test");

    assert!(
        output.status.success(),
        "PTY differential self-test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("PTY differential self-test: "));
    assert!(stdout.ends_with(" scenarios passed\n"));
}

#[test]
fn pty_differential_writes_raw_and_normalized_reports() {
    let Some(bash) = pinned_bash() else {
        return;
    };
    let report_directory = temporary_directory("pty-reports");
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-differential.py"))
        .args([
            "--bash",
            bash.to_str().expect("UTF-8 pinned Bash path"),
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
            "--scenario",
            "interrupt-recovery",
            "--report-dir",
            report_directory.to_str().expect("UTF-8 report path"),
        ])
        .output()
        .expect("run PTY differential report fixture");

    assert!(output.status.success());
    let scenario = report_directory.join("interrupt-recovery");
    for name in ["bash.raw", "bash.txt", "cherub.raw", "cherub.txt"] {
        assert!(scenario.join(name).is_file(), "missing PTY report {name}");
    }
    assert!(report_directory.join("report.json").is_file());
    let report = fs::read_to_string(report_directory.join("report.json"))
        .expect("read PTY differential report");
    assert!(report.contains("\"observations\""));
    assert!(report.contains("\"recovery\": ["));
    let _ = fs::remove_dir_all(report_directory);
}

#[test]
fn pty_differential_rejects_an_unpinned_reference_shell() {
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-differential.py"))
        .args([
            "--bash",
            "/bin/false",
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
            "--scenario",
            "eof",
        ])
        .output()
        .expect("run PTY differential with an unpinned reference shell");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("must report GNU Bash 5.3.15"),
        "unexpected error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pty_differential_requires_the_exact_bash_patch_version() {
    let directory = temporary_directory("pty-near-version");
    let fake_bash = directory.join("bash");
    fs::write(
        &fake_bash,
        "#!/bin/sh\nprintf '%s\\n' 'GNU bash, version 5.3.150(1)-release (test)'\n",
    )
    .expect("write fake Bash executable");
    let mut permissions = fs::metadata(&fake_bash)
        .expect("read fake Bash metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_bash, permissions).expect("make fake Bash executable");

    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-differential.py"))
        .args([
            "--bash",
            fake_bash.to_str().expect("UTF-8 fake Bash path"),
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
            "--scenario",
            "eof",
        ])
        .output()
        .expect("run PTY differential with a near-match Bash version");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("5.3.150"));
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn pty_differential_preserves_timeout_transcripts() {
    let Some(bash) = pinned_bash() else {
        return;
    };
    let report_directory = temporary_directory("pty-timeout");
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-differential.py"))
        .args([
            "--bash",
            bash.to_str().expect("UTF-8 pinned Bash path"),
            "--cherub",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
            "--scenario",
            "interrupt-recovery",
            "--timeout",
            "0.000001",
            "--report-dir",
            report_directory.to_str().expect("UTF-8 report path"),
        ])
        .output()
        .expect("run PTY timeout fixture");

    assert!(!output.status.success());
    let report =
        fs::read_to_string(report_directory.join("report.json")).expect("read PTY timeout report");
    assert!(report.contains("\"status\": \"FAIL\""));
    assert!(report.contains("\"timed_out\": true"));
    let scenario = report_directory.join("interrupt-recovery");
    assert!(scenario.join("bash.raw").is_file());
    assert!(scenario.join("cherub.raw").is_file());
    let _ = fs::remove_dir_all(report_directory);
}

#[test]
fn pty_differential_matches_the_registered_scenarios() {
    run_pty_matrix();
}

#[test]
fn upstream_fetch_tries_the_canonical_gnu_origin_before_the_redirector() {
    let script = fs::read_to_string(workspace_root().join("tools/fetch-upstream.sh"))
        .expect("read upstream fetch script");
    let canonical = script
        .find("\"https://ftp.gnu.org/gnu\"")
        .expect("canonical GNU origin");
    let redirector = script
        .find("\"https://ftpmirror.gnu.org\"")
        .expect("GNU mirror redirector");

    assert!(
        canonical < redirector,
        "the canonical GNU origin must be tried before the redirector"
    );
}

#[test]
fn wiki_validation_runs_for_every_pull_request() {
    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/wiki.yml"))
        .expect("read wiki workflow");
    let publish = workflow.find("\n  publish:\n").expect("wiki publish job");
    let concurrency = workflow
        .find("\n    concurrency:\n")
        .expect("wiki publish concurrency");

    assert!(
        workflow.contains("  pull_request:\n  workflow_dispatch:\n"),
        "the required wiki validation check must run for every pull request"
    );
    assert!(
        !workflow.contains("\nconcurrency:\n") && concurrency > publish,
        "pull request validation must not share publish concurrency"
    );
}

#[test]
fn persistent_fuzz_targets_replay_seed_corpora_and_retain_failures() {
    let root = workspace_root();
    let manifest = fs::read_to_string(root.join("fuzz/Cargo.toml")).expect("read fuzz manifest");
    let workflow =
        fs::read_to_string(root.join(".github/workflows/fuzz.yml")).expect("read fuzz workflow");

    for target in ["lexer", "parser", "expansion", "line_input", "readline_ffi"] {
        assert!(
            manifest.contains(&format!("name = \"{target}\"")),
            "missing persistent fuzz target {target}"
        );
        let corpus = root.join("fuzz/corpus").join(target);
        assert!(
            fs::read_dir(&corpus)
                .unwrap_or_else(|_| panic!("missing seed corpus for {target}"))
                .next()
                .is_some(),
            "empty seed corpus for {target}"
        );
    }

    assert!(workflow.contains("pull_request:"));
    assert!(workflow.contains("schedule:"));
    assert!(workflow.contains("./tools/run-fuzz-corpus.sh"));
    assert!(workflow.contains("cargo fuzz run"));
    assert!(workflow.contains("actions/upload-artifact@v6"));
    assert!(workflow.contains("if: failure()"));

    let replay = fs::read_to_string(root.join("tools/run-fuzz-corpus.sh"))
        .expect("read fuzz corpus replay script");
    assert!(replay.contains("-timeout=10"));
    assert!(replay.contains("-rss_limit_mb=2048"));

    let guide = fs::read_to_string(root.join("fuzz/README.md")).expect("read fuzz guide");
    for command in ["cargo fuzz tmin", "cargo fuzz fmt", "cargo fuzz run"] {
        assert!(guide.contains(command), "fuzz guide omits {command}");
    }
}

#[test]
fn scheduled_benchmarks_publish_reproducible_history_without_thresholds() {
    let root = workspace_root();
    let bench = fs::read_to_string(root.join("tools/bench.sh")).expect("read benchmark driver");
    let workflow = fs::read_to_string(root.join(".github/workflows/benchmarks.yml"))
        .expect("read benchmark workflow");

    for case in ["bash_perf_script", "bash_perftest"] {
        assert!(bench.contains(case), "benchmark driver omits {case}");
    }
    assert!(bench.contains("BENCH_OUTPUT_DIR"));
    assert!(bench.contains("metadata.tsv"));
    for field in [
        "commit",
        "runner",
        "cpu",
        "rust_toolchain",
        "oracle_version",
        "cargo_lock_sha256",
        "fuzz_lock_sha256",
    ] {
        assert!(bench.contains(field), "benchmark metadata omits {field}");
    }

    assert!(workflow.contains("schedule:"));
    assert!(workflow.contains("workflow_dispatch:"));
    assert!(workflow.contains("./tools/bench.sh"));
    assert!(workflow.contains("retention-days: 90"));
    for artifact in ["raw.tsv", "summary.tsv", "metadata.tsv"] {
        assert!(
            workflow.contains(artifact),
            "benchmark workflow omits {artifact}"
        );
    }
    assert!(
        !workflow.contains("threshold") && !workflow.contains("regression"),
        "the collection-only workflow must not impose a performance threshold"
    );
}

#[test]
fn upstream_misc_benchmark_cases_complete_and_write_reports() {
    let Some(bash) = pinned_bash() else {
        return;
    };
    let report_directory = temporary_directory("benchmark-reports");
    let output = Command::new(workspace_root().join("tools/bench.sh"))
        .env("BENCH_BUILD", "0")
        .env("BENCH_CASES", "bash_perf_script,bash_perftest")
        .env("BENCH_OUTPUT_DIR", &report_directory)
        .env("BASH_ORACLE_PATH", &bash)
        .env("CHERUBSH", cherub_path().expect("CherubSH test binary"))
        .env("RUNS", "1")
        .env("WARMUPS", "0")
        .output()
        .expect("run focused benchmark cases");

    assert!(
        output.status.success(),
        "benchmark failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = fs::read_to_string(report_directory.join("raw.tsv")).expect("read raw samples");
    let summary =
        fs::read_to_string(report_directory.join("summary.tsv")).expect("read benchmark summary");
    let metadata =
        fs::read_to_string(report_directory.join("metadata.tsv")).expect("read benchmark metadata");
    for case in ["bash_perf_script", "bash_perftest"] {
        assert!(raw.contains(case), "raw samples omit {case}");
        assert!(summary.contains(case), "summary omits {case}");
    }
    assert!(metadata.contains("cargo_lock_sha256\t"));
    assert!(metadata.contains("fuzz_lock_sha256\t"));
    let _ = fs::remove_dir_all(report_directory);
}

#[test]
fn user_and_contributor_release_materials_cover_public_entry_points() {
    let root = workspace_root();
    for path in ["CONTRIBUTING.md", "CHANGELOG.md"] {
        let contents = fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("read {path}: {error}"));
        assert!(!contents.trim().is_empty(), "{path} is empty");
    }
    for path in [
        ".github/ISSUE_TEMPLATE/bug.yml",
        ".github/ISSUE_TEMPLATE/compatibility.yml",
        ".github/ISSUE_TEMPLATE/feature.yml",
        ".github/pull_request_template.md",
    ] {
        assert!(root.join(path).is_file(), "missing public template {path}");
    }

    let readme = fs::read_to_string(root.join("README.md")).expect("read README");
    let getting_started =
        fs::read_to_string(root.join("wiki/Getting-started.md")).expect("read getting started");
    for text in [&readme, &getting_started] {
        assert!(text.contains("Linux"));
        assert!(text.contains("WSL"));
        assert!(text.contains("install-cherubsh.sh"));
        assert!(text.contains("man cherubsh"));
        assert!(
            !text.contains("cherubsh-0.3.0-x86_64-unknown-linux-gnu.tar.gz"),
            "installation instructions name an unpublished v0.3.0 archive"
        );
    }
}

fn assert_workflow_actions_are_commit_pinned(name: &str, workflow: &str) {
    for line in workflow.lines() {
        let line = line.trim();
        let Some(reference) = line
            .strip_prefix("uses: ")
            .or_else(|| line.strip_prefix("- uses: "))
        else {
            continue;
        };
        let revision = reference
            .rsplit_once('@')
            .unwrap_or_else(|| panic!("{name} action has no revision: {reference}"))
            .1
            .split_whitespace()
            .next()
            .expect("action revision");
        assert!(
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{name} action is not pinned to a commit: {reference}"
        );
    }
}

#[test]
fn release_supply_chain_controls_are_pinned_and_verifiable() {
    let root = workspace_root();
    let security = fs::read_to_string(root.join("SECURITY.md")).expect("read security policy");
    for text in [
        "Supported Versions",
        "Private vulnerability reporting",
        "security/advisories/new",
    ] {
        assert!(security.contains(text), "security policy omits {text}");
    }

    let dependabot =
        fs::read_to_string(root.join(".github/dependabot.yml")).expect("read Dependabot config");
    for ecosystem in ["cargo", "github-actions"] {
        assert!(
            dependabot.contains(&format!("package-ecosystem: \"{ecosystem}\"")),
            "Dependabot omits {ecosystem}"
        );
    }
    assert!(dependabot.contains("directory: \"/fuzz\""));
    assert_eq!(
        dependabot.matches("package-ecosystem: \"cargo\"").count(),
        2,
        "Dependabot must cover the root and fuzz Cargo workspaces"
    );

    let security_workflow = fs::read_to_string(root.join(".github/workflows/security.yml"))
        .expect("read dependency security workflow");
    assert!(security_workflow.contains("pull_request:"));
    assert!(security_workflow.contains("schedule:"));
    assert!(security_workflow.contains("actions/dependency-review-action@"));
    assert!(security_workflow.contains("rustsec/audit-check@"));
    assert!(security_workflow.contains("working-directory: fuzz"));
    assert_eq!(
        security_workflow
            .matches("cargo install cargo-audit --version 0.22.2 --locked")
            .count(),
        2,
        "each RustSec job must preinstall the lockfile-compatible cargo-audit release"
    );
    assert_workflow_actions_are_commit_pinned("security workflow", &security_workflow);

    let release = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert_workflow_actions_are_commit_pinned("release workflow", &release);
    for text in [
        "cargo-cyclonedx --version 0.5.9 --locked",
        "--format json",
        ".cdx.json",
        "attestations: write",
        "id-token: write",
        "actions/attest@",
        "subject-path:",
        "SHA256SUMS",
        "dist/cherubsh*.cdx.json",
        "sbom-path: dist/cherubsh.cdx.json",
        "./tools/prepare-sboms.py",
    ] {
        assert!(release.contains(text), "release workflow omits {text}");
    }

    let readme = fs::read_to_string(root.join("README.md")).expect("read README");
    assert!(readme.contains("gh attestation verify"));
    assert!(readme.contains("--predicate-type https://cyclonedx.org/bom"));

    assert!(security.contains("Starting with the first release after v0.3.0"));
    let prepared = Command::new("python3")
        .arg(root.join("tools/prepare-sboms.py"))
        .arg("--self-test")
        .output()
        .expect("run SBOM preparation self-test");
    assert!(
        prepared.status.success(),
        "SBOM preparation self-test failed:\n{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&prepared.stdout),
        "SBOM preparation self-test passed\n"
    );
}
