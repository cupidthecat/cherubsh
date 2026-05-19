//! bash-5.2.21 upstream test sweep.
//!
//! Wraps every vendored `bash-5.2.21/tests/run-*` script, points it at the
//! cherubsh binary, and classifies the outcome against `upstream-xfail.txt`.
//! Gated on `RUN_UPSTREAM_PARITY=1` to keep `cargo test` fast by default.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use cherubsh_test_harness::upstream::{discover_upstream_tests, load_xfail, run_upstream};
use cherubsh_test_harness::{cherub_path, oracle_available, oracle_bash_path, workspace_root};

const PER_TEST_TIMEOUT_SECS: u64 = 60;
const JOBS_TIMEOUT_SECS: u64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail,
    Timeout,
    XFail,
    XPass,
}

#[test]
fn upstream_parity_all() {
    if std::env::var_os("RUN_UPSTREAM_PARITY").is_none() {
        eprintln!("skip: set RUN_UPSTREAM_PARITY=1 to run the bash-5.2.21 upstream sweep");
        return;
    }

    if !oracle_available() {
        panic!(
            "bash-5.2.21 oracle not available at {}; run oracle/build-bash-5.2.21.sh first",
            oracle_bash_path().display()
        );
    }

    let cherub = cherub_path().expect("CARGO_BIN_EXE_cherubsh");
    let tests_dir = bash_tests_dir();
    let xfail_path = workspace_root().join("crates/test-harness/upstream-xfail.txt");
    let xfail = load_xfail(&xfail_path);

    let tests = filter_tests(discover_upstream_tests(&tests_dir));
    assert!(
        !tests.is_empty(),
        "no run-* scripts discovered under {}",
        tests_dir.display()
    );

    let default_timeout_secs = per_test_timeout_secs();
    let report_dir = report_dir();
    fs::create_dir_all(&report_dir).expect("create upstream parity report dir");
    let report_path = report_dir.join("report.tsv");
    let mut report = fs::File::create(&report_path).expect("create upstream parity report");
    writeln!(
        report,
        "verdict\tname\tstatus\ttimed_out\tstdout_path\tstderr_path\ttstout_path"
    )
    .expect("write report header");

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut timed_out = 0u32;
    let mut xfail_count = 0u32;
    let mut xpass = 0u32;
    let mut failure_log = String::new();

    for test in &tests {
        let timeout = Duration::from_secs(timeout_secs_for(&test.name, default_timeout_secs));
        let in_xfail = xfail.contains(&test.name);
        let outcome = match run_upstream(test, &cherub, timeout) {
            Ok(o) => o,
            Err(err) => {
                eprintln!("error: run {} failed: {err}", test.name);
                fail += 1;
                continue;
            }
        };

        let stdout_path = report_dir.join(format!("{}.stdout", test.name));
        let stderr_path = report_dir.join(format!("{}.stderr", test.name));
        let tstout_path = outcome.tstout_path.as_ref().map(|src| {
            let dst = report_dir.join(format!("{}.tstout", test.name));
            fs::copy(src, &dst).unwrap_or_else(|err| {
                panic!(
                    "copy upstream BASH_TSTOUT artifact {} to {}: {err}",
                    src.display(),
                    dst.display()
                )
            });
            dst
        });
        fs::write(&stdout_path, &outcome.stdout).expect("write upstream stdout artifact");
        fs::write(&stderr_path, &outcome.stderr).expect("write upstream stderr artifact");

        let verdict = match (outcome.passed, outcome.timed_out, in_xfail) {
            (true, _, false) => Verdict::Pass,
            (true, _, true) => Verdict::XPass,
            (false, true, false) => Verdict::Timeout,
            (false, _, false) => Verdict::Fail,
            (false, _, true) => Verdict::XFail,
        };

        match verdict {
            Verdict::Pass => pass += 1,
            Verdict::XFail => {
                xfail_count += 1;
            }
            Verdict::Timeout => {
                timed_out += 1;
                failure_log.push_str(&format!(
                    "TIMEOUT {}: status={} timeout={}s\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- tstout ---\n{}\n",
                    test.name,
                    outcome.status,
                    timeout.as_secs(),
                    truncate(&outcome.stdout, 4000),
                    truncate(&outcome.stderr, 4000),
                    read_artifact_excerpt(tstout_path.as_ref(), 4000),
                ));
            }
            Verdict::Fail => {
                fail += 1;
                failure_log.push_str(&format!(
                    "FAIL {}: status={} timed_out={}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- tstout ---\n{}\n",
                    test.name,
                    outcome.status,
                    outcome.timed_out,
                    truncate(&outcome.stdout, 4000),
                    truncate(&outcome.stderr, 4000),
                    read_artifact_excerpt(tstout_path.as_ref(), 4000),
                ));
            }
            Verdict::XPass => {
                xpass += 1;
                failure_log.push_str(&format!(
                    "XPASS {}: in xfail list but passed; remove it from upstream-xfail.txt\n",
                    test.name
                ));
            }
        };

        writeln!(
            report,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            label(verdict),
            test.name,
            outcome.status,
            outcome.timed_out,
            stdout_path.display(),
            stderr_path.display(),
            tstout_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        )
        .expect("write upstream parity report row");

        eprintln!("{:>5} {}", label(verdict), test.name);
    }

    eprintln!(
        "\nupstream parity: PASS={} FAIL={} TIMEOUT={} XFAIL={} XPASS={} (total {})",
        pass,
        fail,
        timed_out,
        xfail_count,
        xpass,
        tests.len()
    );
    eprintln!("upstream parity report: {}", report_path.display());

    assert!(
        fail == 0 && timed_out == 0 && xpass == 0,
        "upstream parity has unexpected outcomes (FAIL={fail} TIMEOUT={timed_out} XPASS={xpass}):\n{failure_log}"
    );
}

fn label(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
        Verdict::Timeout => "TIMEOUT",
        Verdict::XFail => "xfail",
        Verdict::XPass => "XPASS",
    }
}

fn timeout_secs_for(test_name: &str, default_timeout_secs: u64) -> u64 {
    if test_name == "jobs" {
        default_timeout_secs.max(JOBS_TIMEOUT_SECS)
    } else {
        default_timeout_secs
    }
}

fn per_test_timeout_secs() -> u64 {
    std::env::var("UPSTREAM_PARITY_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(PER_TEST_TIMEOUT_SECS)
}

fn report_dir() -> PathBuf {
    if let Ok(path) = std::env::var("UPSTREAM_PARITY_REPORT_DIR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            path
        } else {
            workspace_root().join(path)
        };
    }
    workspace_root().join("target/parity/upstream")
}

fn bash_tests_dir() -> PathBuf {
    if let Ok(path) = std::env::var("BASH_521_TESTS_DIR") {
        return PathBuf::from(path);
    }
    workspace_root().join("vendor/bash-5.2.21/tests")
}

fn filter_tests(
    tests: Vec<cherubsh_test_harness::upstream::UpstreamTest>,
) -> Vec<cherubsh_test_harness::upstream::UpstreamTest> {
    let Ok(raw) = std::env::var("UPSTREAM_PARITY_FILTER") else {
        return tests;
    };
    let wanted = raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    if wanted.is_empty() {
        return tests;
    }
    tests
        .into_iter()
        .filter(|test| wanted.contains(test.name.as_str()))
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\n... [truncated, {} bytes total]", &s[..max], s.len())
    }
}

fn read_artifact_excerpt(path: Option<&PathBuf>, max: usize) -> String {
    match path {
        Some(path) => fs::read_to_string(path)
            .map(|text| truncate(&text, max))
            .unwrap_or_else(|err| format!("<failed to read {}: {err}>", path.display())),
        None => "<none>".to_string(),
    }
}
