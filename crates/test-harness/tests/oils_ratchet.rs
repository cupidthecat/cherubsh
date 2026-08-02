use std::path::PathBuf;

use cherubsh_test_harness::oils::{
    assess_oils_outcomes, candidate_fingerprint, classify_oils_outcome, load_oils_ratchet,
    write_oils_report, OilsKnownMismatch, OilsOutcome, OilsRunOutput, OilsVerdict,
};
use cherubsh_test_harness::workspace_root;

fn output(status: i32, stdout: &[u8], stderr: &[u8]) -> OilsRunOutput {
    OilsRunOutput {
        status,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
        timed_out: false,
    }
}

fn mismatch() -> OilsOutcome {
    OilsOutcome {
        id: "sample.test.sh::000::sample".to_string(),
        bash: output(0, b"bash\n", b""),
        cherub: output(1, b"cherub\xff\n", b""),
    }
}

#[test]
fn fingerprint_is_stable_and_byte_sensitive() {
    let first = candidate_fingerprint(&mismatch().cherub);
    let second = candidate_fingerprint(&mismatch().cherub);
    let changed = candidate_fingerprint(&output(1, b"cherub\xfe\n", b""));

    assert_eq!(first, second);
    assert_eq!(first.len(), 64);
    assert_ne!(first, changed);
}

#[test]
fn ratchet_classifies_known_drift_new_and_unexpected_pass() {
    let outcome = mismatch();
    let known = OilsKnownMismatch {
        id: outcome.id.clone(),
        fields: vec!["status".to_string(), "stdout".to_string()],
        oracle_fingerprint: candidate_fingerprint(&outcome.bash),
        candidate_fingerprint: candidate_fingerprint(&outcome.cherub),
    };

    assert_eq!(
        classify_oils_outcome(&outcome, None),
        OilsVerdict::NewFailure
    );
    assert_eq!(
        classify_oils_outcome(&outcome, Some(&known)),
        OilsVerdict::Known
    );

    let mut changed = outcome.clone();
    changed.cherub.stdout.push(b'!');
    assert_eq!(
        classify_oils_outcome(&changed, Some(&known)),
        OilsVerdict::Drift
    );

    let mut changed_oracle = outcome.clone();
    changed_oracle.bash.stdout.push(b'!');
    assert_eq!(
        classify_oils_outcome(&changed_oracle, Some(&known)),
        OilsVerdict::Drift
    );

    let mut variable_oracle = known.clone();
    variable_oracle.oracle_fingerprint = "variable".to_string();
    assert_eq!(
        classify_oils_outcome(&changed_oracle, Some(&variable_oracle)),
        OilsVerdict::Known
    );

    let passing = OilsOutcome {
        id: outcome.id,
        bash: output(0, b"same", b""),
        cherub: output(0, b"same", b""),
    };
    assert_eq!(classify_oils_outcome(&passing, None), OilsVerdict::Pass);
    assert_eq!(
        classify_oils_outcome(&passing, Some(&known)),
        OilsVerdict::UnexpectedPass
    );
}

#[test]
fn timeout_fields_are_part_of_the_ratchet_contract() {
    let mut outcome = mismatch();
    outcome.bash.timed_out = true;
    outcome.cherub.timed_out = true;

    assert_eq!(
        outcome.observed_fields(),
        ["bash-timeout", "cherub-timeout", "status", "stdout"]
    );
}

#[test]
fn ratchet_loader_rejects_duplicate_case_ids() {
    let path = ratchet_fixture_path("duplicate");
    let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let row = format!("sample.test.sh::000::sample\tstatus,stdout\t{fingerprint}\t{fingerprint}\n");
    std::fs::write(
        &path,
        format!("case\tfields\toracle_sha256\tcandidate_sha256\n{row}{row}"),
    )
    .expect("write ratchet fixture");

    let error = load_oils_ratchet(&path).expect_err("duplicate IDs must fail");
    std::fs::remove_file(&path).expect("remove ratchet fixture");

    assert!(error.to_string().contains("duplicate Oils ratchet case"));
}

#[test]
fn ratchet_loader_rejects_unlisted_variable_oracles() {
    let path = ratchet_fixture_path("variable");
    let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    std::fs::write(
        &path,
        format!(
            "case\tfields\toracle_sha256\tcandidate_sha256\n\
             sample.test.sh::000::sample\tstdout\tvariable\t{fingerprint}\n"
        ),
    )
    .expect("write variable oracle fixture");

    let error = load_oils_ratchet(&path).expect_err("unlisted variable oracle must fail");
    std::fs::remove_file(&path).expect("remove variable oracle fixture");

    assert!(error
        .to_string()
        .contains("not in the nondeterministic manifest"));
}

fn ratchet_fixture_path(name: &str) -> PathBuf {
    let path = workspace_root().join(format!("target/oils-ratchet-{name}-fixture.tsv"));
    std::fs::create_dir_all(path.parent().expect("ratchet fixture parent"))
        .expect("create ratchet fixture parent");
    path
}

#[test]
fn assessment_is_sorted_and_reports_stale_entries() {
    let outcome = mismatch();
    let stale = OilsKnownMismatch {
        id: "z.test.sh::000::stale".to_string(),
        fields: vec!["status".to_string()],
        oracle_fingerprint: "0".repeat(64),
        candidate_fingerprint: "0".repeat(64),
    };
    let mut known = std::collections::BTreeMap::new();
    known.insert(stale.id.clone(), stale);

    let assessments = assess_oils_outcomes(vec![outcome], &known);

    assert_eq!(assessments.len(), 2);
    assert_eq!(assessments[0].id, "sample.test.sh::000::sample");
    assert_eq!(assessments[0].verdict, OilsVerdict::NewFailure);
    assert_eq!(assessments[1].id, "z.test.sh::000::stale");
    assert_eq!(assessments[1].verdict.as_str(), "STALE");
}

#[test]
fn report_contains_tally_artifacts_and_replacement_ratchet() {
    let report_dir = workspace_root().join("target/oils-report-fixture");
    if report_dir.exists() {
        std::fs::remove_dir_all(&report_dir).expect("remove old report fixture");
    }
    let outcome = mismatch();
    let assessments = assess_oils_outcomes(vec![outcome.clone()], &Default::default());

    let tally = write_oils_report(&report_dir, &assessments).expect("write Oils report");

    assert_eq!(tally.fail, 1);
    let report = std::fs::read_to_string(report_dir.join("report.tsv")).expect("read report");
    assert!(report.contains("verdict\tcase\tfields\toracle_sha256\tcandidate_sha256"));
    assert!(report.contains("FAIL\tsample.test.sh::000::sample\tstatus,stdout"));
    let suggested = std::fs::read_to_string(report_dir.join("suggested-ratchet.tsv"))
        .expect("read suggested ratchet");
    assert!(suggested.contains(&candidate_fingerprint(&outcome.bash)));
    assert!(suggested.contains(&candidate_fingerprint(&outcome.cherub)));
    assert_eq!(
        std::fs::read(report_dir.join("failures/0000/bash.stdout")).expect("read Bash stdout"),
        b"bash\n"
    );
    assert_eq!(
        std::fs::read(report_dir.join("failures/0000/cherub.stdout"))
            .expect("read CherubSH stdout"),
        b"cherub\xff\n"
    );
    std::fs::remove_dir_all(report_dir).expect("remove report fixture");
}

#[test]
fn report_and_loader_round_trip_case_ids_with_escapes() {
    let report_dir = workspace_root().join("target/oils-escape-fixture");
    let mut outcome = mismatch();
    outcome.id = "escape.test.sh::000::backslash \\ tab\t newline\n".to_string();
    let assessments = assess_oils_outcomes(vec![outcome.clone()], &Default::default());
    write_oils_report(&report_dir, &assessments).expect("write escaped Oils report");

    let loaded = load_oils_ratchet(&report_dir.join("suggested-ratchet.tsv"))
        .expect("load escaped Oils report");

    assert!(loaded.contains_key(&outcome.id));
    std::fs::remove_dir_all(report_dir).expect("remove escaped report fixture");
}
