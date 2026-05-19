//! Source/eval parity tests.

use cherubsh_test_harness::{assert_parity, RunSpec};

fn parity(script: &str) {
    assert_parity(&RunSpec {
        script: Some(script),
        ..RunSpec::default()
    });
}

#[test]
fn source_empty_input_returns_success() {
    parity(". /dev/null; echo status:$?");
}

#[test]
fn eval_empty_input_returns_success() {
    parity("eval ''; echo status:$?");
}

#[test]
fn assignment_prefix_expands_left_to_right() {
    parity(
        "unset K A; \
         K=dvb0.net A=${K#dvb} eval echo \\$A; \
         unset K A; \
         K=dvb0.net A=${K#dvb}; echo \"$A\"; \
         unset K A; \
         K=dvb0.net A=${K#dvb} echo \"$A\"; \
         echo after:${A-unset}:${K-unset}",
    );
}

#[test]
fn ifs_colon_does_not_break_posix_pattern_classes() {
    parity("IFS=:; case A in ([[:graph:]]) echo graph;; *) echo non-graph;; esac; [[ A == [[:graph:]] ]] && echo yes || echo no");
}

#[test]
fn source_args_restore_unless_script_sets_positionals() {
    parity(
        "tmp=${TMPDIR:-/tmp}/cherub-source-$$; mkdir -p \"$tmp\"; \
         printf '%s\n' 'echo \"$@\"' > \"$tmp/echoargs\"; \
         printf '%s\n' 'set -- m n o p' > \"$tmp/setargs\"; \
         set -- a b c; . \"$tmp/echoargs\" x y z; echo \"$@\"; \
         . \"$tmp/setargs\" x y z; echo \"$@\"; rm -rf \"$tmp\"",
    );
}

#[test]
fn quoted_heredoc_delimiter_does_not_group_following_aliases() {
    parity(concat!(
        "shopt -s expand_aliases\n",
        "tmp=${TMPDIR:-/tmp}/cherub-hd-$$\n",
        "cat > \"$tmp\" << \\EOF\n",
        "echo heredoc\n",
        "EOF\n",
        ". \"$tmp\"\n",
        "alias later='echo alias-ok'\n",
        "later\n",
        "rm -f \"$tmp\"\n",
    ));
}

#[test]
fn empty_arithmetic_command_is_false_without_diagnostic() {
    parity("(()) 2>/tmp/cherub-empty-arith-$$; echo status:$?; cat /tmp/cherub-empty-arith-$$; rm -f /tmp/cherub-empty-arith-$$");
}

#[test]
fn nounset_expansion_error_exits_subshell_list() {
    parity("set -u; ( echo $9; echo after ); echo parent:$?");
}
