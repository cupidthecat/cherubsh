//! Differential fixture tests against the selected Bash oracle.
//!
//! Set `RUN_PARITY_TESTS=1` to run the full 99-case sweep.

use std::path::PathBuf;

use cherubsh_test_harness::upstream::load_xfail;
use cherubsh_test_harness::{
    cherub_path, diff, discover_fixtures, load_fixture, oracle_available, oracle_bash_path,
    oracle_version, run_cherub, run_shell_spec, workspace_root, RunSpec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail,
    XFail,
    XPass,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .join("test-harness/tests/fixtures")
}

#[test]
fn parity_all_fixtures() {
    if std::env::var_os("RUN_PARITY_TESTS").is_none() {
        eprintln!("skip: set RUN_PARITY_TESTS=1 to enforce bash parity");
        return;
    }

    if !oracle_available() {
        panic!(
            "bash {} oracle is not available at {}",
            oracle_version(),
            oracle_bash_path().display()
        );
    }
    let bash_path = oracle_bash_path();

    let _ = cherub_path().expect("CARGO_BIN_EXE_cherubsh");
    let dir = fixtures_dir();
    let names = discover_fixtures(&dir).expect("discover fixtures");
    assert!(!names.is_empty(), "no fixtures found in {}", dir.display());

    let xfail_path = workspace_root().join("crates/test-harness/fixtures-xfail.txt");
    let xfail = load_xfail(&xfail_path);

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut xfail_count = 0u32;
    let mut xpass = 0u32;
    let mut failure_log = String::new();

    for name in &names {
        let fixture = load_fixture(&dir, name).unwrap_or_else(|err| {
            panic!("load fixture {name}: {err}");
        });
        let spec = RunSpec {
            args: fixture.args.iter().map(|s| s.as_str()).collect(),
            stdin: fixture.stdin.as_deref(),
            env: fixture
                .env
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            env_remove: Vec::new(),
            script: Some(fixture.script.as_str()),
        };
        let bash_output = run_shell_spec(&bash_path, &spec).expect("run bash");
        let cherub_output = run_cherub(&spec).expect("run cherubsh");
        let outcome = diff(&bash_output, &cherub_output);
        let in_xfail = xfail.contains(name);
        let verdict = match (outcome.is_match(), in_xfail) {
            (true, false) => Verdict::Pass,
            (true, true) => Verdict::XPass,
            (false, false) => Verdict::Fail,
            (false, true) => Verdict::XFail,
        };
        match verdict {
            Verdict::Pass => pass += 1,
            Verdict::XFail => xfail_count += 1,
            Verdict::Fail => {
                fail += 1;
                failure_log.push_str(&format!(
                    "FAIL {name}: status: bash={} cherub={}\n  stdout diff:\n{}\n  stderr diff:\n{}\n",
                    bash_output.status, cherub_output.status,
                    outcome.stdout_diff, outcome.stderr_diff,
                ));
            }
            Verdict::XPass => {
                xpass += 1;
                failure_log.push_str(&format!(
                    "XPASS {name}: passing now; remove from fixtures-xfail.txt\n"
                ));
            }
        }
        eprintln!("{:>5} {name}", label(verdict));
    }

    eprintln!(
        "\nfixture parity: PASS={pass} FAIL={fail} XFAIL={xfail_count} XPASS={xpass} (total {})",
        names.len()
    );

    assert!(
        fail == 0 && xpass == 0,
        "fixture parity has unexpected outcomes (FAIL={fail} XPASS={xpass}):\n{failure_log}"
    );
}

fn label(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
        Verdict::XFail => "xfail",
        Verdict::XPass => "XPASS",
    }
}
