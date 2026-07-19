//! Job-control smoke tests (non-PTY).

use cherubsh_test_harness::{assert_parity, run_cherub, RunSpec};

#[test]
fn jobs_empty_in_noninteractive() {
    let out = run_cherub(&RunSpec {
        script: Some("jobs"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(out.stdout.is_empty(), "stdout={:?}", out.stdout);
    assert_eq!(out.status, 0);
}

#[test]
fn kill_l_lists_signal_name() {
    let out = run_cherub(&RunSpec {
        script: Some("kill -l INT"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(out.stdout.trim().contains('2'), "stdout={:?}", out.stdout);
}

#[test]
fn kill_l_matches_trap_l_and_status_offsets() {
    assert_parity(&RunSpec {
        script: Some(
            "sigone=$(kill -l | sed -n 's:^ 1) *\\([^ \t]*\\)[ \t].*$:\\1:p'); \
             kill -l 1; kill -l 129; cmp -s <(kill -l) <(trap -l); echo cmp:$?; \
             kill -l ${sigone/SIG/}",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn wait_no_children_exits_zero() {
    let out = run_cherub(&RunSpec {
        script: Some("wait"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert_eq!(out.status, 0);
}

#[test]
fn wait_n_p_reports_completed_child() {
    assert_parity(&RunSpec {
        script: Some(
            "sleep 0.02 & a=$!; sleep 0.08 & b=$!; wait -n -p who \"$a\" \"$b\"; st=$?; case \"$who\" in \"$a\"|\"$b\") echo id;; *) echo missing:$who;; esac; echo status:$st; wait",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn wait_n_consumes_completed_jobs_separately() {
    assert_parity(&RunSpec {
        script: Some(
            "{ sleep 0.01; exit 0; } & { sleep 0.08; exit 5; } & wait -n; echo first:$?; wait -n; echo second:$?",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn wait_n_reports_each_immediate_child_once() {
    let out = run_cherub(&RunSpec {
        script: Some("(exit 0) & (exit 5) & wait -n; echo first:$?; wait -n; echo second:$?"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    let mut statuses = out
        .stdout
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(_, status)| status.parse::<i32>().expect("numeric wait status"))
        .collect::<Vec<_>>();
    statuses.sort_unstable();
    assert_eq!(statuses, [0, 5], "stdout={:?}", out.stdout);
}

#[test]
fn jobs_x_substitutes_jobspec_with_process_group() {
    let out = run_cherub(&RunSpec {
        script: Some("sleep 0.05 & jobs -x test %1 -gt 0; echo status:$?; wait"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert_eq!(out.stdout.trim(), "status:0", "stdout={:?}", out.stdout);
    assert_eq!(out.status, 0);
}
