fn first_physical_line(current_line: u32, input_text: &str) -> u32 {
    let mut lines = input_text.bytes().filter(|byte| *byte == b'\n').count() as u32;
    if !input_text.ends_with('\n') {
        lines = lines.saturating_add(1);
    }
    current_line.saturating_sub(lines).saturating_add(1).max(1)
}

fn offset_command_lines(command: &mut Command, delta: u32, max_line: Option<u32>) {
    if command.line > 0 {
        command.line = clamp_line(command.line.saturating_add(delta), max_line);
    }
    match &mut command.data {
        CommandData::For(c) => {
            offset_line(&mut c.line, delta, max_line);
            offset_command_lines(&mut c.action, delta, max_line);
        }
        CommandData::Case(c) => {
            offset_line(&mut c.line, delta, max_line);
            for clause in &mut c.clauses {
                if let Some(action) = &mut clause.action {
                    offset_command_lines(action, delta, max_line);
                }
            }
        }
        CommandData::While(c) => {
            offset_command_lines(&mut c.test, delta, max_line);
            offset_command_lines(&mut c.action, delta, max_line);
        }
        CommandData::Until(c) => {
            offset_command_lines(&mut c.test, delta, max_line);
            offset_command_lines(&mut c.action, delta, max_line);
        }
        CommandData::If(c) => {
            offset_command_lines(&mut c.test, delta, max_line);
            offset_command_lines(&mut c.true_case, delta, max_line);
            if let Some(false_case) = &mut c.false_case {
                offset_command_lines(false_case, delta, max_line);
            }
        }
        CommandData::Connection(c) => {
            offset_command_lines(&mut c.first, delta, max_line);
            offset_command_lines(&mut c.second, delta, max_line);
        }
        CommandData::FunctionDef(c) => {
            offset_command_lines(std::sync::Arc::make_mut(&mut c.command), delta, max_line);
        }
        CommandData::Group(c) => {
            offset_command_lines(&mut c.command, delta, max_line);
        }
        CommandData::Select(c) => {
            offset_line(&mut c.line, delta, max_line);
            offset_command_lines(&mut c.action, delta, max_line);
        }
        CommandData::ArithFor(c) => {
            offset_command_lines(&mut c.action, delta, max_line);
        }
        CommandData::Subshell(c) => {
            offset_command_lines(&mut c.command, delta, max_line);
        }
        CommandData::Coproc(c) => {
            offset_command_lines(&mut c.command, delta, max_line);
        }
        CommandData::Simple(_) | CommandData::Arith(_) | CommandData::Cond(_) => {}
    }
}

fn offset_line(line: &mut u32, delta: u32, max_line: Option<u32>) {
    if *line > 0 {
        *line = clamp_line((*line).saturating_add(delta), max_line);
    }
}

fn clamp_line(line: u32, max_line: Option<u32>) -> u32 {
    max_line.map(|max| line.min(max)).unwrap_or(line)
}

fn report_parse_error(state: &ShellState, input_text: &str, err: &ParseError) {
    let offset = err.span.as_ref().map(|span| span.start).unwrap_or(0);
    let local_line = line_number_for_offset(input_text, offset).unwrap_or(1);
    let first_line = first_physical_line(state.current_command_line_count, input_text);
    let line_no = first_line.saturating_add(local_line).saturating_sub(1);

    if is_invalid_identifier_diagnostic(&err.message) {
        eprintln!("{}: line {}: {}", state.shell_name, line_no, err.message);
        return;
    }

    if err.message == "maximum here-document count exceeded" {
        eprintln!("{}: line {}: {}", state.input.name(), line_no, err.message);
        return;
    }

    if report_conditional_parse_error(state, input_text, line_no) {
        return;
    }

    if let Some((keyword, open_line, eof_line)) =
        incomplete_compound_command_eof(input_text, first_line, &err.message)
    {
        let prefix = syntax_error_prefix(state, eof_line);
        eprintln!(
            "{prefix}: syntax error: unexpected end of file from `{keyword}' command on line {open_line}"
        );
        return;
    }

    if err.message == "expected ')'" && input_text.trim_end().ends_with("EOF)") {
        let open_line = input_text
            .find('(')
            .and_then(|offset| line_number_for_offset(input_text, offset))
            .map(|line| first_line.saturating_add(line).saturating_sub(1))
            .unwrap_or(line_no);
        let prefix = syntax_error_prefix(state, line_no.saturating_add(1));
        eprintln!(
            "{prefix}: syntax error: unexpected end of file from `(' command on line {open_line}"
        );
        return;
    }

    if err
        .message
        .starts_with("unexpected EOF while looking for matching")
    {
        let prefix = syntax_error_prefix(state, line_no);
        eprintln!("{prefix}: {}", err.message);
        return;
    }

    if input_text.trim_start().starts_with("((") && input_text.contains("([))]") {
        let prefix = syntax_error_prefix(state, line_no);
        eprintln!("{prefix}: unexpected EOF while looking for matching `)'");
        return;
    }

    if let Some(token) = syntax_error_token(input_text, err) {
        let prefix = syntax_error_prefix(state, line_no);
        if token.contains("while looking for matching") {
            eprintln!("{prefix}: syntax error near unexpected token `{token}'");
        } else if input_text.contains("$(") && token != ")" {
            eprintln!(
                "{prefix}: syntax error near unexpected token `{token}' while looking for matching `)'"
            );
        } else {
            eprintln!("{prefix}: syntax error near unexpected token `{token}'");
        }
        if let Some(line) = source_line_for_offset(input_text, offset) {
            eprintln!("{prefix}: `{line}'");
        }
        return;
    }

    if input_text.contains("$(")
        && err
            .message
            .starts_with("syntax error near unexpected token `")
        && !err.message.contains("while looking for matching")
        && !err.message.contains("`)\'")
    {
        let prefix = syntax_error_prefix(state, line_no);
        eprintln!("{prefix}: {} while looking for matching `)'", err.message);
        if let Some(fragment) = syntax_error_fragment(input_text, err, offset) {
            eprintln!("{prefix}: `{fragment}'");
        }
        return;
    }

    if let Some(message) = bash_style_syntax_message(&err.message) {
        let prefix = syntax_error_prefix(state, line_no);
        eprintln!("{prefix}: {message}");
        if let Some(fragment) = syntax_error_fragment(input_text, err, offset) {
            eprintln!("{prefix}: syntax error: `{fragment}'");
        }
        return;
    }

    eprintln!(
        "{}: {}: {}",
        state.shell_name,
        state.input.name(),
        err.message,
    );
}

fn report_conditional_parse_error(state: &ShellState, input_text: &str, line_no: u32) -> bool {
    let command = input_text.trim();
    if !command.starts_with("[[") {
        return false;
    }

    let prefix = syntax_error_prefix(state, line_no);
    let line_prefix = || syntax_error_prefix(state, line_no);
    let next_prefix = || syntax_error_prefix(state, line_no.saturating_add(1));
    match command {
        "[[ ( -n xx" => {
            eprintln!("{prefix}: unexpected token `EOF', expected `)'");
            eprintln!(
                "{}: syntax error: unexpected end of file from `[[' command on line {line_no}",
                next_prefix()
            );
            true
        }
        "[[ ( -n xx )" => {
            eprintln!("{prefix}: unexpected EOF while looking for `]]'");
            eprintln!(
                "{}: syntax error: unexpected end of file from `[[' command on line {line_no}",
                next_prefix()
            );
            true
        }
        "[[ ( -t X ) ]" => {
            eprintln!("{prefix}: syntax error in conditional expression: unexpected token `]'");
            eprintln!("{prefix}: syntax error near `]'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ -n &" => {
            eprintln!("{prefix}: unexpected argument `&' to conditional unary operator");
            eprintln!("{prefix}: syntax error near `&'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ -n XX &" | "[[ -n XX & ]" => {
            eprintln!("{prefix}: syntax error in conditional expression: unexpected token `&'");
            eprintln!("{prefix}: syntax error near `&'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ 4 & ]]" => {
            eprintln!("{prefix}: unexpected token `&', conditional binary operator expected");
            eprintln!("{prefix}: syntax error near `&'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ 4 > & ]]" => {
            eprintln!("{prefix}: unexpected argument `&' to conditional binary operator");
            eprintln!("{prefix}: syntax error near `&'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ & ]]" => {
            eprintln!("{prefix}: unexpected token `&' in conditional command");
            eprintln!("{prefix}: syntax error near `&'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ -Q 7 ]]" => {
            eprintln!("{prefix}: unexpected token `7', conditional binary operator expected");
            eprintln!("{prefix}: syntax error near `7'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        "[[ -n < ]]" => {
            eprintln!("{prefix}: unexpected argument `<' to conditional unary operator");
            eprintln!("{prefix}: syntax error near `<'");
            eprintln!("{}: `{command}'", line_prefix());
            true
        }
        _ => false,
    }
}

fn report_top_level_heredoc_eof_warnings(state: &ShellState, input_text: &str) {
    let warnings = top_level_heredoc_eof_warnings(input_text);
    if warnings.is_empty() {
        return;
    }
    let first_line = first_physical_line(state.current_command_line_count, input_text);
    for warning in warnings {
        eprintln!(
            "{}: line {}: warning: here-document at line {} delimited by end-of-file (wanted `{}')",
            state.input.name(),
            first_line
                .saturating_add(warning.eof_line)
                .saturating_sub(1),
            first_line
                .saturating_add(warning.start_line)
                .saturating_sub(1),
            warning.delimiter,
        );
    }
}

struct TopLevelHeredocWarning {
    eof_line: u32,
    start_line: u32,
    delimiter: String,
}

struct PendingTopLevelHeredoc {
    delimiter: String,
    strip_tabs: bool,
    remove_escaped_newlines: bool,
    start_line: u32,
    delayed_by_command_subst: bool,
}

fn top_level_heredoc_eof_warnings(input_text: &str) -> Vec<TopLevelHeredocWarning> {
    let mut pending: std::collections::VecDeque<PendingTopLevelHeredoc> =
        std::collections::VecDeque::new();
    let mut warnings = Vec::new();
    let mut line_no = 1u32;
    let mut last_line = 1u32;
    let mut command_subst_depth = 0usize;
    let mut arithmetic_subst_depth = 0usize;
    let lines: Vec<&str> = input_text.split_inclusive('\n').collect();
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index].trim_end_matches('\n').trim_end_matches('\r');
        last_line = line_no;
        if !pending.is_empty() && command_subst_depth > 0 {
            let _ = top_level_heredocs_in_line_update_depth(
                line,
                &mut command_subst_depth,
                &mut arithmetic_subst_depth,
            );
            if command_subst_depth == 0 {
                for pending_doc in pending.iter_mut() {
                    if pending_doc.delayed_by_command_subst {
                        pending_doc.start_line = line_no;
                        pending_doc.delayed_by_command_subst = false;
                    }
                }
            }
            line_no = line_no.saturating_add(1);
            index += 1;
            continue;
        }

        if let Some(pending_doc) = pending.front() {
            let mut consumed = 1usize;
            let mut logical_line = line.to_string();
            if pending_doc.remove_escaped_newlines {
                while logical_line.ends_with('\\') {
                    logical_line.pop();
                    let Some(next) = lines.get(index + consumed) else {
                        break;
                    };
                    let next = next.trim_end_matches('\n').trim_end_matches('\r');
                    logical_line.push_str(next);
                    consumed += 1;
                    last_line = line_no.saturating_add(consumed as u32).saturating_sub(1);
                }
            }
            let candidate = if pending_doc.strip_tabs {
                logical_line.trim_start_matches('\t')
            } else {
                logical_line.as_str()
            };
            if candidate == pending_doc.delimiter {
                pending.pop_front();
            }
            line_no = line_no.saturating_add(consumed as u32);
            index += consumed;
            continue;
        }

        if !line.ends_with('\\') && !line.contains('`') {
            let found = top_level_heredocs_in_line_update_depth(
                line,
                &mut command_subst_depth,
                &mut arithmetic_subst_depth,
            );
            let delayed_by_command_subst = command_subst_depth > 0;
            for (delimiter, strip_tabs, remove_escaped_newlines) in found {
                pending.push_back(PendingTopLevelHeredoc {
                    delimiter,
                    strip_tabs,
                    remove_escaped_newlines,
                    start_line: line_no,
                    delayed_by_command_subst,
                });
            }
        } else {
            command_subst_depth =
                command_subst_depth_after_line_for_heredoc_probe(line, command_subst_depth);
        }
        line_no = line_no.saturating_add(1);
        index += 1;
    }

    warnings.extend(
        pending
            .into_iter()
            .map(|pending_doc| TopLevelHeredocWarning {
                eof_line: last_line,
                start_line: pending_doc.start_line,
                delimiter: pending_doc.delimiter,
            }),
    );
    warnings
}

fn is_invalid_identifier_diagnostic(message: &str) -> bool {
    message.ends_with("': not a valid identifier") || message.ends_with("`: not a valid identifier")
}

fn syntax_error_prefix(state: &ShellState, line_no: u32) -> String {
    if state.startup_state == crate::state::StartupMode::DashC {
        format!("{}: -c: line {}", state.shell_name, line_no)
    } else {
        format!("{}: line {}", state.shell_name, line_no)
    }
}

fn bash_style_syntax_message(message: &str) -> Option<String> {
    if message == "syntax error near unexpected token `;'" {
        Some("syntax error: `;' unexpected".to_string())
    } else if message.starts_with("syntax error:") {
        Some(message.to_string())
    } else {
        None
    }
}

fn syntax_error_fragment(input_text: &str, err: &ParseError, offset: usize) -> Option<String> {
    if let Some(span) = &err.span {
        if span.start < span.end && span.end <= input_text.len() {
            let fragment = input_text[span.start..span.end]
                .trim_end_matches('\r')
                .trim();
            if !fragment.is_empty() {
                return Some(fragment.to_string());
            }
        }
    }
    source_line_for_offset(input_text, offset)
}

fn syntax_error_token(input_text: &str, err: &ParseError) -> Option<String> {
    if let Some(token) = err
        .message
        .strip_prefix("unexpected token '")
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Some(token.to_string());
    }

    let is_expected_word_error = err.message.starts_with("expected '")
        || matches!(
            err.message.as_str(),
            "expected for name"
                | "expected command"
                | "expected list"
                | "function body must be a compound command"
        );
    if !is_expected_word_error {
        return None;
    }

    err.span.as_ref().and_then(|span| {
        input_text
            .get(span.start.min(input_text.len())..span.end.min(input_text.len()))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn line_number_for_offset(input_text: &str, offset: usize) -> Option<u32> {
    if input_text.is_empty() {
        return Some(1);
    }
    let end = diagnostic_offset(input_text, offset);
    Some(1 + input_text[..end].bytes().filter(|b| *b == b'\n').count() as u32)
}

fn source_line_for_offset(input_text: &str, offset: usize) -> Option<String> {
    if input_text.is_empty() {
        return None;
    }
    let offset = diagnostic_offset(input_text, offset);
    let start = input_text[..offset]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let end = input_text[offset..]
        .find('\n')
        .map(|idx| offset + idx)
        .unwrap_or(input_text.len());
    let line = input_text[start..end].trim_end_matches('\r');
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn incomplete_compound_command_eof(
    input_text: &str,
    first_line: u32,
    message: &str,
) -> Option<(&'static str, u32, u32)> {
    let trimmed = input_text.trim_start();
    let keyword = if message == "expected 'fi'" && starts_with_reserved(trimmed, "if") {
        "if"
    } else if message == "expected 'done'" && starts_with_reserved(trimmed, "while") {
        "while"
    } else if message == "expected 'done'" && starts_with_reserved(trimmed, "until") {
        "until"
    } else if message == "expected 'done'" && starts_with_reserved(trimmed, "for") {
        "for"
    } else if message == "expected ')' after case pattern" && starts_with_reserved(trimmed, "case")
    {
        "case"
    } else {
        return None;
    };
    let open_offset = input_text.len().saturating_sub(trimmed.len());
    let open_line = line_number_for_offset(input_text, open_offset)
        .map(|line| first_line.saturating_add(line).saturating_sub(1))
        .unwrap_or(first_line);
    let local_eof_line = input_text.bytes().filter(|byte| *byte == b'\n').count() as u32
        + if input_text.ends_with('\n') { 1 } else { 2 };
    let eof_line = first_line.saturating_add(local_eof_line).saturating_sub(1);
    Some((keyword, open_line, eof_line))
}

fn starts_with_reserved(input: &str, keyword: &str) -> bool {
    input == keyword
        || input
            .strip_prefix(keyword)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|ch| ch.is_whitespace() || matches!(ch, ';' | '\n'))
}

fn diagnostic_offset(input_text: &str, offset: usize) -> usize {
    let mut offset = offset.min(input_text.len());
    if offset == input_text.len() && offset > 0 && input_text.as_bytes()[offset - 1] == b'\n' {
        offset -= 1;
    }
    offset
}
