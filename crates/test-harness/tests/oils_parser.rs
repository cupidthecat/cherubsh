use std::path::Path;

use cherubsh_test_harness::oils::{
    default_oils_spec_dir, discover_oils_cases, oils_nondeterministic_case_ids,
    parse_oils_spec_source,
};
use cherubsh_test_harness::workspace_root;

#[test]
fn pinned_bash_corpus_has_expected_inventory() {
    let cases = discover_oils_cases(&default_oils_spec_dir()).expect("discover Oils cases");
    let files = cases
        .iter()
        .map(|case| case.source_file.as_path())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(files.len(), 135);
    assert_eq!(cases.len(), 2_804);
}

#[test]
fn nondeterministic_case_manifest_only_names_vendored_cases() {
    let cases = discover_oils_cases(&default_oils_spec_dir()).expect("discover Oils cases");
    let case_ids = cases
        .iter()
        .map(|case| case.id())
        .collect::<std::collections::BTreeSet<_>>();
    let nondeterministic =
        oils_nondeterministic_case_ids().expect("load Oils nondeterministic case manifest");

    assert_eq!(nondeterministic.len(), 10);
    for id in nondeterministic {
        assert!(
            case_ids.contains(&id),
            "unknown nondeterministic case: {id}"
        );
    }
}

#[test]
fn parses_oils_case_grammar_without_assertion_text_leaking_into_code() {
    let source = r#"## compare_shells: dash bash-4.4 mksh
## tags: interactive dev-minimal
## legacy_tmp_dir: true

#### literal body
# Oils ignores full-line comments while collecting case code.
printf 'one\n'

printf 'two\n'
## status: 0
## STDOUT:
one
two
## END

#### encoded body
## code: printf broken
## BUG bash status: 2
"#;

    let cases = parse_oils_spec_source(Path::new("sample.test.sh"), source).expect("parse spec");

    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0].id(), "sample.test.sh::000::literal body");
    assert_eq!(cases[0].line_number, 5);
    assert_eq!(cases[0].code, "printf 'one\\n'\n\nprintf 'two\\n'\n");
    assert_eq!(cases[0].tags, ["interactive", "dev-minimal"]);
    assert!(cases[0].legacy_tmp_dir);
    assert_eq!(cases[1].code, "printf broken");
}

#[test]
fn rejects_duplicate_qualified_assertions_across_plain_and_json_forms() {
    let source = r#"## compare_shells: bash
#### duplicate output
printf one
## BUG bash stdout: one
## BUG bash stdout-json: "one\n"
"#;

    let error = parse_oils_spec_source(Path::new("duplicate.test.sh"), source)
        .expect_err("duplicate assertion must be rejected");

    assert!(error.to_string().contains("duplicate stdout assertion"));
}

#[test]
fn selected_corpus_uses_python3_helpers() {
    let spec_dir = default_oils_spec_dir();
    let cases = discover_oils_cases(&spec_dir).expect("discover Oils cases");
    let offenders = cases
        .iter()
        .filter(|case| case.code.contains("python2"))
        .map(|case| case.id())
        .collect::<Vec<_>>();
    assert!(offenders.is_empty(), "Python 2 case helpers: {offenders:?}");

    for helper in [
        "argv.py",
        "printenv.py",
        "read_from_fd.py",
        "show_fd_table.py",
        "stdout_stderr.py",
    ] {
        let text = std::fs::read_to_string(spec_dir.join("bin").join(helper))
            .unwrap_or_else(|error| panic!("read {helper}: {error}"));
        assert!(
            text.starts_with("#!/usr/bin/env python3\n"),
            "{helper} does not use Python 3"
        );
    }
}

#[test]
fn oils_vendor_refresh_is_pinned_and_audited() {
    let root = workspace_root();
    let lock = std::fs::read_to_string(root.join("upstream.lock")).expect("read upstream lock");
    assert!(lock.contains("OILS_REPOSITORY=https://github.com/oils-for-unix/oils.git"));
    assert!(lock.contains("OILS_COMMIT=15de8fd779569e6e3a9f5fcbfc00e7df0ebe0380"));

    let script = std::fs::read_to_string(root.join("tools/vendor-oils.sh"))
        .expect("read Oils vendor refresh script");
    for required in [
        "git clone",
        "OILS_COMMIT",
        "rev-parse FETCH_HEAD",
        "oils-python3.patch",
        "EXPECTED_FILES=135",
        "EXPECTED_CASES=2804",
    ] {
        assert!(
            script.contains(required),
            "missing refresh guard: {required}"
        );
    }
    assert!(root.join("tools/oils-python3.patch").is_file());
}
