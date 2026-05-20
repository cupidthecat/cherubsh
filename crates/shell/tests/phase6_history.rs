//! Phase 6 history & history-expansion smoke tests.

use cherubsh_test_harness::{assert_parity_strict, run_cherub, RunSpec};

#[test]
fn history_s_adds_entry() {
    let out = run_cherub(&RunSpec {
        script: Some("history -s 'echo from-test'; history | tail -1"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(
        out.stdout.contains("echo from-test"),
        "expected 'echo from-test' in stdout, got: {:?}",
        out.stdout
    );
}

#[test]
fn history_p_expands_bang_bang() {
    let out = run_cherub(&RunSpec {
        script: Some("history -s 'echo target'; history -p '!!'"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(
        out.stdout.trim_end().ends_with("echo target"),
        "expected '!! -> echo target', got: {:?}",
        out.stdout
    );
}

#[test]
fn history_c_clears() {
    let out = run_cherub(&RunSpec {
        script: Some("history -s 'echo a'; history -c; history"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(
        out.stdout.trim().is_empty(),
        "expected empty stdout after history -c, got: {:?}",
        out.stdout
    );
}

#[test]
fn interactive_dash_c_does_not_record_command_history() {
    let out = run_cherub(&RunSpec {
        args: vec!["-i", "-c", "history"],
        env: vec![("HISTFILE", "/dev/null")],
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(
        out.stdout.trim().is_empty(),
        "expected no history for interactive -c, got: {:?}",
        out.stdout
    );
}

#[test]
fn noninteractive_history_option_loads_histfile_and_records_commands() {
    assert_parity_strict(&RunSpec {
        script: Some(
            "tmp=${TMPDIR:-/tmp}/cherub-hist-$$; \
             printf 'echo seed\\n' > \"$tmp\"; \
             HISTFILE=$tmp; HISTSIZE=32; HISTIGNORE='history*'; set -o history; \
             echo new; \
             history; \
             rm -f \"$tmp\"",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn noninteractive_set_h_expands_from_history() {
    assert_parity_strict(&RunSpec {
        script: Some("HISTFILE=/dev/null; set -o history; history -s 'echo target'; set -H; !!"),
        ..RunSpec::default()
    });
}

#[test]
fn fc_s_alias_replays_previous_command_with_global_substitutions() {
    assert_parity_strict(&RunSpec {
        script: Some(
            "HISTFILE=/dev/null; unset HISTIGNORE HISTCONTROL; \
             set -o history; shopt -s expand_aliases; alias r='fc -s'; \
             history -c; echo aa ab ac; r a=x; r x=4 b=8",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn heredoc_history_entry_replays_with_original_body() {
    assert_parity_strict(&RunSpec {
        script: Some(
            "HISTFILE=/dev/null; unset HISTIGNORE HISTCONTROL; set -o history; \
             history -c; cat <<!\none\ntwo\n!\nhistory; fc -s cat",
        ),
        ..RunSpec::default()
    });
}
