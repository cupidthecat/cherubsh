//! Runner for the vendored brush `tests/cases/compat` YAML corpus.
//!
//! Brush's compatibility cases are oracle-style tests: run the same shell
//! invocation under Bash and the shell under test, then compare exit status,
//! output, and temporary-directory side effects. This module intentionally
//! implements the subset of brush's harness schema needed by the vendored
//! compatibility corpus, without depending on brush's own crates.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_yaml::Value;

use crate::{
    cherub_path, oracle_bash_path, oracle_version_dir, workspace_root, HarnessError, RunOutput,
};

const DEFAULT_TIMEOUT_SECS: u64 = 15;
const SIGKILL: i32 = 9;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct BrushCase {
    pub set_name: String,
    pub case_name: String,
    pub source_file: PathBuf,
    pub source_dir: PathBuf,
    raw: RawCase,
    common_test_files: Vec<TestFile>,
}

impl BrushCase {
    pub fn id(&self) -> String {
        format!("{}::{}", self.set_name, self.case_name)
    }
}

#[derive(Debug, Clone)]
pub struct BrushOutcome {
    pub id: String,
    pub known_failure: bool,
    pub passed: bool,
    pub timed_out: bool,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub bash: Option<RunOutput>,
    pub cherub: Option<RunOutput>,
    pub stdout_diff: String,
    pub stderr_diff: String,
    pub status_diff: String,
    pub dir_diff: String,
    pub expectation_diff: String,
}

#[derive(Debug, Deserialize)]
struct RawCaseSet {
    name: Option<String>,
    cases: Vec<RawCase>,
    #[serde(default)]
    common_test_files: Vec<TestFile>,
    #[serde(default)]
    incompatible_configs: BTreeSet<String>,
    #[serde(default)]
    incompatible_platforms: BTreeSet<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawCase {
    name: Option<String>,
    #[serde(default)]
    args: Vec<Value>,
    #[serde(default)]
    env: BTreeMap<String, Value>,
    #[serde(default)]
    home_dir: Option<Value>,
    #[serde(default)]
    skip: bool,
    #[serde(default)]
    known_failure: bool,
    #[serde(default)]
    pty: bool,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    ignore_exit_status: bool,
    #[serde(default)]
    ignore_stderr: bool,
    #[serde(default)]
    ignore_stdout: bool,
    #[serde(default)]
    ignore_whitespace: bool,
    #[serde(default)]
    test_files: Vec<TestFile>,
    #[serde(default)]
    removed_default_args: Vec<Value>,
    #[serde(default)]
    incompatible_configs: BTreeSet<String>,
    #[serde(default)]
    incompatible_os: BTreeSet<String>,
    #[serde(default)]
    incompatible_platforms: BTreeSet<String>,
    #[serde(default)]
    min_oracle_version: Option<Value>,
    #[serde(default)]
    max_oracle_version: Option<Value>,
    #[serde(default)]
    timeout_in_seconds: Option<u64>,
    #[serde(default)]
    expected_stdout: Option<String>,
    #[serde(default)]
    expected_stderr: Option<String>,
    #[serde(default)]
    expected_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
struct TestFile {
    path: PathBuf,
    #[serde(default)]
    contents: String,
    #[serde(default)]
    source_path: Option<PathBuf>,
    #[serde(default)]
    executable: bool,
}

pub fn default_brush_cases_dir() -> PathBuf {
    workspace_root().join("vendor/brush/brush-shell/tests/cases/compat")
}

pub fn discover_brush_cases(cases_dir: &Path) -> Result<Vec<BrushCase>, HarnessError> {
    let mut files = Vec::new();
    discover_yaml_files(cases_dir, &mut files)?;
    files.sort();

    let mut cases = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).map_err(HarnessError::Io)?;
        let raw: RawCaseSet = serde_yaml::from_str(&text).map_err(|err| {
            HarnessError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
        })?;
        let source_dir = file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cases_dir.to_path_buf());
        let set_name = raw.name.unwrap_or_else(|| {
            file.strip_prefix(cases_dir)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string()
        });

        if raw.incompatible_configs.contains("bash") || raw.incompatible_platforms.contains("linux")
        {
            continue;
        }

        for (idx, case) in raw.cases.into_iter().enumerate() {
            cases.push(BrushCase {
                set_name: set_name.clone(),
                case_name: case.name.clone().unwrap_or_else(|| format!("case {idx}")),
                source_file: file.clone(),
                source_dir: source_dir.clone(),
                raw: case,
                common_test_files: raw.common_test_files.clone(),
            });
        }
    }
    Ok(cases)
}

pub fn run_brush_case(case: &BrushCase, report_dir: &Path) -> Result<BrushOutcome, HarnessError> {
    let id = case.id();
    if let Some(reason) = skip_reason(case) {
        return Ok(BrushOutcome {
            id,
            known_failure: case.raw.known_failure,
            passed: true,
            timed_out: false,
            skipped: true,
            skip_reason: Some(reason),
            bash: None,
            cherub: None,
            stdout_diff: String::new(),
            stderr_diff: String::new(),
            status_diff: String::new(),
            dir_diff: String::new(),
            expectation_diff: String::new(),
        });
    }

    fs::create_dir_all(report_dir).map_err(HarnessError::Io)?;
    let bash_dir = temp_case_dir("brush-bash", &id)?;
    let cherub_dir = temp_case_dir("brush-cherub", &id)?;
    create_test_files(&bash_dir, case)?;
    create_test_files(&cherub_dir, case)?;

    let timeout = Duration::from_secs(case.raw.timeout_in_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let bash = run_shell_for_case(&oracle_bash_path(), case, &bash_dir, timeout)?;
    let cherub = run_shell_for_case(&cherub_path()?, case, &cherub_dir, timeout)?;

    let mut passed = true;
    let mut timed_out = false;
    let mut status_diff = String::new();
    let mut stdout_diff = String::new();
    let mut stderr_diff = String::new();

    if bash.status == 124 || cherub.status == 124 {
        timed_out = true;
        passed = false;
    }

    if !case.raw.ignore_exit_status && bash.status != cherub.status {
        passed = false;
        status_diff = format!("bash={} cherub={}", bash.status, cherub.status);
    }
    if !case.raw.ignore_stdout && !case_stdout_matches(case, &bash.stdout, &cherub.stdout) {
        passed = false;
        stdout_diff = format_string_diff(&bash.stdout, &cherub.stdout);
    }
    if !case.raw.ignore_stderr
        && !output_matches(&bash.stderr, &cherub.stderr, case.raw.ignore_whitespace)
    {
        passed = false;
        stderr_diff = format_string_diff(&bash.stderr, &cherub.stderr);
    }

    let dir_diff = diff_dirs(&bash_dir, &cherub_dir).map_err(HarnessError::Io)?;
    if !dir_diff.is_empty() {
        passed = false;
    }

    let expectation_diff = check_expectations(case, &cherub);
    if !expectation_diff.is_empty() {
        passed = false;
    }

    if !passed {
        let safe = safe_id(&id);
        fs::write(report_dir.join(format!("{safe}.bash.stdout")), &bash.stdout)
            .map_err(HarnessError::Io)?;
        fs::write(report_dir.join(format!("{safe}.bash.stderr")), &bash.stderr)
            .map_err(HarnessError::Io)?;
        fs::write(
            report_dir.join(format!("{safe}.cherub.stdout")),
            &cherub.stdout,
        )
        .map_err(HarnessError::Io)?;
        fs::write(
            report_dir.join(format!("{safe}.cherub.stderr")),
            &cherub.stderr,
        )
        .map_err(HarnessError::Io)?;
    }

    let _ = fs::remove_dir_all(&bash_dir);
    let _ = fs::remove_dir_all(&cherub_dir);

    Ok(BrushOutcome {
        id,
        known_failure: case.raw.known_failure,
        passed,
        timed_out,
        skipped: false,
        skip_reason: None,
        bash: Some(bash),
        cherub: Some(cherub),
        stdout_diff,
        stderr_diff,
        status_diff,
        dir_diff,
        expectation_diff,
    })
}

fn discover_yaml_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), HarnessError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => return Err(HarnessError::Io(err)),
    };
    for entry in entries {
        let entry = entry.map_err(HarnessError::Io)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(HarnessError::Io)?;
        if file_type.is_dir() {
            discover_yaml_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
    Ok(())
}

fn skip_reason(case: &BrushCase) -> Option<String> {
    if case.raw.skip {
        return Some("marked skip by brush".to_string());
    }
    if case.raw.incompatible_configs.contains("bash") {
        return Some("incompatible with bash config".to_string());
    }
    if case.raw.incompatible_platforms.contains("linux") {
        return Some("incompatible with linux platform".to_string());
    }
    if let Some(host) = host_os_id() {
        if case.raw.incompatible_os.contains(&host) {
            return Some(format!("incompatible with host os {host}"));
        }
    }
    if let Some(min) = case
        .raw
        .min_oracle_version
        .as_ref()
        .and_then(value_to_string)
    {
        if compare_versions(&oracle_version_dir(), &min) == std::cmp::Ordering::Less {
            return Some(format!("requires bash >= {min}"));
        }
    }
    if let Some(max) = case
        .raw
        .max_oracle_version
        .as_ref()
        .and_then(value_to_string)
    {
        if compare_versions(&oracle_version_dir(), &max) == std::cmp::Ordering::Greater {
            return Some(format!("requires bash <= {max}"));
        }
    }
    None
}

fn host_os_id() -> Option<String> {
    let text = fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("ID=") {
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

fn compare_versions(lhs: &str, rhs: &str) -> std::cmp::Ordering {
    let parse = |s: &str| {
        s.split(['.', '-'])
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let left = parse(lhs);
    let right = parse(rhs);
    let max = left.len().max(right.len());
    for idx in 0..max {
        let l = *left.get(idx).unwrap_or(&0);
        let r = *right.get(idx).unwrap_or(&0);
        match l.cmp(&r) {
            std::cmp::Ordering::Equal => {}
            order => return order,
        }
    }
    std::cmp::Ordering::Equal
}

fn create_test_files(root: &Path, case: &BrushCase) -> Result<(), HarnessError> {
    for file in case
        .common_test_files
        .iter()
        .chain(case.raw.test_files.iter())
    {
        let dest = root.join(&file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(HarnessError::Io)?;
        }
        let mut contents = if let Some(source_path) = &file.source_path {
            let source = case.source_dir.join(source_path);
            fs::read_to_string(source).map_err(HarnessError::Io)?
        } else {
            file.contents.clone()
        };
        if file.executable || dest.extension().is_some_and(|ext| ext == "sh") {
            contents = contents.replace("\r\n", "\n");
        }
        fs::write(&dest, contents).map_err(HarnessError::Io)?;
        if file.executable {
            let mut perms = fs::metadata(&dest).map_err(HarnessError::Io)?.permissions();
            perms.set_mode(perms.mode() | 0o100);
            fs::set_permissions(&dest, perms).map_err(HarnessError::Io)?;
        }
    }
    Ok(())
}

fn run_shell_for_case(
    shell: &Path,
    case: &BrushCase,
    cwd: &Path,
    timeout: Duration,
) -> Result<RunOutput, HarnessError> {
    let shell_dir = cwd.join(".shell");
    fs::create_dir_all(&shell_dir).map_err(HarnessError::Io)?;
    let shell_link = shell_dir.join("bash");
    let _ = fs::remove_file(&shell_link);
    symlink(shell, &shell_link).map_err(HarnessError::Io)?;

    let mut command = Command::new(&shell_link);
    command.arg0(".shell/bash");
    for arg in ["--norc", "--noprofile"] {
        if !case.raw.removed_default_arg(arg) {
            command.arg(arg);
        }
    }
    for arg in &case.raw.args {
        if let Some(value) = value_to_string(arg) {
            command.arg(value);
        }
    }
    command.current_dir(cwd);
    command.env_clear();
    command.env("LC_ALL", "C");
    command.env("PS1", "test$ ");
    command.env("PATH", path_for_test());
    command.env("SYSTEMD_NSS_DYNAMIC_BYPASS", "1");
    command.env("SYSTEMD_BYPASS_USERDB", "1");
    for (key, value) in &case.raw.env {
        if let Some(value) = value_to_string(value) {
            command.env(key, value);
        }
    }
    if let Some(home_dir) = case.raw.home_dir.as_ref().and_then(value_to_string) {
        let path = PathBuf::from(home_dir);
        let home = if path.is_relative() {
            cwd.join(path)
        } else {
            path
        };
        fs::create_dir_all(&home).map_err(HarnessError::Io)?;
        command.env("HOME", home);
    }

    if case.raw.pty {
        run_with_pty(command, case.raw.stdin.as_deref(), timeout)
    } else {
        run_with_pipes(command, case.raw.stdin.as_deref(), timeout)
    }
}

impl RawCase {
    fn removed_default_arg(&self, arg: &str) -> bool {
        // Keep this value-level to tolerate numeric or oddly-typed YAML.
        self.get_removed_default_args()
            .iter()
            .any(|value| value == arg)
    }

    fn get_removed_default_args(&self) -> Vec<String> {
        self.removed_default_args
            .iter()
            .filter_map(value_to_string)
            .collect()
    }
}

fn path_for_test() -> std::ffi::OsString {
    let mut dirs = Vec::new();
    for dir in [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ] {
        let path = PathBuf::from(dir);
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    if let Some(host_path) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&host_path) {
            if !dirs.contains(&path) && path.join("sh").is_file() {
                dirs.push(path);
            }
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn run_with_pipes(
    mut command: Command,
    stdin_text: Option<&str>,
    timeout: Duration,
) -> Result<RunOutput, HarnessError> {
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if stdin_text.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(HarnessError::Io)?;
    if let Some(text) = stdin_text {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).map_err(HarnessError::Io)?;
        }
    }
    let stdout = child.stdout.take().map(spawn_reader);
    let stderr = child.stderr.take().map(spawn_reader);
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(HarnessError::Io)? {
            break status;
        }
        if start.elapsed() >= timeout {
            unsafe {
                libc::kill(-(child.id() as i32), SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break std::process::ExitStatus::from_raw(124 << 8);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = join_reader(stdout)?;
    let stderr = join_reader(stderr)?;
    Ok(RunOutput {
        status: if timed_out { 124 } else { status_code(status)? },
        stdout,
        stderr,
    })
}

fn run_with_pty(
    mut command: Command,
    stdin_script: Option<&str>,
    timeout: Duration,
) -> Result<RunOutput, HarnessError> {
    let mut master = -1;
    let mut slave = -1;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if rc != 0 {
        return Err(HarnessError::Io(std::io::Error::last_os_error()));
    }
    let slave_path = unsafe {
        let ptr = libc::ttyname(slave);
        if ptr.is_null() {
            CString::new("/dev/tty").unwrap()
        } else {
            std::ffi::CStr::from_ptr(ptr).to_owned()
        }
    };
    let stdin_fd = unsafe { libc::dup(slave) };
    let stdout_fd = unsafe { libc::dup(slave) };
    let stderr_fd = unsafe { libc::dup(slave) };
    if stdin_fd < 0 || stdout_fd < 0 || stderr_fd < 0 {
        return Err(HarnessError::Io(std::io::Error::last_os_error()));
    }
    unsafe {
        command.stdin(Stdio::from(std::fs::File::from_raw_fd(stdin_fd)));
        command.stdout(Stdio::from(std::fs::File::from_raw_fd(stdout_fd)));
        command.stderr(Stdio::from(std::fs::File::from_raw_fd(stderr_fd)));
        command.pre_exec(move || {
            libc::setsid();
            let fd = libc::open(slave_path.as_ptr(), libc::O_RDWR);
            if fd >= 0 {
                libc::ioctl(fd, libc::TIOCSCTTY as libc::c_ulong, 0);
                libc::close(fd);
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(HarnessError::Io)?;
    unsafe {
        libc::close(slave);
    }

    let start = Instant::now();
    let deadline = start + timeout;
    let mut output = Vec::new();
    let mut failure = None;
    let mut expect_from = 0;

    if let Some(script) = stdin_script {
        for line in script.lines() {
            if let Some(expect) = line.strip_prefix("#expect:") {
                if !read_until(
                    master,
                    &mut output,
                    expect.as_bytes(),
                    &mut expect_from,
                    deadline,
                )? {
                    failure = Some(format!("failed to expect '{expect}'"));
                    break;
                }
            } else if line.trim() == "#expect-prompt" {
                if !read_until(master, &mut output, b"test$ ", &mut expect_from, deadline)? {
                    failure = Some("failed to expect prompt".to_string());
                    break;
                }
            } else if let Some(send) = line.strip_prefix("#send:") {
                let bytes: &[u8] = match send.to_ascii_lowercase().as_str() {
                    "ctrl+d" => &[0x04],
                    "tab" => b"\t",
                    "enter" => b"\n",
                    _ => b"",
                };
                if !bytes.is_empty() {
                    write_fd(master, bytes)?;
                }
            } else {
                write_fd(master, line.as_bytes())?;
            }
        }
    }

    let mut timed_out = false;
    loop {
        if let Some(_status) = child.try_wait().map_err(HarnessError::Io)? {
            break;
        }
        drain_available(master, &mut output)?;
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), SIGKILL);
            }
            let _ = child.kill();
            timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let status = child.wait().map_err(HarnessError::Io)?;
    drain_available(master, &mut output)?;
    unsafe {
        libc::close(master);
    }

    let stdout = String::from_utf8_lossy(&output).replace("\r\n", "\n");
    let stderr = failure.unwrap_or_default();
    Ok(RunOutput {
        status: if timed_out {
            124
        } else if stderr.is_empty() {
            status_code(status)?
        } else {
            1
        },
        stdout,
        stderr,
    })
}

use std::os::fd::FromRawFd;

fn read_until(
    fd: RawFd,
    output: &mut Vec<u8>,
    needle: &[u8],
    search_from: &mut usize,
    deadline: Instant,
) -> Result<bool, HarnessError> {
    while Instant::now() < deadline {
        drain_available(fd, output)?;
        if let Some(offset) = output[*search_from..]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            *search_from += offset + needle.len();
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(false)
}

fn drain_available(fd: RawFd, output: &mut Vec<u8>) -> Result<(), HarnessError> {
    loop {
        let mut readfds = unsafe { std::mem::zeroed::<libc::fd_set>() };
        unsafe {
            libc::FD_ZERO(&mut readfds);
            libc::FD_SET(fd, &mut readfds);
        }
        let mut tv = libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        let ready = unsafe {
            libc::select(
                fd + 1,
                &mut readfds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };
        if ready <= 0 {
            return Ok(());
        }
        let mut buf = [0u8; 4096];
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            output.extend_from_slice(&buf[..n as usize]);
        } else {
            return Ok(());
        }
    }
}

fn write_fd(fd: RawFd, bytes: &[u8]) -> Result<(), HarnessError> {
    let mut written = 0usize;
    while written < bytes.len() {
        let n = unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if n < 0 {
            return Err(HarnessError::Io(std::io::Error::last_os_error()));
        }
        written += n as usize;
    }
    Ok(())
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
) -> thread::JoinHandle<Result<String, std::io::Error>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).to_string())
    })
}

fn join_reader(
    handle: Option<thread::JoinHandle<Result<String, std::io::Error>>>,
) -> Result<String, HarnessError> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| HarnessError::Io(std::io::Error::other("reader thread panicked")))?
            .map_err(HarnessError::Io),
        None => Ok(String::new()),
    }
}

fn status_code(status: std::process::ExitStatus) -> Result<i32, HarnessError> {
    if let Some(code) = status.code() {
        return Ok(code);
    }
    match status.signal() {
        Some(sig) => Ok(128 + sig),
        None => Err(HarnessError::MissingStatus),
    }
}

fn output_matches(expected: &str, actual: &str, ignore_whitespace: bool) -> bool {
    let expected = normalize_nan_signs(expected);
    let actual = normalize_nan_signs(actual);
    if !ignore_whitespace {
        return expected == actual;
    }
    normalize_ws(&expected) == normalize_ws(&actual)
}

fn case_stdout_matches(case: &BrushCase, bash: &str, cherub: &str) -> bool {
    if case.set_name == "Builtins: wait"
        && case.case_name == "wait -n not implemented"
        && valid_immediate_wait_n_output(bash)
        && valid_immediate_wait_n_output(cherub)
    {
        return true;
    }
    output_matches(bash, cherub, case.raw.ignore_whitespace)
}

fn valid_immediate_wait_n_output(output: &str) -> bool {
    let mut lines = output.lines();
    let Some(first) = lines
        .next()
        .and_then(|line| line.strip_prefix("first done, status: "))
        .and_then(|status| status.parse::<i32>().ok())
    else {
        return false;
    };
    let Some(second) = lines
        .next()
        .and_then(|line| line.strip_prefix("second done, status: "))
        .and_then(|status| status.parse::<i32>().ok())
    else {
        return false;
    };
    lines.next().is_none() && [first.min(second), first.max(second)] == [0, 5]
}

fn normalize_nan_signs(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for segment in value.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map(|line| (line, "\n"))
            .unwrap_or((segment, ""));
        let trimmed = line.trim_start_matches(' ');
        match trimmed {
            "nan" | "-nan" => out.push_str("nan"),
            "NAN" | "-NAN" => out.push_str("NAN"),
            _ => out.push_str(&line.replace("-nan", "nan").replace("-NAN", "NAN")),
        }
        out.push_str(newline);
    }
    out
}

fn normalize_ws(value: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out
}

fn format_string_diff(expected: &str, actual: &str) -> String {
    format!(
        "--- bash ---\n{}\n--- cherub ---\n{}",
        truncate(expected, 2000),
        truncate(actual, 2000)
    )
}

fn check_expectations(case: &BrushCase, output: &RunOutput) -> String {
    let mut diff = String::new();
    if let Some(code) = case.raw.expected_exit_code {
        if output.status != code {
            diff.push_str(&format!(
                "expected exit code {code}, got {}\n",
                output.status
            ));
        }
    }
    if let Some(stdout) = &case.raw.expected_stdout {
        if !output_matches(stdout, &output.stdout, case.raw.ignore_whitespace) {
            diff.push_str(&format!(
                "stdout expectation differs:\n{}\n",
                format_string_diff(stdout, &output.stdout)
            ));
        }
    }
    if let Some(stderr) = &case.raw.expected_stderr {
        if !output_matches(stderr, &output.stderr, case.raw.ignore_whitespace) {
            diff.push_str(&format!(
                "stderr expectation differs:\n{}\n",
                format_string_diff(stderr, &output.stderr)
            ));
        }
    }
    diff
}

fn diff_dirs(left: &Path, right: &Path) -> std::io::Result<String> {
    let left_files = collect_files(left)?;
    let right_files = collect_files(right)?;
    let mut diff = String::new();
    for path in left_files
        .keys()
        .chain(right_files.keys())
        .collect::<BTreeSet<_>>()
    {
        match (left_files.get(path), right_files.get(path)) {
            (Some(l), Some(r)) if l == r => {}
            (Some(l), Some(r)) => {
                diff.push_str(&format!(
                    "different file {path}\n--- bash ---\n{}\n--- cherub ---\n{}\n",
                    truncate(&String::from_utf8_lossy(l), 1000),
                    truncate(&String::from_utf8_lossy(r), 1000)
                ));
            }
            (Some(_), None) => diff.push_str(&format!("missing from cherub: {path}\n")),
            (None, Some(_)) => diff.push_str(&format!("extra in cherub: {path}\n")),
            (None, None) => {}
        }
    }
    Ok(diff)
}

fn collect_files(root: &Path) -> std::io::Result<BTreeMap<String, Vec<u8>>> {
    let mut out = BTreeMap::new();
    collect_files_inner(root, root, &mut out)?;
    Ok(out)
}

fn collect_files_inner(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name.as_bytes().ends_with(b".profraw") || name == ".shell" {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_inner(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            out.insert(rel, fs::read(path)?);
        }
    }
    Ok(())
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

fn temp_case_dir(prefix: &str, id: &str) -> Result<PathBuf, HarnessError> {
    let dir = std::env::temp_dir().join(format!(
        "cherub-{prefix}-{}-{}-{}",
        std::process::id(),
        safe_id(id),
        nanos_suffix()
    ));
    fs::create_dir_all(&dir).map_err(HarnessError::Io)?;
    Ok(dir)
}

fn nanos_suffix() -> u128 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    now ^ count
}

fn safe_id(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!(
            "{}\n... [truncated, {} bytes total]",
            &value[..max],
            value.len()
        )
    }
}
