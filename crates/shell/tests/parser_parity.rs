//! Parser parity tests against system bash.
//!
//! Each fixture is a string of bash source. The test asserts that
//! `bash -n` and `cherubsh --parse-only` agree on whether the input is
//! syntactically valid.

use cherubsh_test_harness::assert_parser_accepts_like_bash;

fn check_all(fixtures: &[&str]) {
    let mut failures = Vec::new();
    for fixture in fixtures {
        let result = std::panic::catch_unwind(|| assert_parser_accepts_like_bash(fixture));
        if result.is_err() {
            failures.push(*fixture);
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixtures diverged from bash:\n{}",
        failures.len(),
        failures
            .iter()
            .map(|f| format!("  {:?}", f))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn quoting_fixtures() {
    check_all(&[
        r#"echo 'simple'"#,
        r#"echo "double""#,
        r#"echo "$var""#,
        r#"echo "a b c""#,
        r#"echo $'\n\t'"#,
        r#"echo $"locale""#,
        r#"echo \$x"#,
        r#"echo 'a"b'"#,
        r#"echo "a'b""#,
        r#"echo "$(echo nested)""#,
        r#"echo "outer $(echo "inner") tail""#,
        r#"echo 'a;b;c'"#,
        r#"echo "a;b;c""#,
    ]);
}

#[test]
fn redirection_fixtures() {
    check_all(&[
        "echo a > /tmp/x",
        "echo a >> /tmp/x",
        "cat < /tmp/x",
        "cat <<< hello",
        "cat <<EOF\nhi\nEOF",
        "cat <<-EOF\n\thi\nEOF",
        "cmd 2>&1",
        "cmd 1>&2",
        "exec 3>&-",
        "exec 4<&-",
        "cmd >|file",
        "cmd <>file",
        "cmd &>file",
        "cmd &>>file",
    ]);
}

#[test]
fn compound_assignment_fixtures() {
    check_all(&[
        "arr=()",
        "arr=(a)",
        "arr=(a b c)",
        "arr=(one two three)",
        "a[0]=x",
        "a[10]=y",
        "declare -a arr=(1 2 3)",
    ]);
}

#[test]
fn cond_fixtures() {
    check_all(&[
        "[[ -n $x ]]",
        "[[ -z $x ]]",
        "[[ -e /tmp ]]",
        "[[ -f /tmp/x ]]",
        "[[ -d /tmp ]]",
        "[[ $a == $b ]]",
        "[[ $a = $b ]]",
        "[[ $a != $b ]]",
        "[[ $a -eq 1 ]]",
        "[[ $a -lt 10 ]]",
        "[[ $a == $b && -n $c ]]",
        "[[ $a == $b || -z $c ]]",
        "[[ ! -e /tmp ]]",
        "[[ ( $a == $b ) ]]",
        "[[ ( $a == $b ) || $c ]]",
        "[[ $x =~ abc ]]",
    ]);
}

#[test]
fn arith_fixtures() {
    check_all(&[
        "(( 1 + 2 ))",
        "(( a = 1 ))",
        "(( a < b ))",
        "(( a == b ))",
        "(( a++ ))",
        "for ((i=0; i<10; i++)); do echo $i; done",
        "for (( ; ; )); do break; done",
    ]);
}

#[test]
fn function_fixtures() {
    check_all(&[
        "f() { :; }",
        "function f { :; }",
        "function f() { :; }",
        "function f { [[ -d $1 ]] || :; }",
        "f()\n{ :; }",
        "f() { echo $1; }",
        "_a1() { return 0; }",
    ]);
}

#[test]
fn case_fixtures() {
    check_all(&[
        "case x in a) :; ;; esac",
        "case x in a|b) :; ;; esac",
        "case x in a) :; ;; b) :; ;; esac",
        "case x in a) :; ;& b) :; ;; esac",
        "case x in a) :; ;;& b) :; ;; esac",
        "case x in [abc]) :; ;; esac",
        "case x in *) :; ;; esac",
    ]);
}

#[test]
fn time_fixtures() {
    check_all(&[
        "time sleep 1",
        "time -p sleep 1",
        "time true | true",
        "time;",
        "!;",
    ]);
}

#[test]
fn coproc_fixtures() {
    check_all(&[
        "coproc sleep 1",
        "coproc name",
        "coproc mypipe { sleep 1; }",
        "coproc ( sleep 1 )",
    ]);
}

#[test]
fn pipeline_fixtures() {
    check_all(&[
        "true | false",
        "true |\nfalse",
        "true |& false",
        "! true",
        "! true | false",
        "true && false",
        "true || false",
        "true ; false",
        "true & false",
        "a && b || c",
        "a | b | c",
    ]);
}

#[test]
fn loop_and_if_fixtures() {
    check_all(&[
        "while true; do break; done",
        "until false; do break; done",
        "for i in 1 2 3; do echo $i; done",
        "for i; { echo $i; }",
        "for i in a b; { echo $i; }",
        "for i in a b\ndo echo $i; done",
        "select x in a b; do break; done",
        "select x; { break; }",
        "if true; then :; fi",
        "if true; then :; fi >out",
        "if true; then :; else :; fi",
        "if true; then :; elif false; then :; else :; fi",
        "if true\nthen\n  :\nfi",
    ]);
}

#[test]
fn group_and_subshell_fixtures() {
    check_all(&[
        "{ echo a; }",
        "{ echo a; } >out",
        "( echo a )",
        "( echo a ) >out",
        "{ echo a; echo b; }",
        "(echo a; echo b)",
    ]);
}

#[test]
fn substitution_fixtures() {
    check_all(&[
        "echo $var",
        "echo ${var}",
        "echo ${var:-default}",
        "echo ${var:=default}",
        "echo ${var:+other}",
        "echo ${#var}",
        "echo ${var#prefix}",
        "echo ${var%suffix}",
        "echo $(date)",
        "echo `date`",
        "echo $((1+2))",
        "echo ${a[0]}",
        "echo ${a[@]}",
    ]);
}

#[test]
fn process_substitution_fixtures() {
    check_all(&["cat <(echo x)", "tee >(cat)", "diff <(ls) <(ls /tmp)"]);
}

#[test]
fn heredoc_fixtures() {
    check_all(&[
        "cat <<EOF\nhello\nEOF",
        "cat <<'EOF'\nhi\nEOF",
        "cat <<\"EOF\"\nhi\nEOF",
        "cat <<-EOF\n\thi\nEOF",
        "cat <<EOF >/tmp/x\nhi\nEOF",
    ]);
}

#[test]
fn redir_word_fixtures() {
    check_all(&["echo hi {fd}>out", "{fd}<>out echo ok"]);
}

#[test]
fn negative_syntax_fixtures() {
    check_all(&["echo >", "for i in a b do :; done", "{ echo a }"]);
}
