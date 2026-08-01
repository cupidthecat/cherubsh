use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_common::target::TargetIdentity;
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
    let identity = TargetIdentity::current();
    assert_eq!(lines.first(), Some(&package_line.as_str()));
    assert_eq!(
        lines.get(1),
        Some(
            &format!(
                "GNU bash, version 5.3.15(1)-release ({})",
                identity.machtype
            )
            .as_str()
        )
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
    let identity = TargetIdentity::current();
    assert_eq!(
        stdout.lines().next(),
        Some(
            format!(
                "GNU bash, version 5.3.15(1)-release ({})",
                identity.machtype
            )
            .as_str()
        )
    );
}

#[test]
fn shell_identity_variables_match_the_compiler_target() {
    let output = run_cherub(&[
        "--noprofile",
        "--norc",
        "-c",
        "printf '%s\\n' \"$HOSTTYPE\" \"$OSTYPE\" \"$MACHTYPE\" \"${BASH_VERSINFO[5]}\"",
    ]);

    assert!(output.status.success());
    let identity = TargetIdentity::current();
    let expected = format!(
        "{}\n{}\n{}\n{}\n",
        identity.hosttype, identity.ostype, identity.machtype, identity.machtype
    );
    assert_eq!(output.stdout, expected.as_bytes());
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

#[test]
fn release_materials_are_ready_for_0_4_0() {
    assert_eq!(PACKAGE_VERSION, "0.4.0");

    let changelog =
        fs::read_to_string(workspace_root().join("CHANGELOG.md")).expect("read changelog");
    assert!(changelog.contains("## 0.4.0 - 2026-08-01"));

    let notes_path = workspace_root().join("release-notes/v0.4.0.md");
    let notes = fs::read_to_string(&notes_path).expect("read v0.4.0 release notes");
    for topic in [
        "interactive shell",
        "AArch64",
        "Readline and History development",
        "ABI",
        "supply chain",
        "fuzz",
        "manual pages",
        "module",
    ] {
        assert!(
            notes.contains(topic),
            "{} is missing the {topic} release topic",
            notes_path.display()
        );
    }

    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert!(workflow.contains("--notes-file \"release-notes/${RELEASE_TAG}.md\""));
}

#[test]
fn release_source_guard_rejects_commits_outside_protected_main() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let repository = std::env::temp_dir().join(format!(
        "cherubsh-release-source-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&repository).expect("create release-source repository");

    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(&repository)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--initial-branch", "main"]);
    git(&["config", "user.name", "Release Test"]);
    git(&["config", "user.email", "release-test@example.invalid"]);
    fs::write(repository.join("tracked"), "main\n").expect("write main fixture");
    git(&["add", "tracked"]);
    git(&["commit", "-m", "main"]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

    let guard = workspace_root().join("tools/check-release-source.sh");
    let protected = Command::new("bash")
        .arg(&guard)
        .arg("HEAD")
        .current_dir(&repository)
        .output()
        .expect("run release-source guard on main");
    assert!(
        protected.status.success(),
        "main commit was rejected: {}",
        String::from_utf8_lossy(&protected.stderr)
    );

    git(&["switch", "-c", "release"]);
    fs::write(repository.join("tracked"), "release\n").expect("write release fixture");
    git(&["commit", "-am", "release"]);
    let unmerged = Command::new("bash")
        .arg(&guard)
        .arg("HEAD")
        .current_dir(&repository)
        .output()
        .expect("run release-source guard off main");
    assert_eq!(unmerged.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unmerged.stderr).contains("protected main"));

    fs::remove_dir_all(repository).expect("remove release-source repository");

    let workflow = fs::read_to_string(workspace_root().join(".github/workflows/release.yml"))
        .expect("read release workflow");
    assert!(workflow.contains("verify-release-source:"));
    assert_eq!(
        workflow.matches("needs: verify-release-source").count(),
        2,
        "archive and SBOM builds must both depend on the source guard"
    );
}
