//! Vendored brush compatibility test sweep.
//!
//! Runs `vendor/brush/brush-shell/tests/cases/compat` against CherubSH and the
//! Bash 5.2.21 oracle. Gated on `RUN_BRUSH_PARITY=1` because the corpus has
//! thousands of shell invocations.

use std::fs;

use cherubsh_test_harness::brush::{default_brush_cases_dir, discover_brush_cases, run_brush_case};
use cherubsh_test_harness::{oracle_available, oracle_bash_path, workspace_root};

#[test]
fn brush_compat_parity_all() {
    if std::env::var_os("RUN_BRUSH_PARITY").is_none() {
        eprintln!("skip: set RUN_BRUSH_PARITY=1 to run vendored brush compat cases");
        return;
    }

    if !oracle_available() {
        panic!(
            "bash-5.2.21 oracle not available at {}; run oracle/build-bash-5.2.21.sh first",
            oracle_bash_path().display()
        );
    }

    let cases_dir = std::env::var("BRUSH_COMPAT_CASES_DIR")
        .map(Into::into)
        .unwrap_or_else(|_| default_brush_cases_dir());
    let mut cases = discover_brush_cases(&cases_dir).unwrap_or_else(|err| {
        panic!(
            "discover brush compat cases under {}: {err}",
            cases_dir.display()
        );
    });
    if let Ok(filter) = std::env::var("BRUSH_PARITY_FILTER") {
        cases.retain(|case| case.id().contains(&filter));
    }
    assert!(
        !cases.is_empty(),
        "no brush compat cases discovered under {}",
        cases_dir.display()
    );

    let report_dir = std::env::var("BRUSH_PARITY_REPORT_DIR")
        .map(Into::into)
        .unwrap_or_else(|_| workspace_root().join("target/parity/brush"));
    fs::create_dir_all(&report_dir).expect("create brush parity report dir");
    let report_path = report_dir.join("report.tsv");
    let mut report = String::from("verdict\tcase\treason\n");

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    let mut timeout = 0u32;
    let mut failure_log = String::new();

    for case in &cases {
        let outcome = run_brush_case(case, &report_dir)
            .unwrap_or_else(|err| panic!("run brush case {}: {err}", case.id()));
        if outcome.skipped {
            skip += 1;
            let reason = outcome.skip_reason.as_deref().unwrap_or("");
            eprintln!("brush skip {} ({reason})", outcome.id);
            report.push_str(&format!("SKIP\t{}\t{}\n", outcome.id, reason));
            continue;
        }
        if outcome.passed {
            pass += 1;
            eprintln!("brush PASS {}", outcome.id);
            let reason = if outcome.known_failure {
                "brush marked known_failure, but CherubSH matched Bash"
            } else {
                ""
            };
            report.push_str(&format!("PASS\t{}\t{}\n", outcome.id, reason));
            continue;
        }

        fail += 1;
        if outcome.timed_out {
            timeout += 1;
        }
        let reason = if outcome.timed_out && outcome.known_failure {
            "timeout; brush metadata marks this known_failure"
        } else if outcome.timed_out {
            "timeout"
        } else if outcome.known_failure {
            "unexpected difference; brush metadata marks this known_failure"
        } else {
            "unexpected difference"
        };
        eprintln!("brush FAIL {} ({reason})", outcome.id);
        report.push_str(&format!("FAIL\t{}\t{}\n", outcome.id, reason));
        failure_log.push_str(&format!(
            "FAIL {}\nstatus: {}\nstdout:\n{}\nstderr:\n{}\ndir:\n{}\nexpectations:\n{}\n",
            outcome.id,
            outcome.status_diff,
            outcome.stdout_diff,
            outcome.stderr_diff,
            outcome.dir_diff,
            outcome.expectation_diff,
        ));
    }

    fs::write(&report_path, report).expect("write brush parity report");
    eprintln!(
        "\nbrush parity: PASS={pass} FAIL={fail} TIMEOUT={timeout} SKIP={skip} (total {})",
        cases.len()
    );
    eprintln!("brush parity report: {}", report_path.display());

    assert!(
        fail == 0,
        "brush compatibility parity has unexpected outcomes (FAIL={fail} TIMEOUT={timeout}):\n{failure_log}"
    );
}
