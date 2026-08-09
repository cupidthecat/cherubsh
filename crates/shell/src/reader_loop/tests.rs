#[cfg(test)]
mod tests {
    use super::{
        completion_request, extract_command_substitution_for_parse,
        has_unclosed_command_substitution, has_unclosed_heredoc, parse_text,
        skip_double_quoted_for_parse,
    };
    use crate::completion::CompletionQuote;

    const BREAKS: &str = " \t\n\"'@><=;|&(:";

    #[test]
    fn parse_validator_handles_command_substitution_in_array_assignment() {
        let source = r##"items=($(
  {
    printf "%s\\n" "${items[@]:-}"
    GIT_PAGER='' git -C "${list_path}" \
      grep \
      --color=never \
      --extended-regexp \
      --files-with-matches \
      --ignore-case \
      --max-depth 0 \
      --text \
      -e "${PATTERN:-"#pinned"}" \
      "${list_path}" 2>/dev/null || :
  } | awk '!NF || !seen[$0]++'
))
"##;
        let body_start = source.find("$(").unwrap() + 2;
        let pattern_quote = source.find("\"${PATTERN").unwrap();
        assert!(skip_double_quoted_for_parse(source.as_bytes(), pattern_quote + 1).is_some());
        let (_, _, body, _) = extract_command_substitution_for_parse(source, body_start).unwrap();
        assert!(body.ends_with("} | awk '!NF || !seen[$0]++'\n"), "{body:?}");
        assert!(parse_text(source, true, false, true).is_ok());
    }

    #[test]
    fn completion_words_match_bash_word_break_tokens() {
        let request = completion_request("cmd --foo=ba", 12, BREAKS);
        assert_eq!(request.words, ["cmd", "--foo", "=", "ba"]);
        assert_eq!(request.cword, 3);
        assert_eq!(request.command, "cmd");
        assert_eq!(request.current, "ba");
        assert_eq!(request.previous, "=");
        assert_eq!(request.replace_start, 10);
    }

    #[test]
    fn completion_after_a_break_uses_an_empty_readline_word() {
        let request = completion_request("cmd --foo=", 10, BREAKS);
        assert_eq!(request.words, ["cmd", "--foo", "="]);
        assert_eq!(request.cword, 2);
        assert_eq!(request.current, "");
        assert_eq!(request.replace_start, 10);
    }

    #[test]
    fn completion_tracks_open_quotes_and_utf8_byte_offsets() {
        let line = "écho \"two wo";
        let request = completion_request(line, line.len(), BREAKS);
        assert_eq!(request.words, ["écho", "two wo"]);
        assert_eq!(request.current, "two wo");
        assert_eq!(request.quote, CompletionQuote::Double);
        assert_eq!(request.replace_start, "écho \"".len());
    }

    #[test]
    fn completion_starts_after_the_nearest_command_separator() {
        let line = "echo old; cmd val";
        let request = completion_request(line, line.len(), BREAKS);
        assert_eq!(request.words, ["cmd", "val"]);
        assert_eq!(request.command, "cmd");
        assert_eq!(request.current, "val");
    }

    #[test]
    fn unquoted_heredoc_probe_uses_logical_lines_for_delimiter() {
        assert!(has_unclosed_heredoc("cat <<END\nhello\nEND\\\n"));
        assert!(has_unclosed_heredoc("cat <<END\nhello\nEND\\\nEND\n"));
        assert!(!has_unclosed_heredoc("cat <<END\nhello\nEND\\\nEND\nEND\n"));
    }

    #[test]
    fn quoted_heredoc_probe_does_not_join_backslash_newline() {
        assert!(!has_unclosed_heredoc("cat <<'END'\nhello\nEND\n"));
    }

    #[test]
    fn deblank_heredoc_probe_strips_tabs_from_quoted_delimiter() {
        assert!(!has_unclosed_heredoc("cat <<-'\tEND'\n\thello\n\tEND\n"));
    }

    #[test]
    fn command_substitution_probe_ignores_top_level_heredoc_body() {
        assert!(!has_unclosed_command_substitution(
            "read foo <<EOF\n$(seq 10\nEOF\n"
        ));
    }
}
