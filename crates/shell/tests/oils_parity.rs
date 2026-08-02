use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use cherubsh_test_harness::oils::{
    assess_oils_outcomes, default_oils_ratchet_path, default_oils_spec_dir, discover_oils_cases,
    load_oils_ratchet, oils_nondeterministic_case_ids, run_oils_case_with_shells,
    validate_oils_sandbox, write_oils_report, OilsCase, OilsKnownMismatch, OilsOutcome,
    OilsVerdict,
};
use cherubsh_test_harness::{cherub_path, required_oracle_bash_path, workspace_root, HarnessError};

#[test]
fn oils_osh_parity_all() {
    if std::env::var_os("RUN_OILS_PARITY").is_none() {
        eprintln!("skip: set RUN_OILS_PARITY=1 to run vendored Oils OSH cases");
        return;
    }

    let spec_dir = std::env::var_os("OILS_SPEC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_oils_spec_dir);
    let mut cases = discover_oils_cases(&spec_dir).unwrap_or_else(|error| {
        panic!("discover Oils cases under {}: {error}", spec_dir.display())
    });
    let filter = std::env::var("OILS_PARITY_FILTER").ok();
    if let Some(filter) = &filter {
        cases.retain(|case| case.id().contains(filter));
    }
    assert!(
        !cases.is_empty(),
        "no Oils cases matched under {}",
        spec_dir.display()
    );

    let known = load_oils_ratchet(&default_oils_ratchet_path()).expect("load Oils ratchet");
    let selected_ids = cases.iter().map(|case| case.id()).collect::<BTreeSet<_>>();
    let selected_known = known
        .into_iter()
        .filter(|(id, _)| filter.is_none() || selected_ids.contains(id))
        .collect::<BTreeMap<String, OilsKnownMismatch>>();
    let jobs = std::env::var("OILS_PARITY_JOBS")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .expect("OILS_PARITY_JOBS must be an integer")
        })
        .unwrap_or_else(|| {
            thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .min(8)
        });
    assert!(jobs > 0, "OILS_PARITY_JOBS must be positive");
    let timeout = std::env::var("OILS_PARITY_TIMEOUT_SECS")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .expect("OILS_PARITY_TIMEOUT_SECS must be an integer")
        })
        .unwrap_or(15);
    assert!(timeout > 0, "OILS_PARITY_TIMEOUT_SECS must be positive");

    let bash = required_oracle_bash_path().expect("resolve pinned Bash oracle");
    let cherub = cherub_path().expect("resolve CherubSH test binary");
    validate_oils_sandbox(&bash, &spec_dir, Duration::from_secs(timeout))
        .unwrap_or_else(|error| panic!("validate Oils sandbox: {error}"));
    eprintln!(
        "Oils parity: {} cases, {} workers, {}s timeout",
        cases.len(),
        jobs.min(cases.len()),
        timeout
    );
    let mut outcomes = run_parallel(
        cases.clone(),
        bash.clone(),
        cherub.clone(),
        spec_dir.clone(),
        Duration::from_secs(timeout),
        jobs,
    )
    .unwrap_or_else(|error| panic!("run Oils parity: {error}"));
    stabilize_nondeterministic_cases(
        &cases,
        &mut outcomes,
        &selected_known,
        &bash,
        &cherub,
        &spec_dir,
        Duration::from_secs(timeout),
    )
    .unwrap_or_else(|error| panic!("stabilize Oils parity cases: {error}"));
    let assessments = assess_oils_outcomes(outcomes, &selected_known);
    let report_dir = std::env::var_os("OILS_PARITY_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target/parity/oils"));
    let tally = write_oils_report(&report_dir, &assessments).expect("write Oils parity report");

    eprintln!(
        "Oils parity: PASS={} KNOWN={} FAIL={} DRIFT={} XPASS={} STALE={}",
        tally.pass, tally.known, tally.fail, tally.drift, tally.xpass, tally.stale
    );
    eprintln!(
        "Oils parity report: {}",
        report_dir.join("report.tsv").display()
    );
    assert!(
        !tally.has_regressions(),
        "Oils parity has unexpected outcomes: FAIL={} DRIFT={} XPASS={} STALE={}; see {}",
        tally.fail,
        tally.drift,
        tally.xpass,
        tally.stale,
        report_dir.display()
    );
}

fn run_parallel(
    cases: Vec<OilsCase>,
    bash: PathBuf,
    cherub: PathBuf,
    spec_dir: PathBuf,
    timeout: Duration,
    jobs: usize,
) -> Result<Vec<OilsOutcome>, HarnessError> {
    let cases = Arc::new(cases);
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let worker_count = jobs.min(cases.len());
    thread::scope(|scope| {
        for _ in 0..worker_count {
            let cases = Arc::clone(&cases);
            let next = Arc::clone(&next);
            let sender = sender.clone();
            let bash = &bash;
            let cherub = &cherub;
            let spec_dir = &spec_dir;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = cases.get(index) else {
                    break;
                };
                let result = run_oils_case_with_shells(case, bash, cherub, spec_dir, timeout);
                if sender.send((index, result)).is_err() {
                    break;
                }
            });
        }
        drop(sender);
        let mut outcomes = receiver.into_iter().collect::<Vec<_>>();
        outcomes.sort_by_key(|(index, _)| *index);
        outcomes.into_iter().map(|(_, outcome)| outcome).collect()
    })
}

fn stabilize_nondeterministic_cases(
    cases: &[OilsCase],
    outcomes: &mut [OilsOutcome],
    known: &BTreeMap<String, OilsKnownMismatch>,
    bash: &Path,
    cherub: &Path,
    spec_dir: &Path,
    timeout: Duration,
) -> Result<(), HarnessError> {
    let nondeterministic = oils_nondeterministic_case_ids()?;
    for (case, outcome) in cases.iter().zip(outcomes.iter_mut()) {
        if !nondeterministic.contains(&outcome.id)
            || accepted_attempt(outcome, known.get(&outcome.id))
        {
            continue;
        }
        for attempt in 2..=8 {
            let retried = run_oils_case_with_shells(case, bash, cherub, spec_dir, timeout)?;
            let accepted = accepted_attempt(&retried, known.get(&retried.id));
            *outcome = retried;
            if accepted {
                eprintln!(
                    "Oils parity: stabilized {} on attempt {attempt}",
                    outcome.id
                );
                break;
            }
        }
    }
    Ok(())
}

fn accepted_attempt(outcome: &OilsOutcome, known: Option<&OilsKnownMismatch>) -> bool {
    let verdict = cherubsh_test_harness::oils::classify_oils_outcome(outcome, known);
    if known.is_some_and(|entry| entry.candidate_fingerprint == "variable") {
        matches!(verdict, OilsVerdict::Known | OilsVerdict::Pass)
    } else if known.is_some() {
        verdict == OilsVerdict::Known
    } else {
        verdict == OilsVerdict::Pass
    }
}
