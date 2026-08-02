use std::path::PathBuf;

use cherubsh_test_harness::oils::{
    assess_oils_outcomes, candidate_fingerprint, classify_oils_outcome, load_oils_ratchet,
    load_oils_ratchet_for_arch, write_oils_report, OilsKnownMismatch, OilsOutcome, OilsRunOutput,
    OilsVerdict,
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

    let mut variable_candidate = known;
    variable_candidate.oracle_fingerprint = "variable".to_string();
    variable_candidate.candidate_fingerprint = "variable".to_string();
    assert_eq!(
        classify_oils_outcome(&changed, Some(&variable_candidate)),
        OilsVerdict::Known
    );
    assert_eq!(
        classify_oils_outcome(&passing, Some(&variable_candidate)),
        OilsVerdict::Pass
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
fn ratchet_prefers_architecture_specific_overrides() {
    let path = ratchet_fixture_path("architectures");
    let generic = "0".repeat(64);
    let arm = "1".repeat(64);
    std::fs::write(
        &path,
        format!(
            "case\tarch\tfields\toracle_sha256\tcandidate_sha256\n\
             sample.test.sh::000::sample\t*\tstdout\t{generic}\t{generic}\n\
             sample.test.sh::000::sample\taarch64\tstderr\t{arm}\t{arm}\n\
             arm-only.test.sh::000::sample\taarch64\tstatus\t{arm}\t{arm}\n"
        ),
    )
    .expect("write architecture ratchet fixture");

    let x86 = load_oils_ratchet_for_arch(&path, "x86_64").expect("load x86 ratchet");
    let aarch64 = load_oils_ratchet_for_arch(&path, "aarch64").expect("load aarch64 ratchet");
    std::fs::remove_file(&path).expect("remove ratchet fixture");

    assert_eq!(x86["sample.test.sh::000::sample"].fields, ["stdout"]);
    assert!(!x86.contains_key("arm-only.test.sh::000::sample"));
    assert_eq!(aarch64["sample.test.sh::000::sample"].fields, ["stderr"]);
    assert!(aarch64.contains_key("arm-only.test.sh::000::sample"));
}

#[test]
fn ratchet_loader_rejects_duplicate_case_ids() {
    let path = ratchet_fixture_path("duplicate");
    let fingerprint = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let row =
        format!("sample.test.sh::000::sample\t*\tstatus,stdout\t{fingerprint}\t{fingerprint}\n");
    std::fs::write(
        &path,
        format!("case\tarch\tfields\toracle_sha256\tcandidate_sha256\n{row}{row}"),
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
            "case\tarch\tfields\toracle_sha256\tcandidate_sha256\n\
             sample.test.sh::000::sample\t*\tstdout\tvariable\t{fingerprint}\n"
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
fn report_contains_tally_artifacts_and_arch_observations() {
    let report_dir = workspace_root().join("target/oils-report-fixture");
    if report_dir.exists() {
        std::fs::remove_dir_all(&report_dir).expect("remove old report fixture");
    }
    let outcome = mismatch();
    let assessments = assess_oils_outcomes(vec![outcome.clone()], &Default::default());

    std::fs::create_dir_all(&report_dir).expect("create report fixture");
    std::fs::write(report_dir.join("suggested-ratchet.tsv"), "obsolete")
        .expect("write legacy suggested ratchet");

    let tally = write_oils_report(&report_dir, &assessments).expect("write Oils report");

    assert_eq!(tally.fail, 1);
    let report = std::fs::read_to_string(report_dir.join("report.tsv")).expect("read report");
    assert!(report.contains("verdict\tcase\tarch\tfields\toracle_sha256\tcandidate_sha256"));
    assert!(report.contains(&format!(
        "FAIL\tsample.test.sh::000::sample\t{}\tstatus,stdout",
        std::env::consts::ARCH
    )));
    let observations = std::fs::read_to_string(
        report_dir.join(format!("observed-ratchet-{}.tsv", std::env::consts::ARCH)),
    )
    .expect("read architecture observations");
    assert!(observations.contains(&candidate_fingerprint(&outcome.bash)));
    assert!(observations.contains(&candidate_fingerprint(&outcome.cherub)));
    assert!(!report_dir.join("suggested-ratchet.tsv").exists());
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

    let loaded = load_oils_ratchet(
        &report_dir.join(format!("observed-ratchet-{}.tsv", std::env::consts::ARCH)),
    )
    .expect("load escaped Oils report");

    assert!(loaded.contains_key(&outcome.id));
    std::fs::remove_dir_all(report_dir).expect("remove escaped report fixture");
}
