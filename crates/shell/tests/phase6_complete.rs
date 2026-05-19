//! Programmable-completion smoke tests.

use cherubsh_test_harness::{assert_parity, run_cherub, RunSpec};

#[test]
fn compgen_wordlist() {
    let out = run_cherub(&RunSpec {
        script: Some("compgen -W 'apple banana cherry' a"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert_eq!(out.stdout.trim(), "apple", "stdout={:?}", out.stdout);
}

#[test]
fn complete_register_and_print() {
    let out = run_cherub(&RunSpec {
        script: Some("complete -W 'a b c' mycmd; complete -p"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(
        out.stdout.contains("mycmd"),
        "expected mycmd in completion listing, got: {:?}",
        out.stdout
    );
}

#[test]
fn complete_remove() {
    let out = run_cherub(&RunSpec {
        script: Some("complete -W 'x' c; complete -r c; complete -p"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(!out.stdout.contains(" c\n"), "spec for c should be gone");
}

#[test]
fn complete_remove_all_parity() {
    assert_parity(&RunSpec {
        script: Some("complete -W 'x' c; complete -f d; complete -r; complete -p"),
        ..RunSpec::default()
    });
}

#[test]
fn complete_double_dash_and_named_print_parity() {
    assert_parity(&RunSpec {
        script: Some("complete -f -- . source; complete -p . source"),
        ..RunSpec::default()
    });
}

#[test]
fn complete_missing_named_spec_parity() {
    assert_parity(&RunSpec {
        script: Some("complete -f cmd; complete -p cmd missing; echo print=$?; complete -r missing; echo remove=$?"),
        ..RunSpec::default()
    });
}

#[test]
fn compgen_shopt_names_parity() {
    assert_parity(&RunSpec {
        script: Some("compgen -A shopt globskipdots; compgen -A setopt pipefail"),
        ..RunSpec::default()
    });
}
