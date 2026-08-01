//! Deterministic coverage for Bash's `tests/misc` scripts.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cherubsh_test_harness::{
    cherub_path, diff, oracle_bash_path, run_shell_spec, workspace_root, RunOutput, RunSpec,
};

const AUTOMATED_CASES: &[&str] = &[
    "read-nchars.tests",
    "redir-t2.sh",
    "run-r2.sh",
    "sigint-1.sh",
    "sigint-2.sh",
    "sigint-3.sh",
    "sigint-4.sh",
    "test-minus-e.1",
    "test-minus-e.2",
    "wait-bg.tests",
];

#[test]
fn misc_manifest_classifies_every_vendor_file() {
    let manifest =
        fs::read_to_string(workspace_root().join("crates/test-harness/bash-misc-cases.txt"))
            .expect("read misc manifest");
    let rows = manifest_rows(&manifest);
    let classified = rows
        .iter()
        .map(|(_, file, _)| file.as_str())
        .collect::<BTreeSet<_>>();
    let vendor = fs::read_dir(misc_dir())
        .expect("read Bash misc directory")
        .map(|entry| {
            entry
                .expect("read Bash misc entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(classified, vendor.iter().map(String::as_str).collect());

    let automated = rows
        .iter()
        .filter(|(mode, _, _)| mode != "benchmark" && mode != "network")
        .map(|(_, file, _)| file.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(automated, AUTOMATED_CASES.iter().copied().collect());
}

#[test]
fn test_minus_e_scripts_match_pinned_bash() {
    for case in ["test-minus-e.1", "test-minus-e.2"] {
        let bash = run_script_in_isolated_directory(&oracle_bash_path(), case, Some("yes\n"));
        let cherub = run_script_in_isolated_directory(
            &cherub_path().expect("cherub binary"),
            case,
            Some("yes\n"),
        );
        let outcome = diff(&bash, &cherub);
        assert!(
            outcome.is_match(),
            "case={case}: {outcome:?}; bash={bash:?}; cherub={cherub:?}"
        );
    }
}

#[test]
fn wait_bg_matches_pinned_bash_without_wall_clock_sleep() {
    let source = fs::read_to_string(misc_dir().join("wait-bg.tests"))
        .expect("read wait-bg.tests")
        .replace("sleep 4", "sleep 0.05")
        .replace("echo $$: Job $i: pid is $pid rv=$rv", "echo Job $i rv=$rv")
        .replace(
            "echo Waiting for job $i '('pid $wpid')'",
            "echo Waiting for job $i",
        );
    let spec = RunSpec {
        args: vec!["wait-bg", "3"],
        script: Some(&source),
        ..RunSpec::default()
    };
    let bash = run_shell_spec(&oracle_bash_path(), &spec).expect("run Bash wait-bg");
    let cherub = run_shell_spec(&cherub_path().expect("cherub binary"), &spec)
        .expect("run CherubSH wait-bg");
    assert_eq!(cherub, bash);
}

#[test]
fn sigint_scripts_match_pinned_bash_with_short_sleeps() {
    for case in ["sigint-1.sh", "sigint-2.sh", "sigint-3.sh", "sigint-4.sh"] {
        let mut source = fs::read_to_string(misc_dir().join(case))
            .unwrap_or_else(|error| panic!("read {case}: {error}"));
        if case == "sigint-1.sh" {
            source = source.replacen(
                "\tsleep 5",
                "\tif (( i == 1 )); then\n\t\tpython3 -c 'import os, signal, sys; signal.signal(signal.SIGINT, lambda *_: os._exit(130)); path=sys.argv[1]; open(path + \".tmp\", \"w\").write(str(os.getpid())); os.rename(path + \".tmp\", path); signal.pause()' \"$SIGNAL_READY\"\n\telse\n\t\tsleep 0.15\n\tfi\n\tprintf 'sleep-status=%s\\n' \"$?\"",
                1,
            );
        } else if case == "sigint-2.sh" {
            source = source.replacen(
                "\tsleep 5",
                "\tif (( i == 1 )); then\n\t\tpython3 -c 'import os, signal, sys; signal.signal(signal.SIGINT, lambda *_: os._exit(130)); path=sys.argv[1]; open(path + \".tmp\", \"w\").write(str(os.getpid())); os.rename(path + \".tmp\", path); signal.pause()' \"$SIGNAL_READY\"\n\telse\n\t\tsleep 0.15\n\tfi",
                1,
            );
        } else {
            source = source.replace("sleep 5 &", "sleep 2 &");
            source = source.replacen(
                "echo wait 1\nwait",
                "echo wait 1\n: > \"$SIGNAL_READY\"\nwait",
                1,
            );
        }
        let bash = run_with_ready_sigint(&oracle_bash_path(), case, &source);
        let cherub = run_with_ready_sigint(&cherub_path().expect("cherub binary"), case, &source);
        let outcome = diff(&bash, &cherub);
        assert!(
            outcome.is_match(),
            "case={case}: {outcome:?}; bash={bash:?}; cherub={cherub:?}"
        );
    }
}

#[test]
fn sigint_reaches_every_foreground_pipeline_stage() {
    let source = r#"
trap 'echo caught sigint' INT
python3 -c 'import os, signal, sys; signal.signal(signal.SIGINT, lambda *_: os._exit(130)); path=sys.argv[1]; open(path + ".tmp", "w").close(); os.rename(path + ".tmp", path); signal.pause()' "$SIGNAL_READY.left" |
    python3 -c 'import os, signal, sys; signal.signal(signal.SIGINT, lambda *_: os._exit(130)); path=sys.argv[1]; open(path + ".tmp", "w").close(); os.rename(path + ".tmp", path); signal.pause()' "$SIGNAL_READY.right"
printf 'pipeline-status=%s\n' "${PIPESTATUS[*]}"
"#;
    let bash = run_with_ready_sigint(&oracle_bash_path(), "sigint-pipeline", source);
    let cherub = run_with_ready_sigint(
        &cherub_path().expect("cherub binary"),
        "sigint-pipeline",
        source,
    );
    let outcome = diff(&bash, &cherub);

    assert!(
        outcome.is_match(),
        "{outcome:?}; bash={bash:?}; cherub={cherub:?}"
    );
    assert!(cherub.stdout.contains("pipeline-status=130 130"));
}

fn manifest_rows(manifest: &str) -> Vec<(String, String, String)> {
    manifest
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            assert_eq!(fields.len(), 3, "invalid misc manifest row: {line}");
            (
                fields[0].to_string(),
                fields[1].to_string(),
                fields[2].to_string(),
            )
        })
        .collect()
}

fn run_script_in_isolated_directory(shell: &Path, case: &str, stdin: Option<&str>) -> RunOutput {
    let directory = temporary_directory(case);
    let directory_text = directory.to_string_lossy().into_owned();
    let case_path = misc_dir().join(case).to_string_lossy().into_owned();
    let script = "cd \"$MISC_TEST_DIR\" && . \"$MISC_CASE\"";
    let output = run_shell_spec(
        shell,
        &RunSpec {
            script: Some(script),
            stdin,
            env: vec![
                ("MISC_TEST_DIR", &directory_text),
                ("MISC_CASE", &case_path),
            ],
            ..RunSpec::default()
        },
    )
    .unwrap_or_else(|error| panic!("run {case}: {error}"));
    fs::remove_dir_all(&directory).expect("remove misc test directory");
    output
}

fn run_with_ready_sigint(shell: &Path, case: &str, source: &str) -> RunOutput {
    let directory = temporary_directory(case);
    let ready = directory.join("ready");
    let stdout_path = directory.join("stdout");
    let stderr_path = directory.join("stderr");
    let mut command = Command::new(shell);
    command
        .args(["--noprofile", "--norc", "-c", source])
        .env("SIGNAL_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            File::create(&stdout_path).expect("create stdout"),
        ))
        .stderr(Stdio::from(
            File::create(&stderr_path).expect("create stderr"),
        ))
        .process_group(0);
    let mut child = command.spawn().expect("run signal script");
    let ready_deadline = Instant::now() + Duration::from_secs(3);
    let ready_paths = if case == "sigint-pipeline" {
        vec![ready.with_extension("left"), ready.with_extension("right")]
    } else {
        vec![ready.clone()]
    };
    while !ready_paths.iter().all(|path| path.exists()) {
        if let Some(status) = child.try_wait().expect("poll signal script") {
            fs::remove_dir_all(&directory).expect("remove signal directory");
            panic!(
                "{} {case} exited before its signal point: {status}",
                shell.display()
            );
        }
        if Instant::now() >= ready_deadline {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            fs::remove_dir_all(&directory).expect("remove signal directory");
            panic!("{} {case} did not become ready", shell.display());
        }
        thread::sleep(Duration::from_millis(5));
    }

    if matches!(case, "sigint-3.sh" | "sigint-4.sh") {
        let wait_path = format!("/proc/{}/wchan", child.id());
        loop {
            let blocked_in_wait = fs::read_to_string(&wait_path)
                .map(|wchan| wchan.contains("wait"))
                .unwrap_or(false);
            if blocked_in_wait {
                break;
            }
            if let Some(status) = child.try_wait().expect("poll waiting signal script") {
                fs::remove_dir_all(&directory).expect("remove signal directory");
                panic!(
                    "{} {case} exited before blocking in wait: {status}",
                    shell.display()
                );
            }
            if Instant::now() >= ready_deadline {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                fs::remove_dir_all(&directory).expect("remove signal directory");
                panic!("{} {case} did not block in wait", shell.display());
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    let signal_target = if case == "sigint-2.sh" {
        let target_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(target) = fs::read_to_string(&ready)
                .map(|value| value.trim().parse::<i32>())
                .and_then(|result| result.map_err(std::io::Error::other))
            {
                break target;
            }
            if Instant::now() >= target_deadline {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                fs::remove_dir_all(&directory).expect("remove signal directory");
                panic!(
                    "{} {case} published an invalid signal target",
                    shell.display()
                );
            }
            thread::sleep(Duration::from_millis(1));
        }
    } else {
        -(child.id() as i32)
    };
    let signal_result = unsafe { libc::kill(signal_target, libc::SIGINT) };
    if signal_result != 0 {
        let error = std::io::Error::last_os_error();
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
        fs::remove_dir_all(&directory).expect("remove signal directory");
        panic!("failed to signal {} {case}: {error}", shell.display());
    }
    let exit_deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll interrupted script") {
            break status;
        }
        if Instant::now() >= exit_deadline {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            fs::remove_dir_all(&directory).expect("remove signal directory");
            panic!("{} {case} did not exit after SIGINT", shell.display());
        }
        thread::sleep(Duration::from_millis(5));
    };
    let output = RunOutput {
        status: status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap()),
        stdout: fs::read_to_string(&stdout_path).expect("read stdout"),
        stderr: fs::read_to_string(&stderr_path).expect("read stderr"),
    };
    fs::remove_dir_all(&directory).expect("remove signal directory");
    output
}

fn misc_dir() -> PathBuf {
    workspace_root().join("vendor/bash-5.3.15/tests/misc")
}

fn temporary_directory(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "cherubsh-misc-{}-{label}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create misc test directory");
    path
}
