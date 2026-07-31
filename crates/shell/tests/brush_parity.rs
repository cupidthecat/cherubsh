//! Runs the vendored Brush compatibility cases against Bash and CherubSH.
//!
//! Set `RUN_BRUSH_PARITY=1` to include this slower suite.

use std::fs;

use cherubsh_test_harness::brush::{
    default_brush_cases_dir, discover_brush_cases, ported_brush_case_ids, run_brush_case,
};
use cherubsh_test_harness::{oracle_available, oracle_bash_path, oracle_version, workspace_root};

#[test]
fn ported_job_and_coproc_case_ids_match_the_upstream_skip_set() {
    let cases = discover_brush_cases(&default_brush_cases_dir()).expect("discover Brush cases");
    let manifest = ported_brush_case_ids().expect("read ported Brush case manifest");
    let upstream_skips = cases
        .iter()
        .filter(|case| {
            case.marked_skip()
                && matches!(
                    case.set_name.as_str(),
                    "Background jobs" | "Compound commands: coproc"
                )
        })
        .map(|case| case.id())
        .collect();

    assert_eq!(manifest, upstream_skips);
    assert_eq!(manifest.len(), 14);
    for id in manifest {
        let case = cases
            .iter()
            .find(|case| case.id() == id)
            .unwrap_or_else(|| panic!("missing ported Brush case: {id}"));
        assert!(case.ported(), "manifest case is not enabled: {id}");
    }
}

#[test]
fn brush_compat_parity_all() {
    if std::env::var_os("RUN_BRUSH_PARITY").is_none() {
        eprintln!("skip: set RUN_BRUSH_PARITY=1 to run vendored brush compat cases");
        return;
    }

    if !oracle_available() {
        panic!(
            "bash {} oracle is not available at {}",
            oracle_version(),
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
    let mut report = String::from("verdict\tclassification\tcase\treason\n");

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
            report.push_str(&format!(
                "SKIP\tupstream-skip\t{}\t{}\n",
                outcome.id, reason
            ));
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
            let classification = if outcome.ported { "ported" } else { "baseline" };
            report.push_str(&format!(
                "PASS\t{classification}\t{}\t{}\n",
                outcome.id, reason
            ));
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
        let classification = if outcome.ported { "ported" } else { "baseline" };
        report.push_str(&format!(
            "FAIL\t{classification}\t{}\t{}\n",
            outcome.id, reason
        ));
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
