use std::path::{Path, PathBuf};
use std::time::Duration;

use cherubsh_test_harness::oils::{run_oils_case_with_shells, OilsCase};

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
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/oils/spec")
        .leak()
}

#[test]
fn sandbox_preserves_raw_bytes_and_uses_identical_logical_paths() {
    let fixture = case(
        "printf '%s\\n' \"$SH|$HOME|$TMP|$PWD|$$|$0\"\n\
         python3 -c 'import sys; sys.stdout.buffer.write(b\"\\xff\")'\n\
         ! grep -q eth0 /proc/net/dev && printf '|isolated\\n'\n",
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
    assert!(outcome.bash.stdout.ends_with(b"\xff|isolated\n"));
    let text = String::from_utf8_lossy(&outcome.bash.stdout);
    assert!(text.starts_with("bash|/tmp/home|/tmp/work|/tmp/work|2|/tmp/.cherub-bin/bash\n"));
}

#[test]
fn sandbox_reports_exact_output_dimensions() {
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
