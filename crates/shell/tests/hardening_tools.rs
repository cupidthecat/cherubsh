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
