//! Command/type/hash parity tests.

use cherubsh_test_harness::{assert_parity_strict, RunSpec};

#[test]
fn command_describes_shell_functions() {
    assert_parity_strict(&RunSpec {
        script: Some("func() { echo this is func; }; command -v func; command -V func"),
        ..RunSpec::default()
    });
}

#[test]
fn command_p_executes_with_standard_path() {
    assert_parity_strict(&RunSpec {
        script: Some("PATH= command -p sh -c 'printf \"%s\\n\" \"$0\"'; case ${PATH+x} in x) echo after=set;; *) echo after=unset;; esac; PATH= command -pv cat >/dev/null; echo pv=$?; PATH= command -v cat >/dev/null; echo v=$?"),
        ..RunSpec::default()
    });
}

#[test]
fn startup_sets_default_path_when_environment_lacks_path() {
    assert_parity_strict(&RunSpec {
        script: Some("case ${PATH+x} in x) echo path-set;; *) echo path-unset;; esac; command -v cat >/dev/null; echo cat=$?; export -p | grep 'declare -x PATH' || echo path-not-exported"),
        env_remove: vec!["PATH"],
        ..RunSpec::default()
    });
}

#[test]
fn path_search_skips_files_without_execute_access_for_owner() {
    assert_parity_strict(&RunSpec {
        script: Some(
            "d=${TMPDIR:-/tmp}/cherub-path-$$; \
             rm -rf \"$d\"; mkdir -p \"$d/a\" \"$d/b\"; \
             printf '%s\\n' 'echo bad' > \"$d/a/foo\"; \
             printf '%s\\n' 'echo good' > \"$d/b/foo\"; \
             chmod 655 \"$d/a/foo\"; chmod 755 \"$d/b/foo\"; \
             PATH=\"$d/a:$d/b\" foo; s=$?; rm -rf \"$d\"; exit $s",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn exec_argv0_and_temp_env_parity() {
    assert_parity_strict(&RunSpec {
        script: Some("(exec -a specialname /bin/sh -c 'echo first:$0'); (exec -l -a specialname /bin/sh -c 'echo second:$0'); (FOO=BAR exec printenv) | grep '^FOO='"),
        ..RunSpec::default()
    });
}

#[test]
fn aliases_are_visible_to_type_and_command_only_when_expanded() {
    assert_parity_strict(&RunSpec {
        script: Some(
            "alias m=more; type -t m; type m; command -v m; \
             shopt -s expand_aliases; type -t m; command -v m",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn type_hash_lookups_increment_hash_hits() {
    assert_parity_strict(&RunSpec {
        script: Some(
            "hash -r; \
             hash -p /bin/sh sh; type -p sh; \
             hash -p /tmp/fakecmd fakecmd; type -p fakecmd; type fakecmd; type -t fakecmd; \
             hash",
        ),
        ..RunSpec::default()
    });
}

#[test]
fn type_printed_heredoc_function_can_be_evaled() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"foo()
{
    echo
    cat <<END
bar
END
}
type foo
eval "$(type foo | sed 1d)"
foo"#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn type_renders_compound_heredoc_bodies() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"foo() {
    for f in a b c; do
        cat <<-EOF >> ${f}
        file
        EOF
    done
}
bb()
{
    (
    cat <<EOF
foo
bar
EOF
    )
}
type foo
type bb"#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn type_preserves_arithmetic_expansion_spacing() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"foo() {
    i=0
    while (( i < 2 )); do
        i=$(( i + 1 ))
    done
    echo $((1+2)) x$((1 + 2))y
    for name in $( echo 1 2 3 ); do
        echo "$name"
    done
}
type foo"#,
        ),
        ..RunSpec::default()
    });
}
