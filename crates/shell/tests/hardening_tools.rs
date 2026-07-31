use std::path::PathBuf;
use std::process::Command;

use cherubsh_test_harness::{cherub_path, default_bash_path};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
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
    let output = Command::new("python3")
        .arg(workspace_root().join("tools/pty-stress.py"))
        .args([
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
