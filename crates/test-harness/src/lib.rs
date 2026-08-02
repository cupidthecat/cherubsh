use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub mod brush;
pub mod readline;
pub mod upstream;

#[derive(Debug)]
pub enum HarnessError {
    Io(std::io::Error),
    MissingStatus,
    MissingShellPath,
    MissingCherubBinary,
    OracleUnavailable(PathBuf),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::MissingStatus => write!(f, "process exited without a status code"),
            Self::MissingShellPath => write!(f, "shell path was empty"),
            Self::MissingCherubBinary => write!(
                f,
                "CARGO_BIN_EXE_cherubsh not set (run tests with cargo, not rustc)"
            ),
            Self::OracleUnavailable(path) => write!(
                f,
                "bash {} oracle not available at {}; run ./tools/run-workspace-tests.sh to provision it",
                oracle_version(),
                path.display()
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Default, Clone)]
pub struct RunSpec<'a> {
    pub args: Vec<&'a str>,
    pub stdin: Option<&'a str>,
    pub env: Vec<(&'a str, &'a str)>,
    pub env_remove: Vec<&'a str>,
    pub script: Option<&'a str>,
}

pub struct BashOracle {
    pub bash_path: PathBuf,
}

impl BashOracle {
    pub fn new(bash_path: PathBuf) -> Self {
        Self { bash_path }
    }

    pub fn run(&self, script: &str) -> Result<RunOutput, HarnessError> {
        run_shell_script(&self.bash_path, script)
    }

    pub fn run_spec(&self, spec: &RunSpec<'_>) -> Result<RunOutput, HarnessError> {
        run_shell_spec(&self.bash_path, spec)
    }
}

pub fn run_shell_script(shell_path: &Path, script: &str) -> Result<RunOutput, HarnessError> {
    let spec = RunSpec {
        script: Some(script),
        ..RunSpec::default()
    };
    run_shell_spec(shell_path, &spec)
}

pub fn run_shell_spec(shell_path: &Path, spec: &RunSpec<'_>) -> Result<RunOutput, HarnessError> {
    if shell_path.as_os_str().is_empty() {
        return Err(HarnessError::MissingShellPath);
    }
    let mut command = Command::new(shell_path);
    // Disable rc/profile sourcing so both shells start from a clean state
    // regardless of the user's local config.
    command.arg("--norc").arg("--noprofile");
    if let Some(script) = spec.script {
        command.arg("-c").arg(script);
    }
    for arg in &spec.args {
        command.arg(arg);
    }
    command.env_clear();
    // Preserve a minimal default env so the shell can find PATH / HOME during fixture tests.
    let mut base_env: BTreeMap<OsString, OsString> = BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if matches!(
            k.as_ref(),
            "PATH" | "HOME" | "USER" | "LOGNAME" | "TERM" | "LANG" | "LC_ALL" | "TZ"
        ) {
            base_env.insert(key, value);
        }
    }
    for (key, value) in &base_env {
        command.env(key, value);
    }
    for key in &spec.env_remove {
        command.env_remove(key);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    if spec.stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn().map_err(HarnessError::Io)?;
    if let Some(input) = spec.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .map_err(HarnessError::Io)?;
        }
    }
    let output = child.wait_with_output().map_err(HarnessError::Io)?;
    let status = match output.status.code() {
        Some(code) => code,
        None => {
            use std::os::unix::process::ExitStatusExt;
            match output.status.signal() {
                Some(sig) => 128 + sig,
                None => return Err(HarnessError::MissingStatus),
            }
        }
    };
    Ok(RunOutput {
        status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub fn default_bash_path() -> PathBuf {
    std::env::var("BASH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/bin/bash"))
}

/// Returns the workspace root that contains `crates/test-harness`.
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("test-harness must be inside the workspace's crates directory")
        .to_path_buf()
}

/// Returns the selected Bash oracle version.
pub fn oracle_version() -> String {
    std::env::var("BASH_ORACLE_VERSION").unwrap_or_else(|_| "5.3.15".to_string())
}

pub fn oracle_version_dir() -> String {
    match oracle_version().as_str() {
        "5.2" | "5.2.21" => "5.2.21".to_string(),
        "5.3" | "5.3.0" | "5.3.15" => "5.3.15".to_string(),
        other => other.to_string(),
    }
}

/// Returns the selected Bash oracle binary.
pub fn oracle_bash_path() -> PathBuf {
    if let Ok(path) = std::env::var("BASH_ORACLE_PATH") {
        return PathBuf::from(path);
    }
    match oracle_version_dir().as_str() {
        "5.2.21" => {
            if let Ok(path) = std::env::var("BASH_521_PATH") {
                return PathBuf::from(path);
            }
            workspace_root().join("target/oracle/bash-5.2.21/bash")
        }
        "5.3.15" => {
            if let Ok(path) = std::env::var("BASH_5315_PATH") {
                return PathBuf::from(path);
            }
            if let Ok(path) = std::env::var("BASH_53_PATH") {
                return PathBuf::from(path);
            }
            workspace_root().join("target/oracle/bash-5.3.15/bash")
        }
        other => workspace_root().join(format!("target/oracle/bash-{other}/bash")),
    }
}

fn oracle_banner_matches(version: &str, banner: &str) -> bool {
    banner.contains(&format!("GNU bash, version {version}("))
}

/// True if the oracle binary exists and reports the selected version.
pub fn oracle_available() -> bool {
    let path = oracle_bash_path();
    if !path.exists() {
        return false;
    }
    let output = Command::new(&path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(out) => {
            let banner = String::from_utf8_lossy(&out.stdout);
            oracle_banner_matches(&oracle_version_dir(), &banner)
        }
        Err(_) => false,
    }
}

fn require_available_oracle(path: PathBuf, available: bool) -> Result<PathBuf, HarnessError> {
    if available {
        Ok(path)
    } else {
        Err(HarnessError::OracleUnavailable(path))
    }
}

/// Returns the selected Bash oracle only when it exists and reports the exact
/// configured patch version.
pub fn required_oracle_bash_path() -> Result<PathBuf, HarnessError> {
    let path = oracle_bash_path();
    require_available_oracle(path, oracle_available())
}

pub fn cherub_path() -> Result<PathBuf, HarnessError> {
    if let Some(value) =
        std::env::var_os("CHERUBSH_BIN").or_else(|| std::env::var_os("CARGO_BIN_EXE_cherubsh"))
    {
        return Ok(PathBuf::from(value));
    }

    let current = std::env::current_exe().map_err(HarnessError::Io)?;
    let Some(profile_dir) = current.parent().and_then(Path::parent) else {
        return Err(HarnessError::MissingCherubBinary);
    };
    let candidate = profile_dir.join(format!("cherubsh{}", std::env::consts::EXE_SUFFIX));
    candidate
        .is_file()
        .then_some(candidate)
        .ok_or(HarnessError::MissingCherubBinary)
}

pub fn run_bash(spec: &RunSpec<'_>) -> Result<RunOutput, HarnessError> {
    run_shell_spec(&default_bash_path(), spec)
}

pub fn run_cherub(spec: &RunSpec<'_>) -> Result<RunOutput, HarnessError> {
    let path = cherub_path()?;
    run_shell_spec(&path, spec)
}

#[derive(Debug, Clone)]
pub struct DiffOutcome {
    pub status_match: bool,
    pub stdout_match: bool,
    pub stderr_match: bool,
    pub stdout_diff: String,
    pub stderr_diff: String,
}

impl DiffOutcome {
    pub fn is_match(&self) -> bool {
        self.status_match && self.stdout_match && self.stderr_match
    }
}

pub fn diff(reference: &RunOutput, candidate: &RunOutput) -> DiffOutcome {
    DiffOutcome {
        status_match: reference.status == candidate.status,
        stdout_match: reference.stdout == candidate.stdout,
        stderr_match: normalize_stderr(&reference.stderr) == normalize_stderr(&candidate.stderr),
        stdout_diff: line_diff(&reference.stdout, &candidate.stdout),
        stderr_diff: line_diff(&reference.stderr, &candidate.stderr),
    }
}

fn normalize_stderr(text: &str) -> String {
    // Strip shell-specific diagnostic prefixes so parity compares the actual
    // error text instead of the executable name or bash's `line N` wrapper.
    text.lines()
        .map(|line| strip_shell_diagnostic_prefix(line).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_shell_diagnostic_prefix(line: &str) -> &str {
    if let Some(rest) = strip_bash_line_prefix(line) {
        return rest;
    }
    if let Some(rest) = line.strip_prefix("bash: ") {
        return strip_bash_line_prefix(rest).unwrap_or(rest);
    }
    if let Some((program, rest)) = line.split_once(": ") {
        let file_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str());
        if matches!(file_name, Some("bash" | "cherubsh")) {
            return strip_bash_line_prefix(rest).unwrap_or(rest);
        }
    }
    line.strip_prefix("cherubsh: ").unwrap_or(line)
}

fn strip_bash_line_prefix(line: &str) -> Option<&str> {
    let marker = ": line ";
    let marker_at = line.find(marker)?;
    let after_marker = &line[marker_at + marker.len()..];
    let colon_at = after_marker.find(": ")?;
    if after_marker[..colon_at].chars().all(|c| c.is_ascii_digit()) {
        Some(&after_marker[colon_at + 2..])
    } else {
        None
    }
}

fn line_diff(left: &str, right: &str) -> String {
    let mut out = String::new();
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    let max = left_lines.len().max(right_lines.len());
    for index in 0..max {
        let l = left_lines.get(index).copied().unwrap_or("<missing>");
        let r = right_lines.get(index).copied().unwrap_or("<missing>");
        if l != r {
            out.push_str(&format!("- {l}\n+ {r}\n"));
        }
    }
    out
}

/// Run `bash -n` on the script. Exit 0 = bash accepts, non-zero = bash rejects.
pub fn bash_parse_check(script: &str) -> Result<i32, HarnessError> {
    let bash_path = default_bash_path();
    let mut command = Command::new(&bash_path);
    command.arg("-n").arg("-c").arg(script);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    let status = command.status().map_err(HarnessError::Io)?;
    status.code().ok_or(HarnessError::MissingStatus)
}

/// Run cherubsh --parse-only on the script. Exit 0 = accept, non-zero = reject.
pub fn cherub_parse_check(script: &str) -> Result<i32, HarnessError> {
    let cherub = cherub_path()?;
    let mut command = Command::new(&cherub);
    command.arg("--parse-only").arg(script);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    let status = command.status().map_err(HarnessError::Io)?;
    status.code().ok_or(HarnessError::MissingStatus)
}

/// Assert that bash and cherubsh agree on whether the script is syntactically valid.
/// Returns true on agreement, false on divergence. Panics on harness errors.
pub fn assert_parser_accepts_like_bash(script: &str) {
    let bash_code = bash_parse_check(script).expect("bash -n");
    let cherub_code = match cherub_parse_check(script) {
        Ok(code) => code,
        Err(HarnessError::MissingCherubBinary) => {
            panic!("CARGO_BIN_EXE_cherubsh not set; run via cargo test");
        }
        Err(err) => panic!("cherub --parse-only: {err}"),
    };
    let bash_accepts = bash_code == 0;
    let cherub_accepts = cherub_code == 0;
    assert_eq!(
        bash_accepts, cherub_accepts,
        "parser divergence on script: {:?}\n  bash exit={} (accepts={})\n  cherub exit={} (accepts={})",
        script, bash_code, bash_accepts, cherub_code, cherub_accepts,
    );
}

/// Compare cherubsh's behavior on `spec` against the bash oracle.
///
/// Two operating modes for fixtures:
/// - **Golden mode:** the fixture file ships expected_status / expected_stdout
///   / expected_stderr sidecars; the harness checks cherubsh's output
///   against those literals (handled by callers, not this function).
/// - **Differential mode:** no sidecars; the oracle is run live and its
///   output is treated as the expected answer. This is what `assert_parity`
///   does - it shells out to bash for ground truth, then diffs cherubsh.
///
/// Oracle resolution requires the selected Bash patch version. Use
/// `tools/run-workspace-tests.sh` to provision it before an ordinary local run.
pub fn assert_parity(spec: &RunSpec<'_>) {
    let bash_path = required_oracle_bash_path().expect("resolve pinned Bash oracle");
    run_assert_parity(&bash_path, spec);
}

/// Run against the selected Bash oracle during a declared parity sweep.
///
/// This remains a separate entry point for callers that declare a full parity
/// sweep, but both ordinary and strict comparisons require the selected oracle.
pub fn assert_parity_strict(spec: &RunSpec<'_>) {
    let bash_path = required_oracle_bash_path().expect("resolve pinned Bash oracle");
    run_assert_parity(&bash_path, spec);
}

fn run_assert_parity(bash_path: &Path, spec: &RunSpec<'_>) {
    let bash_output = run_shell_spec(bash_path, spec).expect("run bash");
    let cherub_output = match run_cherub(spec) {
        Ok(output) => output,
        Err(HarnessError::MissingCherubBinary) => {
            panic!("CARGO_BIN_EXE_cherubsh not set; run via cargo test");
        }
        Err(err) => panic!("run cherubsh: {err}"),
    };
    let outcome = diff(&bash_output, &cherub_output);
    assert!(
        outcome.is_match(),
        "parity mismatch\n  oracle={}\n  spec={:?}\n  status: bash={} cherub={} match={}\n  stdout match={} diff:\n{}\n  stderr match={} diff:\n{}",
        bash_path.display(),
        spec,
        bash_output.status,
        cherub_output.status,
        outcome.status_match,
        outcome.stdout_match,
        outcome.stdout_diff,
        outcome.stderr_match,
        outcome.stderr_diff,
    );
}

#[derive(Debug, Clone)]
pub struct Fixture {
    pub name: String,
    pub script: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub env: Vec<(String, String)>,
    pub expected_status: Option<i32>,
    pub expected_stdout: Option<String>,
    pub expected_stderr: Option<String>,
}

impl Fixture {
    pub fn run_spec(&self) -> RunSpec<'_> {
        RunSpec {
            args: self.args.iter().map(|s| s.as_str()).collect(),
            stdin: self.stdin.as_deref(),
            env: self
                .env
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            env_remove: Vec::new(),
            script: Some(self.script.as_str()),
        }
    }
}

/// Load a fixture from `<fixtures_dir>/<name>.sh`.
/// Optional sidecar files: `<name>.args` (one arg per line),
/// `<name>.stdin` (raw bytes), `<name>.env` (KEY=VALUE per line),
/// `<name>.status`, `<name>.stdout`, `<name>.stderr`.
pub fn load_fixture(fixtures_dir: &Path, name: &str) -> Result<Fixture, HarnessError> {
    let script_path = fixtures_dir.join(format!("{name}.sh"));
    let script = fs::read_to_string(&script_path).map_err(HarnessError::Io)?;

    let read_optional = |suffix: &str| -> Result<Option<String>, HarnessError> {
        let path = fixtures_dir.join(format!("{name}.{suffix}"));
        match fs::read_to_string(&path) {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(HarnessError::Io(err)),
        }
    };

    let args = read_optional("args")?
        .map(|text| {
            text.lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string())
                .collect()
        })
        .unwrap_or_default();
    let stdin = read_optional("stdin")?;
    let env = read_optional("env")?
        .map(|text| {
            text.lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(2, '=');
                    let key = parts.next()?.to_string();
                    let value = parts.next()?.to_string();
                    Some((key, value))
                })
                .collect()
        })
        .unwrap_or_default();
    let expected_status = read_optional("status")?.and_then(|text| text.trim().parse::<i32>().ok());
    let expected_stdout = read_optional("stdout")?;
    let expected_stderr = read_optional("stderr")?;

    Ok(Fixture {
        name: name.to_string(),
        script,
        args,
        stdin,
        env,
        expected_status,
        expected_stdout,
        expected_stderr,
    })
}

pub fn discover_fixtures(fixtures_dir: &Path) -> Result<Vec<String>, HarnessError> {
    let mut names = Vec::new();
    let entries = fs::read_dir(fixtures_dir).map_err(HarnessError::Io)?;
    for entry in entries {
        let entry = entry.map_err(HarnessError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("sh") {
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_detects_status_mismatch() {
        let a = RunOutput {
            status: 0,
            stdout: "x".into(),
            stderr: String::new(),
        };
        let b = RunOutput {
            status: 1,
            stdout: "x".into(),
            stderr: String::new(),
        };
        let outcome = diff(&a, &b);
        assert!(!outcome.status_match);
        assert!(outcome.stdout_match);
    }

    #[test]
    fn normalize_strips_program_prefix() {
        assert_eq!(
            normalize_stderr("bash: line 1: foo\ncherubsh: oops"),
            "foo\noops"
        );
        assert_eq!(
            normalize_stderr("/tmp/bash: line 12: complete: missing: no completion specification"),
            "complete: missing: no completion specification"
        );
        assert_eq!(
            normalize_stderr("/tmp/oracle/bash: connect: Connection refused"),
            "connect: Connection refused"
        );
    }

    #[test]
    fn oracle_path_honors_env_override() {
        // SAFETY: tests in this crate are single-threaded by default for env
        // mutation; if expanded, gate behind a Mutex.
        let prev = std::env::var_os("BASH_ORACLE_PATH");
        std::env::set_var("BASH_ORACLE_PATH", "/some/explicit/path");
        assert_eq!(oracle_bash_path(), PathBuf::from("/some/explicit/path"));
        match prev {
            Some(value) => std::env::set_var("BASH_ORACLE_PATH", value),
            None => std::env::remove_var("BASH_ORACLE_PATH"),
        }
    }

    #[test]
    fn workspace_root_resolves() {
        let root = workspace_root();
        assert!(
            root.join("Cargo.toml").exists(),
            "workspace_root() = {} should contain Cargo.toml",
            root.display()
        );
    }

    #[test]
    fn required_oracle_reports_the_workspace_test_command_when_missing() {
        let path = PathBuf::from("/missing/bash-5.3.15");
        let error = require_available_oracle(path.clone(), false).expect_err("missing oracle");

        assert!(matches!(&error, HarnessError::OracleUnavailable(value) if value == &path));
        assert!(error.to_string().contains("./tools/run-workspace-tests.sh"));
    }

    #[test]
    fn selected_oracle_banner_must_match_the_exact_patch_version() {
        assert!(oracle_banner_matches(
            "5.3.15",
            "GNU bash, version 5.3.15(1)-release"
        ));
        assert!(!oracle_banner_matches(
            "5.3.15",
            "GNU bash, version 5.2.21(1)-release"
        ));
        assert!(!oracle_banner_matches(
            "5.3.15",
            "GNU bash, version 5.3.150(1)-release"
        ));
    }
}
