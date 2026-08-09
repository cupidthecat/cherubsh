use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use cherubsh_test_harness::oils::{run_oils_case_with_shells, validate_oils_sandbox, OilsCase};
use cherubsh_test_harness::workspace_root;

fn case(code: &str) -> OilsCase {
    OilsCase {
        source_file: PathBuf::from("runner.test.sh"),
        case_index: 0,
        line_number: 1,
        description: "runner fixture".to_string(),
        code: code.to_string(),
        tags: Vec::new(),
        legacy_tmp_dir: false,
    }
}

fn fixture_spec_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../vendor/oils/spec"
    ))
}

fn sandbox_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let bwrap = std::env::var_os("BWRAP").unwrap_or_else(|| "bwrap".into());
        Command::new(bwrap)
            .args(["--unshare-pid", "--ro-bind", "/", "/", "--", "/bin/true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn skip_without_sandbox() -> bool {
    if sandbox_available() {
        return false;
    }
    eprintln!("Oils sandbox test skipped: Bubblewrap process namespace unavailable");
    true
}

#[test]
fn sandbox_preflight_accepts_a_working_shell() {
    if skip_without_sandbox() {
        return;
    }
    validate_oils_sandbox(
        Path::new("/bin/bash"),
        fixture_spec_dir(),
        Duration::from_secs(3),
    )
    .expect("validate working Oils sandbox");
}

#[test]
fn sandbox_preflight_rejects_a_command_that_never_runs_the_probe() {
    if skip_without_sandbox() {
        return;
    }
    let error = validate_oils_sandbox(
        Path::new("/bin/false"),
        fixture_spec_dir(),
        Duration::from_secs(3),
    )
    .expect_err("reject broken Oils sandbox");

    assert!(error.to_string().contains("Oils sandbox preflight failed"));
}

#[test]
fn sandbox_preserves_raw_bytes_and_uses_identical_logical_paths() {
    if skip_without_sandbox() {
        return;
    }
    let fixture = case(
        "printf '%s\\n' \"$SH|$HOME|$TMP|$PWD|$$|$0\"\n\
         python3 -c 'import sys; sys.stdout.buffer.write(b\"\\xff\")'\n",
    );

    let outcome = run_oils_case_with_shells(
        &fixture,
        Path::new("/bin/bash"),
        Path::new("/bin/bash"),
        fixture_spec_dir(),
        Duration::from_secs(3),
    )
    .expect("run sandboxed case");

    assert!(outcome.passed());
    assert_eq!(outcome.bash, outcome.cherub);
    assert!(outcome.bash.stdout.ends_with(b"\xff"));
    let text = String::from_utf8_lossy(&outcome.bash.stdout);
    assert!(text.starts_with("bash|/tmp/home|/tmp/work|/tmp/work|2|/tmp/.cherub-bin/bash\n"));
}

#[test]
fn sandbox_reports_exact_output_dimensions() {
    if skip_without_sandbox() {
        return;
    }
    let fixture = case(
        "printf '%s\\n' \"${BASH_VERSION-unset}\"\n\
         test -n \"${BASH_VERSION-}\"\n",
    );

    let outcome = run_oils_case_with_shells(
        &fixture,
        Path::new("/bin/bash"),
        Path::new("/bin/dash"),
        fixture_spec_dir(),
        Duration::from_secs(3),
    )
    .expect("run differing shells");

    assert!(!outcome.passed());
    assert_eq!(outcome.differing_fields(), ["status", "stdout"]);
}

#[test]
fn sandbox_timeout_is_never_a_pass_even_when_both_shells_timeout() {
    if skip_without_sandbox() {
        return;
    }
    let fixture = case("sleep 5 & wait\n");

    let outcome = run_oils_case_with_shells(
        &fixture,
        Path::new("/bin/bash"),
        Path::new("/bin/bash"),
        fixture_spec_dir(),
        Duration::from_millis(100),
    )
    .expect("run timed case");

    assert!(outcome.bash.timed_out);
    assert!(outcome.cherub.timed_out);
    assert!(!outcome.passed());
    assert!(outcome.differing_fields().is_empty());
}

#[test]
fn sandbox_does_not_expose_host_workspace_files() {
    if skip_without_sandbox() {
        return;
    }
    let sentinel = workspace_root().join("target/oils-host-secret");
    std::fs::create_dir_all(sentinel.parent().expect("sentinel parent"))
        .expect("create sentinel parent");
    std::fs::write(&sentinel, b"private\n").expect("write host sentinel");
    let fixture = case(&format!(
        "if test -r '{}'; then echo exposed; else echo hidden; fi\n",
        sentinel.display()
    ));

    let outcome = run_oils_case_with_shells(
        &fixture,
        Path::new("/bin/bash"),
        Path::new("/bin/bash"),
        fixture_spec_dir(),
        Duration::from_secs(3),
    )
    .expect("run isolated case");
    std::fs::remove_file(&sentinel).expect("remove host sentinel");

    assert_eq!(outcome.bash.stdout, b"hidden\n");
}

#[test]
fn timeout_includes_blocked_stdin_delivery() {
    if skip_without_sandbox() {
        return;
    }
    let mut code = String::from("sleep 1\n");
    for _ in 0..100_000 {
        code.push_str("# padding that fills the stdin pipe\n");
    }
    let fixture = case(&code);
    let started = std::time::Instant::now();

    let outcome = run_oils_case_with_shells(
        &fixture,
        Path::new("/bin/bash"),
        Path::new("/bin/bash"),
        fixture_spec_dir(),
        Duration::from_millis(100),
    )
    .expect("time stdin delivery");

    assert!(outcome.bash.timed_out);
    assert!(outcome.cherub.timed_out);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "stdin write escaped the timeout: {:?}",
        started.elapsed()
    );
}
