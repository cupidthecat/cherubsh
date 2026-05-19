//! Driver for the bash-5.2.21 upstream test suite (`bash-5.2.21/tests/`).
//!
//! Each upstream test is a `run-<name>` shell script that invokes
//! `${THIS_SH} ./<name>.tests` and diffs the captured output against
//! `<name>.right`. Driver scripts already do the diff, so we only need to
//! invoke them with `THIS_SH` pointed at cherubsh and report exit status.
//!
//! Tests classified as Pass / Fail / XFail / XPass against an xfail manifest.

use std::collections::BTreeSet;
use std::ffi::{CStr, CString, OsString};
use std::fs;
use std::io::Read;
use std::os::fd::RawFd;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use crate::{oracle_bash_path, workspace_root, HarnessError};

#[derive(Debug, Clone)]
pub struct UpstreamTest {
    pub name: String,
    pub run_script: PathBuf,
    pub right_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct UpstreamOutcome {
    pub name: String,
    pub passed: bool,
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub tstout_path: Option<PathBuf>,
}

/// Discover all `run-*` driver scripts in `tests_dir`, excluding aggregate
/// runners (`run-all`, `run-minimal`) and the nonstandard profiling runner.
pub fn discover_upstream_tests(tests_dir: &Path) -> Vec<UpstreamTest> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(tests_dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let stem = match name.strip_prefix("run-") {
            Some(s) => s.to_string(),
            None => continue,
        };
        if stem == "all" || stem == "minimal" || stem == "gprof" {
            continue;
        }
        let right = tests_dir.join(format!("{stem}.right"));
        out.push(UpstreamTest {
            name: stem,
            run_script: path,
            right_file: if right.exists() { Some(right) } else { None },
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Run an upstream test driver under `shell`, with a wall-clock timeout.
pub fn run_upstream(
    test: &UpstreamTest,
    shell: &Path,
    timeout: Duration,
) -> Result<UpstreamOutcome, HarnessError> {
    let tests_dir = test
        .run_script
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    ensure_upstream_helper_modes(&tests_dir);
    cleanup_upstream_artifacts(&tests_dir);

    let mut command = Command::new(oracle_bash_path());
    command.arg(&test.run_script);
    command.current_dir(&tests_dir);
    command.env_clear();
    // Preserve only what the upstream drivers need; matches the env policy in
    // run_shell_spec so cherubsh sees the same baseline as bash.
    for (key, value) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if matches!(
            k.as_ref(),
            "PATH" | "HOME" | "USER" | "LOGNAME" | "TERM" | "LANG" | "LC_ALL" | "TZ" | "TMPDIR"
        ) {
            command.env(key, value);
        }
    }
    command.env(
        "TMPDIR",
        std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into()),
    );
    // Some minimal/container hosts expose nss-systemd without a responsive
    // userdb service. Upstream procsub tests run `ls -l`, and in that setup
    // both bash and cherubsh can block inside NSS instead of shell code.
    command.env("SYSTEMD_NSS_DYNAMIC_BYPASS", "1");
    command.env("SYSTEMD_BYPASS_USERDB", "1");
    let shell_link_dir = std::env::temp_dir().join(format!(
        "cherub-upstream-shell-{}-{}-{}",
        std::process::id(),
        test.name,
        nanos_suffix(),
    ));
    let shell_for_test =
        bash_named_shell(shell, &shell_link_dir).unwrap_or_else(|| shell.to_path_buf());
    let locale_helper = upstream_locale_helper();
    let support_helper = upstream_support_helper(&tests_dir);
    if let Some(helper) = &locale_helper {
        command.env("LOCPATH", &helper.locale_dir);
    }
    command.env(
        "PATH",
        upstream_path_with_helpers(&tests_dir, locale_helper.as_ref(), support_helper.as_ref()),
    );
    let clean_home = shell_link_dir.join("home");
    let _ = fs::create_dir_all(&clean_home);
    command.env("HOME", &clean_home);
    command.env("THIS_SH", &shell_for_test);
    command.env("BASH", &shell_for_test);
    command.env("BUILD_DIR", tests_dir.parent().unwrap_or(&tests_dir));
    // Upstream drivers redirect to ${BASH_TSTOUT}; the bash-5.2.21 run-all
    // script sets this up centrally, but we run drivers individually so we
    // must provide a unique tmp file per invocation.
    let tstout = std::env::temp_dir().join(format!(
        "cherub-bashtst-{}-{}-{}.out",
        std::process::id(),
        test.name,
        nanos_suffix(),
    ));
    command.env("BASH_TSTOUT", &tstout);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut controlling_pty = ControllingPty::open();
    if let Some(pty) = controlling_pty.as_ref() {
        let slave_path = pty.slave_path.clone();
        let inherited_slave = pty.slave;
        unsafe {
            command.pre_exec(move || {
                libc::setsid();
                let fd = libc::open(slave_path.as_ptr(), libc::O_RDWR);
                if fd >= 0 {
                    libc::ioctl(fd, libc::TIOCSCTTY as libc::c_ulong, 0);
                    libc::close(fd);
                }
                libc::close(inherited_slave);
                Ok(())
            });
        }
    } else {
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(HarnessError::Io)?;
    if let Some(pty) = controlling_pty.as_mut() {
        pty.close_slave();
    }
    let stdout_reader = child.stdout.take().map(spawn_reader);
    let stderr_reader = child.stderr.take().map(spawn_reader);
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(HarnessError::Io)? {
            Some(status) => break status,
            None => {
                if start.elapsed() >= timeout {
                    let pgid = -(child.id() as i32);
                    unsafe {
                        libc_kill(pgid, SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    // Synthesize a 124 exit (matches GNU `timeout` convention).
                    break std::process::ExitStatus::default();
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    };

    let stdout = join_reader(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    let _ = fs::remove_dir_all(&shell_link_dir);

    let code = if timed_out {
        124
    } else {
        match status.code() {
            Some(c) => c,
            None => {
                use std::os::unix::process::ExitStatusExt;
                match status.signal() {
                    Some(sig) => 128 + sig,
                    None => return Err(HarnessError::MissingStatus),
                }
            }
        }
    };
    let passed = !timed_out && code == 0;
    let tstout_path = if passed {
        let _ = fs::remove_file(&tstout);
        None
    } else if tstout.exists() {
        Some(tstout)
    } else {
        None
    };

    Ok(UpstreamOutcome {
        name: test.name.clone(),
        passed,
        status: code,
        stdout,
        stderr,
        timed_out,
        tstout_path,
    })
}

struct ControllingPty {
    master: RawFd,
    slave: RawFd,
    slave_path: CString,
}

impl ControllingPty {
    fn open() -> Option<Self> {
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
            return None;
        }
        let path = unsafe {
            let ptr = libc::ttyname(slave);
            if ptr.is_null() {
                libc::close(master);
                libc::close(slave);
                return None;
            }
            CStr::from_ptr(ptr).to_owned()
        };
        Some(Self {
            master,
            slave,
            slave_path: path,
        })
    }

    fn close_slave(&mut self) {
        if self.slave >= 0 {
            unsafe {
                libc::close(self.slave);
            }
            self.slave = -1;
        }
    }
}

impl Drop for ControllingPty {
    fn drop(&mut self) {
        if self.master >= 0 {
            unsafe {
                libc::close(self.master);
            }
        }
        if self.slave >= 0 {
            unsafe {
                libc::close(self.slave);
            }
        }
    }
}

fn bash_named_shell(shell: &Path, dir: &Path) -> Option<PathBuf> {
    fs::create_dir_all(dir).ok()?;
    let link = dir.join("bash");
    symlink(shell, &link).ok()?;
    Some(link)
}

#[derive(Debug, Clone)]
struct UpstreamLocaleHelper {
    bin_dir: PathBuf,
    locale_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct UpstreamSupportHelper {
    bin_dir: PathBuf,
}

static UPSTREAM_LOCALE_HELPER: OnceLock<Option<UpstreamLocaleHelper>> = OnceLock::new();
static UPSTREAM_SUPPORT_HELPER: OnceLock<Option<UpstreamSupportHelper>> = OnceLock::new();

fn upstream_locale_helper() -> Option<UpstreamLocaleHelper> {
    UPSTREAM_LOCALE_HELPER
        .get_or_init(prepare_upstream_locale_helper)
        .clone()
}

fn prepare_upstream_locale_helper() -> Option<UpstreamLocaleHelper> {
    let root = std::env::temp_dir().join(format!(
        "cherub-upstream-locale-{}-{}",
        std::process::id(),
        nanos_suffix(),
    ));
    let bin_dir = root.join("bin");
    let locale_dir = root.join("locales");
    fs::create_dir_all(&bin_dir).ok()?;
    fs::create_dir_all(&locale_dir).ok()?;

    for (name, input, charset, allow_ascii_warning) in [
        ("de_DE.UTF-8", "de_DE", "UTF-8", false),
        ("fr_FR.ISO8859-1", "fr_FR", "ISO-8859-1", false),
        ("ja_JP.SJIS", "ja_JP", "SHIFT_JIS", true),
        ("zh_HK.big5hkscs", "zh_HK", "BIG5-HKSCS", false),
        ("zh_TW.BIG5", "zh_TW", "BIG5", false),
    ] {
        let mut cmd = Command::new("localedef");
        if allow_ascii_warning {
            cmd.arg("--no-warnings=ascii");
        }
        let status = cmd
            .arg("-i")
            .arg(input)
            .arg("-f")
            .arg(charset)
            .arg(locale_dir.join(name))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&root);
            return None;
        }
    }

    if write_locale_wrapper(&bin_dir.join("locale")).is_err() {
        let _ = fs::remove_dir_all(&root);
        return None;
    }

    Some(UpstreamLocaleHelper {
        bin_dir,
        locale_dir,
    })
}

fn upstream_support_helper(tests_dir: &Path) -> Option<UpstreamSupportHelper> {
    if upstream_helpers_exist(tests_dir) {
        return None;
    }
    UPSTREAM_SUPPORT_HELPER
        .get_or_init(prepare_upstream_support_helper)
        .clone()
}

fn prepare_upstream_support_helper() -> Option<UpstreamSupportHelper> {
    let vendor_dir = workspace_root().join("vendor/bash-5.2.21");
    let source_dir = vendor_dir.join("support");
    let root = std::env::temp_dir().join(format!(
        "cherub-upstream-support-{}-{}",
        std::process::id(),
        nanos_suffix(),
    ));
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).ok()?;

    for name in ["recho", "zecho", "printenv", "xcase"] {
        let source = source_dir.join(format!("{name}.c"));
        let output = bin_dir.join(name);
        if !source.exists() {
            let _ = fs::remove_dir_all(&root);
            return None;
        }
        let status = Command::new("cc")
            .arg("-std=gnu89")
            .arg("-DHAVE_STRING_H=1")
            .arg("-DHAVE_STDLIB_H=1")
            .arg("-DHAVE_UNISTD_H=1")
            .arg("-I")
            .arg(&vendor_dir)
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&root);
            return None;
        }
    }

    Some(UpstreamSupportHelper { bin_dir })
}

fn upstream_helpers_exist(tests_dir: &Path) -> bool {
    ["recho", "zecho", "printenv", "xcase"]
        .iter()
        .all(|name| is_executable(&tests_dir.join(name)))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn write_locale_wrapper(path: &Path) -> std::io::Result<()> {
    fs::write(
        path,
        "#!/bin/sh\n\
         if [ \"$1\" = \"-a\" ]; then\n\
         \t/usr/bin/locale -a\n\
         \techo de_DE.UTF-8\n\
         \techo fr_FR.ISO8859-1\n\
         \techo ja_JP.SJIS\n\
         \techo zh_HK.big5hkscs\n\
         \techo zh_TW.BIG5\n\
         else\n\
         \texec /usr/bin/locale \"$@\"\n\
         fi\n",
    )?;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(path, perms)
}

fn upstream_path_with_helpers(
    tests_dir: &Path,
    locale_helper: Option<&UpstreamLocaleHelper>,
    support_helper: Option<&UpstreamSupportHelper>,
) -> OsString {
    let original = std::env::var_os("PATH").unwrap_or_default();
    let mut path = OsString::new();
    if let Some(helper) = locale_helper {
        path.push(helper.bin_dir.as_os_str());
        path.push(":");
    }
    if let Some(helper) = support_helper {
        path.push(helper.bin_dir.as_os_str());
        path.push(":");
    }
    path.push(tests_dir.as_os_str());
    path.push(":.:");
    path.push(original);
    path
}

fn cleanup_upstream_artifacts(tests_dir: &Path) {
    // Some upstream tests create simple glob fixtures in their working
    // directory. We run drivers individually and repeatedly, so stale fixtures
    // can perturb later tests that compare against checked-in .right files.
    for name in [
        "a",
        ".b",
        "a.log",
        "[3]=abcde",
        "_[]",
        "r",
        "s",
        "t",
        "u",
        "v",
    ] {
        let path = tests_dir.join(name);
        if path.is_file() || path.is_symlink() {
            let _ = fs::remove_file(path);
        }
    }
}

fn ensure_upstream_helper_modes(tests_dir: &Path) {
    let Ok(entries) = fs::read_dir(tests_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("sub") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mut perms = meta.permissions();
        let mode = perms.mode();
        if mode & 0o111 == 0 {
            perms.set_mode(mode | 0o755);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

fn spawn_reader<R>(mut pipe: R) -> thread::JoinHandle<Result<String, std::io::Error>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut out = Vec::new();
        pipe.read_to_end(&mut out)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    })
}

fn join_reader(
    reader: Option<thread::JoinHandle<Result<String, std::io::Error>>>,
) -> Result<String, HarnessError> {
    match reader {
        Some(handle) => match handle.join() {
            Ok(result) => result.map_err(HarnessError::Io),
            Err(_) => Ok(String::new()),
        },
        None => Ok(String::new()),
    }
}

const SIGKILL: i32 = 9;

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    kill(pid, sig)
}

fn nanos_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Load an xfail manifest. Plain text; one test name per line; lines starting
/// with `#` and blank lines are ignored. Missing file ⇒ empty set.
pub fn load_xfail(path: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return out,
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        out.insert(trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn xfail_loader_strips_comments_and_blanks() {
        let dir = tempdir();
        let path = dir.join("xfail.txt");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "# header comment").unwrap();
        writeln!(f, "alpha").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "  beta  ").unwrap();
        writeln!(f, "# trailing").unwrap();
        let set = load_xfail(&path);
        assert!(set.contains("alpha"));
        assert!(set.contains("beta"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn xfail_loader_missing_is_empty() {
        let set = load_xfail(Path::new("/no/such/file/please"));
        assert!(set.is_empty());
    }

    #[test]
    fn discover_skips_aggregate_runners() {
        let dir = tempdir();
        for name in &[
            "run-all",
            "run-minimal",
            "run-foo",
            "run-bar",
            "foo.right",
            "other",
        ] {
            fs::File::create(dir.join(name)).unwrap();
        }
        let tests = discover_upstream_tests(&dir);
        let names: Vec<_> = tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["bar", "foo"]);
        let foo = tests.iter().find(|t| t.name == "foo").unwrap();
        assert!(foo.right_file.is_some());
        let bar = tests.iter().find(|t| t.name == "bar").unwrap();
        assert!(bar.right_file.is_none());
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cherub-upstream-{}-{}",
            std::process::id(),
            random_suffix()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn random_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}
