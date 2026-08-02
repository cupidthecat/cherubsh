use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_test_harness::{cherub_path, required_oracle_bash_path};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Output {
    status: i32,
    stdout: String,
    stderr: String,
}

#[derive(Default)]
struct Spec<'a> {
    args: Vec<&'a str>,
    stdin: Option<&'a str>,
    env: Vec<(&'a str, &'a str)>,
}

fn cherub() -> PathBuf {
    cherub_path().expect("find cherubsh test binary")
}

fn bash_oracle() -> PathBuf {
    required_oracle_bash_path().expect("resolve pinned Bash oracle")
}

fn run_shell(path: PathBuf, spec: &Spec<'_>) -> Output {
    let mut command = Command::new(path);
    command.args(["--norc", "--noprofile"]);
    command.args(&spec.args);
    command.env_clear();
    command.env("PATH", "/usr/bin:/bin");
    command.env("HOME", "/tmp");
    command.env("LANG", "C");
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    if spec.stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn().expect("spawn shell");
    if let Some(input) = spec.stdin {
        child
            .stdin
            .take()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait shell");
    let status = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + output.status.signal().unwrap_or(0)
    });
    Output {
        status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn run_both(spec: &Spec<'_>) -> (Output, Output) {
    let bash = bash_oracle();
    (run_shell(bash, spec), run_shell(cherub(), spec))
}

fn run_cherub_with_args(args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = Command::new(cherub());
    command.args(args);
    command.env_clear();
    command.env("PATH", "/usr/bin:/bin");
    command.env("HOME", "/tmp");
    command.env("LANG", "C");
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn().expect("spawn cherub");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait cherub");
    let status = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + output.status.signal().unwrap_or(0)
    });
    Output {
        status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn run_cherub_in_home(args: &[&str], stdin: Option<&str>, home: &std::path::Path) -> Output {
    let mut command = Command::new(cherub());
    command.args(args);
    command.env_clear();
    command.env("PATH", "/usr/bin:/bin");
    command.env("HOME", home);
    command.env("LANG", "C");
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn().expect("spawn cherub");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin pipe")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait cherub");
    let status = output.status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + output.status.signal().unwrap_or(0)
    });
    Output {
        status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn temp_file(name: &str, content: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("cherubsh-{name}-{}-{nonce}.sh", std::process::id()));
    fs::write(&path, content).expect("write temp script");
    path
}

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cherubsh-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create temp directory");
    path
}

#[test]
fn interactive_shell_reads_cherubrc_and_ignores_bashrc() {
    let home = temp_dir("cherubrc-default");
    fs::write(home.join(".cherubrc"), "printf 'from-cherubrc\\n'\n").expect("write cherubrc");
    fs::write(home.join(".bashrc"), "printf 'from-bashrc\\n'\n").expect("write bashrc");

    let output = run_cherub_in_home(&["--noprofile", "-i", "-c", "true"], None, &home);
    let _ = fs::remove_dir_all(home);

    assert_eq!(output.status, 0);
    assert!(output.stdout.contains("from-cherubrc"));
    assert!(!output.stdout.contains("from-bashrc"));
}

#[test]
fn missing_cherubrc_does_not_fall_back_to_bashrc() {
    let home = temp_dir("cherubrc-missing");
    fs::write(home.join(".bashrc"), "printf 'from-bashrc\\n'\n").expect("write bashrc");

    let output = run_cherub_in_home(&["--noprofile", "-i", "-c", "true"], None, &home);
    let _ = fs::remove_dir_all(home);

    assert_eq!(output.status, 0);
    assert!(!output.stdout.contains("from-bashrc"));
}

#[test]
fn norc_skips_cherubrc() {
    let home = temp_dir("cherubrc-norc");
    fs::write(home.join(".cherubrc"), "printf 'from-cherubrc\\n'\n").expect("write cherubrc");

    let output = run_cherub_in_home(&["--noprofile", "--norc", "-i", "-c", "true"], None, &home);
    let _ = fs::remove_dir_all(home);

    assert_eq!(output.status, 0);
    assert!(!output.stdout.contains("from-cherubrc"));
}

#[test]
fn rcfile_overrides_cherubrc() {
    let home = temp_dir("cherubrc-override");
    let custom = home.join("custom.rc");
    fs::write(home.join(".cherubrc"), "printf 'from-cherubrc\\n'\n").expect("write cherubrc");
    fs::write(&custom, "printf 'from-custom\\n'\n").expect("write custom rc");
    let custom_arg = custom.display().to_string();

    let output = run_cherub_in_home(
        &["--noprofile", "--rcfile", &custom_arg, "-i", "-c", "true"],
        None,
        &home,
    );
    let _ = fs::remove_dir_all(home);

    assert_eq!(output.status, 0);
    assert!(output.stdout.contains("from-custom"));
    assert!(!output.stdout.contains("from-cherubrc"));
}

#[test]
fn noexec_dash_c_parses_without_executing() {
    let spec = Spec {
        args: vec!["-n", "-c", "echo hi"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(bash.status, 0);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn noexec_dash_c_reports_syntax_failure() {
    let spec = Spec {
        args: vec!["-n", "-c", "if"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_ne!(bash.status, 0);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert!(!cherub.stderr.is_empty());
}

#[test]
fn bash_versinfo_is_indexed_array_for_startup_hooks() {
    let spec = Spec {
        args: vec![
            "-c",
            r#"printf '%s\n' "${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]} ${BASH_VERSINFO[2]} ${BASH_VERSINFO[3]} ${BASH_VERSINFO[4]}" "${#BASH_VERSINFO[@]}"; (( BASH_VERSINFO[0] >= 3 && BASH_VERSINFO[1] >= 1 )); echo status=$?"#,
        ],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(bash.status, 0);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn interactive_rc_functions_survive_for_dash_c() {
    let rc = temp_file(
        "rc-function",
        r#"
from_rc() { echo from-rc; }
"#,
    );
    let rc_arg = rc.display().to_string();
    let cherub = run_cherub_with_args(&["--rcfile", &rc_arg, "-i", "-c", "from_rc"], None);
    let _ = fs::remove_file(rc);
    assert_eq!(cherub.status, 0);
    assert_eq!(cherub.stdout, "from-rc\n");
    assert!(!cherub.stderr.contains("from_rc: command not found"));
}

#[test]
fn prompt_command_uses_startup_functions() {
    let rc = temp_file(
        "prompt-command-function",
        r#"
pc_from_rc() { echo prompt-hit; PROMPT_COMMAND=; }
PROMPT_COMMAND=pc_from_rc
"#,
    );
    let rc_arg = rc.display().to_string();
    let cherub = run_cherub_with_args(&["--rcfile", &rc_arg, "-i"], Some("exit\n"));
    let _ = fs::remove_file(rc);
    assert_eq!(cherub.status, 0);
    assert!(cherub.stdout.contains("prompt-hit"));
    assert!(!cherub.stderr.contains("pc_from_rc: command not found"));
}

#[test]
fn prompt_command_substitution_preserves_previous_exit_status() {
    let cherub = run_cherub_with_args(
        &["--noprofile", "--norc", "-i"],
        Some("PS1='$(printf prompt)> '\nfalse\nprintf 'after:%s\\n' \"$?\"\nexit\n"),
    );
    assert_eq!(cherub.status, 0);
    assert!(cherub.stdout.contains("after:1\n"), "{cherub:?}");
}

#[test]
fn just_one_command_from_stdin_reads_one_line() {
    let spec = Spec {
        args: vec!["-t"],
        stdin: Some("echo one\necho two\n"),
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn noninteractive_shell_does_not_create_mailcheck() {
    let spec = Spec {
        args: vec![
            "-c",
            "if [[ -v MAILCHECK ]]; then echo set; else echo unset; fi",
        ],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub, bash);
}

#[test]
fn just_one_command_from_stdin_reads_compound_command() {
    let spec = Spec {
        args: vec!["-t"],
        stdin: Some("if true\nthen echo yes\nfi\necho no\n"),
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn failglob_expansion_error_skips_simple_command() {
    let script = temp_file(
        "failglob",
        "printf 'before\\n'\nshopt -s failglob\nprintf 'bad %s\\n' /tmp/cherubsh-definitely-no-such-*\nprintf 'after\\n'\n",
    );
    let script_arg = script.to_string_lossy().into_owned();
    let spec = Spec {
        args: vec![script_arg.as_str()],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(&script);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "before\nafter\n");
    assert_eq!(cherub.stderr, bash.stderr);
    assert!(cherub
        .stderr
        .contains("line 3: no match: /tmp/cherubsh-definitely-no-such-*"));
}

#[test]
fn command_substitution_heredoc_delimiter_continuation_matches_bash() {
    let spec = Spec {
        args: vec![
            "-c",
            "x=$( cat <<\\EOT\\\n4\nd \\\ng\nEOT4\n)\necho \"$x\"\n",
        ],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn command_substitution_heredoc_eof_warnings_match_bash() {
    let script = temp_file(
        "comsub-heredoc-eof",
        "foo=$(cat <<EOF\nhi\nEOF )\necho \"$foo\"\n",
    );
    let script_arg = script.to_string_lossy().into_owned();
    let spec = Spec {
        args: vec![script_arg.as_str()],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(&script);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn heredoc_body_command_substitution_eof_diagnostic_matches_bash() {
    let script = temp_file(
        "heredoc-comsub-eof",
        "read foo <<EOF\n$(seq 10\nEOF\n\ntrue\n",
    );
    let script_arg = script.to_string_lossy().into_owned();
    let spec = Spec {
        args: vec![script_arg.as_str()],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(&script);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn outer_heredoc_ignores_delimiter_inside_command_substitution() {
    let script = temp_file(
        "heredoc-comsub-delimiter-scope",
        "cat <<EOF && grep $(\n foobar\nEOF\necho notthereanywhere) *.c\n",
    );
    let script_arg = script.to_string_lossy().into_owned();
    let spec = Spec {
        args: vec![script_arg.as_str()],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(&script);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn command_substitution_execution_expands_aliases_like_bash() {
    let spec = Spec {
        args: vec![
            "-c",
            "shopt -s expand_aliases\nset -o posix\nalias hi='echo ok'\necho \"$( hi )\"\n",
        ],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn nested_command_substitution_quotes_do_not_close_outer_quotes() {
    let spec = Spec {
        args: vec!["-c", "printf '<%s>\\n' $(echo \"foo$(echo \")\")\")"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);

    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn just_one_command_from_script_reads_one_line() {
    let script = temp_file("phase1-t", "echo one\necho two\n");
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec!["-t", &script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn dash_c_keeps_full_string_under_t() {
    let spec = Spec {
        args: vec!["-t", "-c", "echo one; echo two"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn bash_env_is_sourced_before_dash_c_body() {
    let env_file = temp_file("phase1-env", "echo from-env\n");
    let env_arg = env_file.to_string_lossy().to_string();
    let spec = Spec {
        args: vec!["-c", "echo body"],
        env: vec![("BASH_ENV", &env_arg)],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(env_file);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn dash_c_executes_complete_command_before_later_syntax_error() {
    let spec = Spec {
        args: vec!["-c", "echo ok\nif"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "ok\n");
}

#[test]
fn script_executes_complete_command_before_later_syntax_error() {
    let script = temp_file("phase1-late-syntax", "echo ok\nif\n");
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "ok\n");
}

#[test]
fn script_invalid_for_name_is_non_fatal() {
    let script = temp_file(
        "phase1-invalid-for-name",
        "for 1 in a b; do echo bad; done\necho status:$?\necho after\n",
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(bash.status, 0);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "status:1\nafter\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn backtick_quotes_do_not_force_more_input() {
    let script = temp_file(
        "phase1-backtick-complete",
        r#"show() { printf 'argc=%s\n' "$#"; for arg; do printf '<%s>\n' "$arg"; done; }
show `echo "(\\")"`
# produces no output
: `: "\\""`
# ultimate workaround
show `echo "(\")"`
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "argc=1\n<(\")>\nargc=1\n<(\")>\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn script_eof_backslash_matches_bash_53() {
    let spec = Spec {
        stdin: Some(r#"printf '[%s]\n' \"#),
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "[\\]\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn backtick_substitution_with_trailing_escaped_backslash_matches_bash() {
    let script = temp_file(
        "phase1-backtick-trailing-backslash",
        "qpath='\\/tmp\\/foo\\/bar'\nprintf \"%s\\n\" ${qpath//\"`printf '%s' \\\\`\"/}\n",
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "/tmp/foo/bar\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn quoted_empty_command_substitution_preserves_field() {
    let script = temp_file(
        "phase1-quoted-empty-cmdsub",
        r#"show() { printf 'argc=%s\n' "$#"; for arg; do printf '<%s>\n' "$arg"; done; }
x=x
show "$(:)" "${x:+"$(:)"}" "`:`" "${x:+"`:`"}"
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "argc=4\n<>\n<>\n<>\n<>\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn command_substitution_reader_ignores_quoted_and_comment_parens() {
    let script = temp_file(
        "phase1-cmdsub-reader-parens",
        r#"echo $(echo sh_352.27 ')' ")" \)
	# ) comment
	)
echo after
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "sh_352.27 ) ) )\nafter\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn double_paren_command_substitution_ambiguity_matches_bash() {
    let spec = Spec {
        args: vec![
            "-c",
            "set -o posix\necho $((echo sh_352.25a);(echo sh_352.25b))\necho $(( echo ab cde ) )",
        ],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "sh_352.25a sh_352.25b\nab cde\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn command_substitution_case_pattern_parens_match_bash() {
    let spec = Spec {
        args: vec!["-c", "echo $(case x in x) echo yes;; esac)"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "yes\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn command_substitution_comments_after_words_match_bash() {
    let script = temp_file(
        "phase1-cmdsub-comment-boundary",
        r#"echo $(
echo abc # a comment with )
)
echo $(# a comment with )
echo def)
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "abc\ndef\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn heredoc_body_quotes_do_not_hold_reader_open() {
    let script = temp_file(
        "phase1-heredoc-body-quotes",
        r#"echo $(cat <<\eof
'
eof
)
echo after 1
echo "$(cat <<\eof
`
eof
)"
echo after 2
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "'\nafter 1\n`\nafter 2\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn heredoc_reader_keeps_line_continued_body_until_real_delimiter() {
    let spec = Spec {
        stdin: Some("cat <<END\nhello\nEND\\\nEND\nEND\necho end ENDEND\n"),
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stdout, "hello\nENDEND\nend ENDEND\n");
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn backslash_newline_continuation_reaches_next_line() {
    let script = temp_file(
        "phase1-backslash-newline",
        r#"recho() { n=1; for arg; do printf 'argv[%s] = <%s>\n' "$n" "$arg"; n=$((n + 1)); done; }
echo $(echo abcde)\
foo
recho $(echo abcde)\
   foo
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(
        cherub.stdout,
        "abcdefoo\nargv[1] = <abcde>\nargv[2] = <foo>\n"
    );
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn redirected_stdin_is_shared_with_child_shells() {
    let sub = temp_file(
        "phase1-input-sub",
        "read line\necho \"child read: $line\"\n",
    );
    let sub_arg = sub.to_string_lossy().to_string();
    let bash_arg = bash_oracle().to_string_lossy().to_string();
    let cherub_arg = cherub().to_string_lossy().to_string();
    let bash_stdin = format!("\"{bash_arg}\" \"{sub_arg}\"\nline from parent stdin\necho done\n");
    let cherub_stdin =
        format!("\"{cherub_arg}\" \"{sub_arg}\"\nline from parent stdin\necho done\n");
    let bash_spec = Spec {
        stdin: Some(&bash_stdin),
        ..Spec::default()
    };
    let cherub_spec = Spec {
        stdin: Some(&cherub_stdin),
        ..Spec::default()
    };
    let bash = run_shell(bash_oracle(), &bash_spec);
    let cherub = run_shell(cherub(), &cherub_spec);
    let _ = fs::remove_file(sub);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn noexec_reads_later_syntax_without_executing_prior_command() {
    let spec = Spec {
        args: vec!["-n", "-c", "echo ok\nif"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert!(cherub.stdout.is_empty());
}

#[test]
fn bash_env_value_is_expanded() {
    let home = std::env::temp_dir().join(format!(
        "cherubsh-home-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&home).expect("create temp home");
    let env_file = home.join("env.sh");
    fs::write(&env_file, "echo expanded-env\n").expect("write env file");
    let home_arg = home.to_string_lossy().to_string();
    let spec = Spec {
        args: vec!["-c", "echo body"],
        env: vec![("HOME", &home_arg), ("BASH_ENV", "$HOME/env.sh")],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(env_file);
    let _ = fs::remove_dir(home);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn local_oracle_rejects_wordexp_option() {
    let spec = Spec {
        args: vec!["--wordexp"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn plus_c_still_takes_command_string() {
    let spec = Spec {
        args: vec!["+c", "echo hi"],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
}

#[test]
fn uid_and_euid_are_bound_readonly_integers() {
    let spec = Spec {
        args: vec![
            "-c",
            "echo uid:${UID+set}:$UID euid:${EUID+set}:$EUID; (( UID >= 0 && EUID >= 0 )); echo arith:$?",
        ],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn groups_zero_is_effective_group_for_test_g() {
    let script = r#"
path="/tmp/cherubsh-test-g-$$"
: > "$path"
chgrp "${GROUPS[0]}" "$path"
echo "groups0:${GROUPS[0]}"
test -G "$path"
echo "test-g:$?"
rm -f "$path"
"#;
    let spec = Spec {
        args: vec!["-c", script],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}

#[test]
fn test_builtin_reports_bash_style_script_diagnostics() {
    let script = temp_file(
        "phase1-test-diag",
        r#"b()
{
	[ "$@" ]
	echo $?
}

t()
{
	test "$@"
	echo $?
}

t 4+3 -eq 7
b 4+3 -eq 7
t \( 1 = 2
b \( 1 = 2
t -A v
t 4 -eq 4 -a 2 -ne 5 -a 4 -ne
t 4 -eq 4 -a 3 4
[
echo $?
"#,
    );
    let script_arg = script.to_string_lossy().to_string();
    let spec = Spec {
        args: vec![&script_arg],
        ..Spec::default()
    };
    let (bash, cherub) = run_both(&spec);
    let _ = fs::remove_file(script);
    assert_eq!(cherub.status, bash.status);
    assert_eq!(cherub.stdout, bash.stdout);
    assert_eq!(cherub.stderr, bash.stderr);
}
