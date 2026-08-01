use std::fs;
use std::path::PathBuf;
use std::process::Command;

use cherubsh_test_harness::cherub_path;

const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run_cherub(args: &[&str]) -> std::process::Output {
    Command::new(cherub_path().expect("find CherubSH test binary"))
        .args(args)
        .env_clear()
        .env("HOME", "/tmp")
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run CherubSH")
}

#[test]
fn package_version_precedes_bash_compatibility_identity() {
    let output = run_cherub(&["--version"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 version output");
    let lines: Vec<_> = stdout.lines().collect();
    let package_line = format!("cherubsh, version {PACKAGE_VERSION}");
    assert_eq!(lines.first(), Some(&package_line.as_str()));
    assert_eq!(
        lines.get(1),
        Some(&"GNU bash, version 5.3.15(1)-release (x86_64-pc-linux-gnu)")
    );
}

#[test]
fn bash_version_variable_remains_compatibility_facing() {
    let output = run_cherub(&[
        "--noprofile",
        "--norc",
        "-c",
        "printf '%s\\n' \"$BASH_VERSION\"",
    ]);

    assert!(output.status.success());
    assert_eq!(output.stdout, b"5.3.15(1)-release\n");
}

#[test]
fn builtin_help_keeps_the_bash_compatibility_header() {
    let output = run_cherub(&["--noprofile", "--norc", "-c", "help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help output");
    assert_eq!(
        stdout.lines().next(),
        Some("GNU bash, version 5.3.15(1)-release (x86_64-pc-linux-gnu)")
    );
}

#[test]
fn release_tag_accepts_the_workspace_package_version() {
    let release_tag = format!("v{PACKAGE_VERSION}");
    let output = Command::new("bash")
        .arg(workspace_root().join("tools/check-release-tag.sh"))
        .arg(&release_tag)
        .current_dir(workspace_root())
        .output()
        .expect("run release tag guard");

    assert!(
        output.status.success(),
        "tag guard failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        format!("release tag {release_tag} matches Cargo {PACKAGE_VERSION}\n").as_bytes()
    );
}

#[test]
fn release_tag_rejects_a_version_that_differs_from_cargo() {
    let output = Command::new("bash")
        .arg(workspace_root().join("tools/check-release-tag.sh"))
        .arg("v999.0.0")
        .current_dir(workspace_root())
        .output()
        .expect("run release tag guard");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stderr,
        format!("error: release tag v999.0.0 does not match Cargo {PACKAGE_VERSION}\n").as_bytes()
    );
}

#[test]
fn release_workflow_checks_the_tag_before_building() {
    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    let guard = workflow
        .find("./tools/check-release-tag.sh")
        .expect("release tag guard step");
    let verification = workflow
        .find("cargo test --workspace --locked")
        .expect("workspace verification step");

    assert!(
        guard < verification,
        "release tag guard must run before builds"
    );
}
