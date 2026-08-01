use std::fs;
use std::path::PathBuf;

#[test]
fn interactive_reader_has_focused_responsibility_files() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/reader_loop");
    let expected = [
        ("mod.rs", "pub fn reader_loop_with_exec_state"),
        ("history.rs", "fn history_record_line"),
        ("diagnostics.rs", "fn report_parse_error"),
        ("parsing.rs", "fn parse_text"),
        ("input.rs", "fn read_logical_command"),
        (
            "history_provider.rs",
            "impl HistoryProvider for HistorySnapshot",
        ),
        (
            "completion.rs",
            "impl CompletionProvider for ShellCompleter",
        ),
        ("continuation_probes.rs", "fn parse_error_wants_more_input"),
        (
            "substitution_probes.rs",
            "fn skip_parameter_brace_for_probe",
        ),
        ("heredoc_probes.rs", "fn has_unclosed_heredoc"),
        ("prompt.rs", "fn execute_prompt_command"),
        ("tests.rs", "mod tests"),
    ];

    for (name, responsibility_anchor) in expected {
        let contents = fs::read_to_string(source.join(name))
            .unwrap_or_else(|error| panic!("read interactive reader file {name}: {error}"));
        assert!(
            contents.lines().count() < 900,
            "interactive reader file {name} is still too large"
        );
        assert!(
            contents.contains(responsibility_anchor),
            "interactive reader file {name} lost {responsibility_anchor}"
        );
        assert!(
            !contents.ends_with("\n\n"),
            "interactive reader file {name} has a surplus blank line at EOF"
        );
    }

    let module = fs::read_to_string(source.join("mod.rs")).expect("read reader module root");
    for public_entry in [
        "pub fn reader_loop_with_exec_state",
        "pub fn run_until_eof_with_exec_state",
        "pub fn read_command",
        "pub fn parse_command",
    ] {
        assert!(
            module.contains(public_entry),
            "reader root lost {public_entry}"
        );
    }

    let includes: Vec<_> = module
        .lines()
        .filter(|line| line.starts_with("include!("))
        .collect();
    assert_eq!(
        includes,
        [
            "include!(\"history.rs\");",
            "include!(\"diagnostics.rs\");",
            "include!(\"parsing.rs\");",
            "include!(\"input.rs\");",
            "include!(\"history_provider.rs\");",
            "include!(\"completion.rs\");",
            "include!(\"continuation_probes.rs\");",
            "include!(\"substitution_probes.rs\");",
            "include!(\"heredoc_probes.rs\");",
            "include!(\"prompt.rs\");",
            "include!(\"tests.rs\");",
        ]
    );
}
