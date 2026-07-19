//! Runs the GNU Readline 8.3 differential gate from Rust tests.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::{workspace_root, HarnessError, RunOutput};

pub const ORACLE_VERSION: &str = "8.3";
pub const ORACLE_PATCH_LEVEL: u8 = 3;

pub fn oracle_root() -> PathBuf {
    std::env::var_os("READLINE_ORACLE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target/oracle/readline-8.3"))
}

pub fn implementation_root() -> PathBuf {
    std::env::var_os("READLINE_OUTPUT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target/readline"))
}

pub fn report_root() -> PathBuf {
    std::env::var_os("READLINE_PARITY_REPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target/parity/readline"))
}

pub fn oracle_available() -> bool {
    let root = oracle_root();
    root.join("include/readline/readline.h").is_file()
        && root.join("include/readline/history.h").is_file()
        && root.join("lib/libreadline.so.8.3").is_file()
        && root.join("lib/libhistory.so.8.3").is_file()
}

pub fn implementation_available() -> bool {
    let root = implementation_root();
    root.join("include/readline/readline.h").is_file()
        && root.join("include/readline/history.h").is_file()
        && root.join("lib/libreadline.so").is_file()
        && root.join("lib/libhistory.so").is_file()
}

pub fn run_parity() -> Result<RunOutput, HarnessError> {
    let root = workspace_root();
    let mut command = Command::new("bash");
    command
        .arg(root.join("tools/run-readline-parity.sh"))
        .current_dir(&root)
        .env("READLINE_ORACLE_ROOT", oracle_root())
        .env("READLINE_OUTPUT_ROOT", implementation_root())
        .env("READLINE_PARITY_REPORT_ROOT", report_root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output().map_err(HarnessError::Io)?;
    let status = output.status.code().ok_or(HarnessError::MissingStatus)?;
    Ok(RunOutput {
        status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn assert_parity() {
    let output = run_parity().expect("run GNU Readline parity gate");
    assert_eq!(
        output.status, 0,
        "GNU Readline parity failed\nstdout:\n{}\nstderr:\n{}",
        output.stdout, output.stderr
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_locations_are_isolated_under_target() {
        let target = workspace_root().join("target");
        assert!(oracle_root().starts_with(&target));
        assert!(implementation_root().starts_with(&target));
        assert!(report_root().starts_with(&target));
    }

    #[test]
    fn pinned_oracle_identifies_the_patched_release() {
        assert_eq!(ORACLE_VERSION, "8.3");
        assert_eq!(ORACLE_PATCH_LEVEL, 3);
    }
}
