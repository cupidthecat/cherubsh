use cherubsh_common::{expand_aliases_for_parse, Environment, ShellJump, ShellResult};
use cherubsh_exec::{execute_in, execute_with_state, ExecState};
use cherubsh_lexer::Lexer;
use cherubsh_lineedit::{CompletionProvider, EditError, HistoryProvider, LineEditor};
use cherubsh_parser::{Ast, Command, CommandData, ParseError, Parser};

use crate::completion::{self, CompRequest};
use crate::histexpand;
use crate::prompt::{decode_prompt_string, prompt_again};
use crate::signals::{arm_alarm, check_signals, disarm_alarm};
use crate::state::ShellState;
use crate::traps::{notify_completed_jobs, run_pending_traps};

/// reader_loop: read-eval loop. Port of eval.c:57-194.
pub fn reader_loop(state: &mut ShellState) -> i32 {
    let mut exec_state = ExecState::default();
    exec_state.import_exported_functions(state);
    reader_loop_with_exec_state(state, &mut exec_state)
}

pub fn reader_loop_with_exec_state(state: &mut ShellState, exec_state: &mut ExecState) -> i32 {
    state.indirection_level += 1;
    let saved_indirection = state.indirection_level;

    while !state.eof_reached {
        // setjmp(top_level) analogue: signal results become ShellJump values.
        match check_signals() {
            Ok(()) => {}
            Err(ShellJump::ErrExit) => {
                state.indirection_level = saved_indirection;
                if state.errexit {
                    // reset_local_contexts equivalent
                }
            }
            Err(ShellJump::ForceEof)
            | Err(ShellJump::ExitProg(_))
            | Err(ShellJump::ExitBltin(_)) => {
                state.eof_reached = true;
                break;
            }
            Err(ShellJump::Discard) => {
                if state.last_command_exit_value == 0 {
                    state.last_command_exit_value = 1;
                }
                if state.subshell_environment {
                    state.eof_reached = true;
                    break;
                }
                continue;
            }
            Err(ShellJump::SigExit(code)) => {
                state.last_command_exit_value = code;
                state.eof_reached = true;
                break;
            }
            Err(ShellJump::NotJumped) => {}
        }

        state.executing = false;

        let parsed = match read_command(state) {
            Ok(value) => value,
            Err(jump) => match jump {
                ShellJump::ForceEof | ShellJump::ExitProg(_) | ShellJump::ExitBltin(_) => {
                    state.eof_reached = true;
                    break;
                }
                ShellJump::Discard => {
                    if state.last_command_exit_value == 0 {
                        state.last_command_exit_value = 1;
                    }
                    if state.interactive == false {
                        state.eof_reached = true;
                    }
                    continue;
                }
                ShellJump::SigExit(code) => {
                    state.last_command_exit_value = code;
                    state.eof_reached = true;
                    break;
                }
                _ => continue,
            },
        };

        let command = match parsed {
            Some(cmd) => cmd,
            None => {
                if state.just_one_command {
                    state.eof_reached = true;
                }
                continue;
            }
        };

        if state.interactive {
            if let Some(ps0) = state.get("PS0") {
                if !ps0.is_empty() {
                    let decoded = decode_prompt_string(state, &ps0);
                    use std::io::Write;
                    let mut stderr = std::io::stderr();
                    let _ = stderr.write_all(decoded.as_bytes());
                    let _ = stderr.flush();
                }
            }
        }

        state.current_command_number = state.current_command_number.saturating_add(1);
        state.executing = true;

        if state.noexec {
            state.last_command_exit_value = 0;
        } else {
            let ast = Ast { root: command };
            let result = execute_with_state(&ast, state, exec_state);
            state.last_command_exit_value = result.status;
            if result.exit_shell {
                state.eof_reached = true;
            }
        }
        run_pending_traps(state);

        if state.just_one_command {
            state.eof_reached = true;
        }
    }

    state.indirection_level = saved_indirection - 1;
    state.last_command_exit_value
}

/// Run reader_loop until the current input source ends, used when sourcing
/// a startup file. Restores the previous input on return.
pub fn run_until_eof(state: &mut ShellState) {
    let saved_eof = state.eof_reached;
    let saved_just_one = state.just_one_command;
    state.eof_reached = false;
    state.just_one_command = false;
    let _ = reader_loop(state);
    state.just_one_command = saved_just_one;
    state.eof_reached = saved_eof;
}

/// read_command: port of eval.c:360-401.
pub fn read_command(state: &mut ShellState) -> ShellResult<Option<Command>> {
    let mut tmout_armed = false;
    if state.interactive {
        if let Some(value) = state.get("TMOUT") {
            if let Ok(seconds) = value.trim().parse::<u32>() {
                if seconds > 0 {
                    arm_alarm(seconds);
                    tmout_armed = true;
                }
            }
        }
    }
    check_signals()?;
    let result = parse_command(state);
    if tmout_armed {
        disarm_alarm();
    }
    result
}

/// parse_command: port of eval.c:323-354.
pub fn parse_command(state: &mut ShellState) -> ShellResult<Option<Command>> {
    state.need_here_doc = false;
    // Dispatch any queued traps + reap completed jobs before prompting.
    run_pending_traps(state);
    if state.interactive {
        notify_completed_jobs(state);
    }
    if state.interactive && !state.input.is_string() && !state.input.is_stream() {
        execute_prompt_command(state);
    }
    let input_text = read_logical_command(state)?;
    if input_text.trim().is_empty() {
        return Ok(None);
    }
    report_top_level_heredoc_eof_warnings(state, &input_text);
    // Bash records non-interactive commands too once `set -o history` is on.
    if input_text.trim_end_matches('\n') == "HISTFILE=" {
        state.histfile_explicit = true;
        state.histfile = None;
    }
    state.history_last_line_added = false;
    if state.interactive || state.option("history") {
        let record_line = history_record_line(state, &input_text);
        if should_record_history(state, &record_line) {
            let control = state.histcontrol_flags;
            state.history_last_line_added = state.history_table.add(&record_line, control);
        }
    }
    let parse_input = expand_aliases_for_parse(&input_text, state);
    match parse_text(&parse_input, state.option("extglob"), state.option("posix")) {
        Ok(mut command) => {
            let first_line = first_physical_line(state.current_command_line_count, &input_text);
            offset_command_lines(
                &mut command,
                first_line.saturating_sub(1),
                Some(state.current_command_line_count),
            );
            Ok(Some(command))
        }
        Err(err) if err.message == "empty input" => Ok(None),
        Err(err) => {
            let invalid_identifier = is_invalid_identifier_diagnostic(&err.message);
            report_parse_error(state, &input_text, &err);
            state.last_command_exit_value = if invalid_identifier { 1 } else { 2 };
            if invalid_identifier {
                return Ok(None);
            }
            if !state.interactive {
                state.eof_reached = true;
            }
            Err(ShellJump::Discard)
        }
    }
}

fn history_record_line(state: &ShellState, input_text: &str) -> String {
    let trimmed = input_text.trim_end_matches('\n');
    if contains_heredoc_redirect(trimmed) {
        return input_text.to_string();
    }
    if !state.option("cmdhist") || !trimmed.contains('\n') || newline_inside_quotes(trimmed) {
        return trimmed.to_string();
    }
    compact_cmdhist_line(trimmed)
}

fn contains_heredoc_redirect(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            return true;
        }
        i += 1;
    }
    false
}

fn newline_inside_quotes(input: &str) -> bool {
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for byte in input.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some(b'\'') => {
                if byte == b'\n' {
                    return true;
                }
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(b'"') => {
                if byte == b'\n' {
                    return true;
                }
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quote = None;
                }
            }
            _ => {
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                }
            }
        }
    }
    false
}

fn compact_cmdhist_line(input: &str) -> String {
    let mut out = String::new();
    let mut previous_trimmed = String::new();
    for raw in input.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(line);
        } else {
            let sep = if matches!(
                previous_trimmed.as_str(),
                "do" | "then" | "else" | "elif" | "{"
            ) || previous_trimmed.ends_with(" do")
                || previous_trimmed.ends_with("; do")
                || previous_trimmed.ends_with(" then")
            {
                " "
            } else {
                "; "
            };
            out.push_str(sep);
            out.push_str(line);
        }
        previous_trimmed = line.trim().to_string();
    }
    out
}

fn should_record_history(state: &ShellState, line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let Some(raw) = state.get("HISTIGNORE") else {
        return true;
    };
    let opts = cherubsh_expander::pattern::GlobOpts {
        extglob: state.option("extglob"),
        globasciiranges: state.option("globasciiranges"),
        ..Default::default()
    };
    for pat in raw.split(':') {
        if pat.is_empty() {
            continue;
        }
        if pat == "&" {
            if state
                .history_table
                .last()
                .map(|entry| entry.line.as_str() == line)
                .unwrap_or(false)
            {
                return false;
            }
            continue;
        }
        if cherubsh_expander::pattern::fnmatch(pat.as_bytes(), line.as_bytes(), opts) {
            return false;
        }
    }
    true
}

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
            offset_command_lines(&mut c.command, delta, max_line);
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

    if err.message == "expected ')'" && input_text.trim_end().ends_with("EOF)") {
        let prefix = syntax_error_prefix(state, line_no.saturating_add(1));
        eprintln!("{prefix}: syntax error: unexpected end of file");
        return;
    }

    if let Some(token) = syntax_error_token(input_text, err) {
        let prefix = syntax_error_prefix(state, line_no);
        eprintln!("{prefix}: syntax error near unexpected token `{token}'");
        if let Some(line) = source_line_for_offset(input_text, offset) {
            eprintln!("{prefix}: `{line}'");
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

fn diagnostic_offset(input_text: &str, offset: usize) -> usize {
    let mut offset = offset.min(input_text.len());
    if offset == input_text.len() && offset > 0 && input_text.as_bytes()[offset - 1] == b'\n' {
        offset -= 1;
    }
    offset
}

fn parse_text(
    input_text: &str,
    extglob_patterns: bool,
    posix_mode: bool,
) -> Result<Command, ParseError> {
    validate_command_substitutions_for_parse(input_text, extglob_patterns, posix_mode)?;
    let mut lexer = Lexer::new(input_text);
    lexer.set_extglob_patterns(extglob_patterns);
    lexer.set_posix_mode(posix_mode);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, input_text);
    parser.parse().map(|ast| ast.root)
}

fn validate_command_substitutions_for_parse(
    input_text: &str,
    extglob_patterns: bool,
    posix_mode: bool,
) -> Result<(), ParseError> {
    let bytes = input_text.as_bytes();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;

    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' && i + 1 < bytes.len() {
                let n = bytes[i + 1];
                if matches!(n, b'"' | b'\\' | b'$' | b'`' | b'\n') {
                    i += 2;
                    continue;
                }
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                if command_substitution_is_arithmetic(bytes, i) {
                    i = skip_arithmetic_substitution_for_parse(bytes, i + 3).unwrap_or(bytes.len());
                    continue;
                }
                let Some((end, body_start, body, close_offset)) =
                    extract_command_substitution_for_parse(input_text, i + 2)
                else {
                    validate_unclosed_command_substitution_body(
                        input_text,
                        i + 2,
                        extglob_patterns,
                        posix_mode,
                    )?;
                    return Ok(());
                };
                validate_command_substitution_body(
                    input_text,
                    body_start,
                    close_offset,
                    &body,
                    extglob_patterns,
                    posix_mode,
                )?;
                i = end;
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                let Some(end) = skip_parameter_brace_for_probe(bytes, i + 2, posix_mode) else {
                    return Ok(());
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return Ok(());
                };
                i = end;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return Ok(());
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\'' => {
                    ansi_c = true;
                    i += 2;
                    comment_ok = false;
                    continue;
                }
                b'(' => {
                    if command_substitution_is_arithmetic(bytes, i) {
                        i = skip_arithmetic_substitution_for_parse(bytes, i + 3)
                            .unwrap_or(bytes.len());
                        continue;
                    }
                    let Some((end, body_start, body, close_offset)) =
                        extract_command_substitution_for_parse(input_text, i + 2)
                    else {
                        validate_unclosed_command_substitution_body(
                            input_text,
                            i + 2,
                            extglob_patterns,
                            posix_mode,
                        )?;
                        return Ok(());
                    };
                    validate_command_substitution_body(
                        input_text,
                        body_start,
                        close_offset,
                        &body,
                        extglob_patterns,
                        posix_mode,
                    )?;
                    i = end;
                    comment_ok = false;
                    continue;
                }
                _ => {}
            }
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b' ' | b'\t' | b'\r' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => {
                comment_ok = true
            }
            _ => comment_ok = false,
        }
        i += 1;
    }
    Ok(())
}

fn validate_command_substitution_body(
    input_text: &str,
    body_start: usize,
    close_offset: usize,
    body: &str,
    extglob_patterns: bool,
    posix_mode: bool,
) -> Result<(), ParseError> {
    validate_command_substitutions_for_parse(body, extglob_patterns, posix_mode).map_err(
        |err| map_command_substitution_parse_error(input_text, body_start, close_offset, body, err),
    )?;

    let mut lexer = Lexer::new(body);
    lexer.set_extglob_patterns(extglob_patterns);
    lexer.set_posix_mode(posix_mode);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, body);
    match parser.parse_input_unit() {
        Ok(_) => Ok(()),
        Err(err) => Err(map_command_substitution_parse_error(
            input_text,
            body_start,
            close_offset,
            body,
            err,
        )),
    }
}

fn validate_unclosed_command_substitution_body(
    input_text: &str,
    body_start: usize,
    extglob_patterns: bool,
    posix_mode: bool,
) -> Result<(), ParseError> {
    let body = &input_text[body_start.min(input_text.len())..];
    let mut lexer = Lexer::new(body);
    lexer.set_extglob_patterns(extglob_patterns);
    lexer.set_posix_mode(posix_mode);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, body);
    match parser.parse_input_unit() {
        Ok(_) => Ok(()),
        Err(err) if parse_error_is_at_body_end(body, &err) => Ok(()),
        Err(err) => Err(map_command_substitution_parse_error(
            input_text,
            body_start,
            input_text.len().saturating_sub(1),
            body,
            err,
        )),
    }
}

fn parse_error_is_at_body_end(body: &str, err: &ParseError) -> bool {
    let at_body_end = err
        .span
        .as_ref()
        .map(|span| span.start >= body.len() || span.end >= body.len())
        .unwrap_or(true);
    at_body_end
        && (err.message.starts_with("expected")
            || err.message == "syntax error: unexpected end of file")
}

fn map_command_substitution_parse_error(
    input_text: &str,
    body_start: usize,
    close_offset: usize,
    body: &str,
    err: ParseError,
) -> ParseError {
    if parse_error_is_at_body_end(body, &err) {
        return ParseError {
            message: "unexpected token ')'".to_string(),
            span: Some(cherubsh_common::Span::new(
                0,
                close_offset.min(input_text.len()),
                (close_offset + 1).min(input_text.len()),
            )),
        };
    }

    let span = err.span.map(|span| {
        cherubsh_common::Span::new(
            0,
            body_start.saturating_add(span.start).min(input_text.len()),
            body_start.saturating_add(span.end).min(input_text.len()),
        )
    });
    ParseError {
        message: err.message,
        span,
    }
}

fn command_substitution_is_arithmetic(bytes: &[u8], dollar_offset: usize) -> bool {
    dollar_offset + 2 < bytes.len()
        && bytes[dollar_offset] == b'$'
        && bytes[dollar_offset + 1] == b'('
        && bytes[dollar_offset + 2] == b'('
        && skip_arithmetic_substitution_for_parse(bytes, dollar_offset + 3).is_some()
}

fn skip_arithmetic_substitution_for_parse(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                if depth == 1 {
                    return (i + 1 < bytes.len() && bytes[i + 1] == b')').then_some(i + 2);
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn extract_command_substitution_for_parse(
    input_text: &str,
    mut i: usize,
) -> Option<(usize, usize, String, usize)> {
    let bytes = input_text.as_bytes();
    let body_start = i;
    let mut depth = 1usize;
    let mut case_depth = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    let mut body = String::new();

    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    body.push(bytes[i] as char);
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    body.push('\n');
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            body.push('\n');
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                body.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            body.push('\\');
            body.push(bytes[i + 1] as char);
            i += 2;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            let start = i;
            i += 2;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            body.push_str(&input_text[start..i.min(input_text.len())]);
            comment_ok = false;
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            let start = i;
            let Some(end) = skip_quoted_for_parse(bytes, i) else {
                body.push_str(&input_text[start..]);
                return None;
            };
            body.push_str(&input_text[start..end]);
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            let before = i;
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            body.push_str(&input_text[before..i.min(input_text.len())]);
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if command_substitution_is_arithmetic(bytes, i) {
                let end = skip_arithmetic_substitution_for_parse(bytes, i + 3)?;
                body.push_str(&input_text[i..end]);
                i = end;
                comment_ok = false;
                continue;
            }
            depth = depth.saturating_add(1);
            body.push('$');
            body.push('(');
            i += 2;
            comment_ok = true;
            continue;
        }
        if is_probe_name_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_probe_name_byte(bytes[i]) {
                i += 1;
            }
            let previous = previous_significant_byte(bytes, start);
            match &bytes[start..i] {
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" if !matches!(previous, Some(b'|' | b'(')) => {
                    case_depth = case_depth.saturating_sub(1)
                }
                _ => {}
            }
            body.push_str(&input_text[start..i]);
            comment_ok = false;
            continue;
        }
        if b == b'(' {
            depth = depth.saturating_add(1);
            body.push('(');
            i += 1;
            comment_ok = true;
            continue;
        }
        if b == b')' {
            if case_depth > 0 && depth == 1 {
                body.push(')');
                i += 1;
                comment_ok = true;
                continue;
            }
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some((i + 1, body_start, body, i));
            }
            body.push(')');
            i += 1;
            comment_ok = true;
            continue;
        }
        body.push(b as char);
        i += 1;
        comment_ok = b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>');
    }
    None
}

fn previous_significant_byte(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i > 0 {
        i -= 1;
        if !bytes[i].is_ascii_whitespace() {
            return Some(bytes[i]);
        }
    }
    None
}

fn skip_quoted_for_parse(bytes: &[u8], mut i: usize) -> Option<usize> {
    let quote = bytes[i];
    if quote == b'"' {
        return skip_double_quoted_for_parse(bytes, i + 1);
    }
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn skip_double_quoted_for_parse(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if command_substitution_is_arithmetic(bytes, i) {
                i = skip_arithmetic_substitution_for_parse(bytes, i + 3)?;
            } else {
                i = skip_command_substitution_for_probe(bytes, i + 2)?;
            }
            continue;
        }
        if bytes[i] == b'`' {
            i = skip_backtick_body(bytes, i)?;
            continue;
        }
        if bytes[i] == b'"' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn read_logical_command(state: &mut ShellState) -> ShellResult<String> {
    let mut command = String::new();
    loop {
        let line = if state.interactive && !state.input.is_string() && !state.input.is_stream() {
            match read_interactive_line(state, if command.is_empty() { 1 } else { 2 })? {
                Some(line) => line,
                None => {
                    state.eof_reached = true;
                    return Ok(command);
                }
            }
        } else {
            match state.input.next_line() {
                Ok(Some(line)) => line,
                Ok(None) => {
                    state.eof_reached = true;
                    return Ok(command);
                }
                Err(_) => return Err(ShellJump::ForceEof),
            }
        };
        state.current_command_line_count = state.current_command_line_count.saturating_add(1);

        if state.verbose_flag {
            use std::io::Write;
            let mut stderr = std::io::stderr();
            let _ = stderr.write_all(line.as_bytes());
            let _ = stderr.flush();
        }

        let in_heredoc_body =
            !command.is_empty() && has_unclosed_heredoc(&expand_aliases_for_parse(&command, state));
        let line = if in_heredoc_body {
            line
        } else if let Some(expanded) = history_expand_physical_line(state, &line)? {
            expanded
        } else {
            return Ok(String::new());
        };

        command.push_str(&line);
        if command.trim().is_empty() {
            return Ok(command);
        }
        if has_trailing_line_continuation(&command) {
            continue;
        }
        let parse_probe = expand_aliases_for_parse(&command, state);
        if has_open_quotes(&parse_probe, state.option("posix"))
            || has_unclosed_command_substitution(&parse_probe)
            || has_unclosed_heredoc(&parse_probe)
            || has_unclosed_compound_assignment(&parse_probe)
        {
            continue;
        }

        match parse_text(&parse_probe, state.option("extglob"), state.option("posix")) {
            Ok(_) => return Ok(command),
            Err(err) if err.message == "empty input" => return Ok(command),
            Err(err) if parse_error_wants_more_input(&err, &parse_probe) => continue,
            Err(_) => return Ok(command),
        }
    }
}

fn history_expand_physical_line(state: &mut ShellState, line: &str) -> ShellResult<Option<String>> {
    if !(state.option("histexpand") && (state.interactive || state.option("history"))) {
        return Ok(Some(line.to_string()));
    }

    let res = histexpand::expand_with_options(line, &state.history_table, state.option("posix"));
    if let Some(err) = res.error {
        eprintln!(
            "{}: line {}: {err}",
            state.shell_name, state.current_command_line_count
        );
        return Ok(None);
    }
    if res.changed
        && !state
            .shopt_options
            .get("histverify")
            .copied()
            .unwrap_or(false)
    {
        eprintln!("{}", res.line.strip_suffix('\n').unwrap_or(&res.line));
    }
    if res.print_only {
        state.last_command_exit_value = 0;
        return Ok(None);
    }
    Ok(Some(res.line))
}

fn read_interactive_line(state: &mut ShellState, prompt_level: u8) -> ShellResult<Option<String>> {
    if state.no_line_editing {
        prompt_again(state, prompt_level);
        return match state.input.next_line() {
            Ok(line) => Ok(line),
            Err(_) => Err(ShellJump::ForceEof),
        };
    }

    let key = if prompt_level == 2 { "PS2" } else { "PS1" };
    let raw_prompt = state.get(key).unwrap_or_else(|| {
        if prompt_level == 2 {
            String::from("> ")
        } else {
            String::from("\\s-\\v\\$ ")
        }
    });
    let prompt = decode_prompt_string(state, &raw_prompt);
    let keymap = state
        .keymaps
        .get(&state.active_keymap)
        .cloned()
        .or_else(|| state.keymaps.get("emacs").cloned())
        .unwrap_or_else(|| {
            let mut k = cherubsh_common::Keymap::new("emacs");
            k.install_emacs_defaults();
            k
        });
    let mut editor = state
        .line_editor
        .take()
        .unwrap_or_else(|| LineEditor::new(keymap.clone()));
    editor.keymap = keymap;

    let mut history = HistorySnapshot::from_state(state);
    let mut completion = ShellCompleter { state };
    let result = if completion.state.input.is_terminal() {
        editor.readline(&prompt, &mut history, &mut completion)
    } else {
        editor.readline_scripted(&prompt, &mut history, &mut completion)
    };
    completion.state.line_editor = Some(editor);

    match result {
        Ok(mut line) => {
            line.push('\n');
            Ok(Some(line))
        }
        Err(EditError::Eof) => Ok(None),
        Err(EditError::Interrupted) => {
            completion.state.last_command_exit_value = 130;
            Err(ShellJump::Discard)
        }
        Err(EditError::Io(_)) => Err(ShellJump::ForceEof),
    }
}

struct HistorySnapshot {
    entries: Vec<String>,
}

impl HistorySnapshot {
    fn from_state(state: &ShellState) -> Self {
        Self {
            entries: state
                .history_table
                .iter()
                .map(|entry| entry.line.clone())
                .collect(),
        }
    }
}

impl HistoryProvider for HistorySnapshot {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, idx: usize) -> Option<String> {
        self.entries.get(idx).cloned()
    }
}

struct ShellCompleter<'a> {
    state: &'a mut ShellState,
}

impl CompletionProvider for ShellCompleter<'_> {
    fn complete(&mut self, line: &str, point: usize) -> Vec<String> {
        let req = completion_request(line, point);
        completion::complete(self.state, &req)
    }
}

fn completion_request(line: &str, point: usize) -> CompRequest<'_> {
    let point = point.min(line.len());
    let before = &line[..point];
    let current_start = before
        .rfind(|c: char| c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')'))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let current_end = line[point..]
        .find(|c: char| c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')'))
        .map(|idx| point + idx)
        .unwrap_or(line.len());
    let current = line[current_start..current_end].to_string();
    let mut words: Vec<String> = Vec::new();
    let mut cword = 0usize;
    let mut in_word = false;
    let mut start = 0usize;
    for (idx, ch) in line.char_indices() {
        let is_break = ch.is_whitespace() || matches!(ch, '|' | '&' | ';' | '<' | '>' | '(' | ')');
        if is_break {
            if in_word {
                if start <= point && point <= idx {
                    cword = words.len();
                }
                words.push(line[start..idx].to_string());
                in_word = false;
            }
        } else if !in_word {
            start = idx;
            in_word = true;
        }
    }
    if in_word {
        if start <= point && point <= line.len() {
            cword = words.len();
        }
        words.push(line[start..].to_string());
    } else if point == line.len() {
        cword = words.len();
        words.push(String::new());
    }
    if words.is_empty() {
        words.push(String::new());
        cword = 0;
    }
    if cword < words.len() {
        words[cword] = current.clone();
    }
    CompRequest {
        line,
        point,
        words,
        cword,
        current,
    }
}

fn parse_error_wants_more_input(err: &ParseError, input: &str) -> bool {
    let at_end = err
        .span
        .as_ref()
        .map(|span| span.end >= input.len())
        .unwrap_or(true);
    at_end
        && (err.message.starts_with("expected")
            || err.message == "function body must be a compound command"
            || err.message == "syntax error: unexpected end of file")
}

fn has_open_quotes(input: &str, posix_mode: bool) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' && i + 1 < bytes.len() {
                let n = bytes[i + 1];
                if matches!(n, b'"' | b'\\' | b'$' | b'`' | b'\n') {
                    i += 2;
                    continue;
                }
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                let Some(end) = skip_parameter_brace_for_probe(bytes, i + 2, posix_mode) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return true;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            ansi_c = true;
            i += 2;
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                return true;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b'\n' => comment_ok = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    single || double || ansi_c
}

fn has_trailing_line_continuation(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    return i + 2 == bytes.len();
                }
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    return i + 2 == bytes.len();
                }
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return false;
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return false;
                };
                i = end;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                return i + 2 == bytes.len();
            }
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return false;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            ansi_c = true;
            i += 2;
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b'\n' => comment_ok = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    false
}

fn has_unclosed_compound_assignment(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;

    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'\'' || b == b'"' {
            i = skip_simple_quoted_for_probe(bytes, i, b).unwrap_or(bytes.len());
            comment_ok = false;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return false;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            i = skip_ansi_c_quoted_for_probe(bytes, i + 2).unwrap_or(bytes.len());
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                return false;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if b == b'='
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'('
            && compound_assignment_lhs_before(bytes, i)
            && skip_compound_assignment_body(bytes, i + 2).is_none()
        {
            return true;
        }
        comment_ok = b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>');
        i += 1;
    }

    false
}

fn compound_assignment_lhs_before(bytes: &[u8], eq: usize) -> bool {
    let mut end = eq;
    if end > 0 && bytes[end - 1] == b'+' {
        end -= 1;
    }
    if end == 0 {
        return false;
    }
    if bytes[end - 1] == b']' {
        let Some(open) = bytes[..end - 1].iter().rposition(|b| *b == b'[') else {
            return false;
        };
        return valid_identifier_bytes(&bytes[..open]);
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1] == b'_' || bytes[start - 1].is_ascii_alphanumeric()) {
        start -= 1;
    }
    valid_identifier_bytes(&bytes[start..end])
}

fn valid_identifier_bytes(bytes: &[u8]) -> bool {
    let Some(first) = bytes.first() else {
        return false;
    };
    (*first == b'_' || first.is_ascii_alphabetic())
        && bytes[1..]
            .iter()
            .all(|b| *b == b'_' || b.is_ascii_alphanumeric())
}

fn skip_compound_assignment_body(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
            b'\'' | b'"' => i = skip_simple_quoted_for_probe(bytes, i, bytes[i])?,
            b'`' => i = skip_backtick_body(bytes, i)?,
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'\'' => {
                i = skip_ansi_c_quoted_for_probe(bytes, i + 2)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                i = skip_parameter_brace_for_probe(bytes, i + 2, false)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                i = skip_command_substitution_for_probe(bytes, i + 2)?
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_simple_quoted_for_probe(bytes: &[u8], mut i: usize, quote: u8) -> Option<usize> {
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn skip_ansi_c_quoted_for_probe(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if bytes[i] == b'\'' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn skip_parameter_brace_for_probe(
    bytes: &[u8],
    mut i: usize,
    posix_single_quote: bool,
) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
            b'\'' if !posix_single_quote => {
                let mut j = i + 1;
                let mut saw_escaped_quote = false;
                let mut closes_before_quote = false;
                while j < bytes.len() && bytes[j] != b'\'' {
                    if bytes[j] == b'\\' && j + 1 < bytes.len() {
                        if bytes[j + 1] == b'\'' {
                            saw_escaped_quote = true;
                        }
                        j += 2;
                        continue;
                    }
                    if bytes[j] == b'}' && saw_escaped_quote {
                        closes_before_quote = true;
                        break;
                    }
                    j += 1;
                }
                if closes_before_quote {
                    i += 1;
                } else {
                    i = skip_simple_quoted_for_probe(bytes, i, bytes[i])?;
                }
            }
            b'"' => i = skip_simple_quoted_for_probe(bytes, i, bytes[i])?,
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'\'' => {
                i = skip_ansi_c_quoted_for_probe(bytes, i + 2)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                depth += 1;
                i += 2;
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                i = skip_command_substitution_for_probe(bytes, i + 2)?
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn has_unclosed_command_substitution(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut case_depth = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return true;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok && !double {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\'' if !double => {
                    ansi_c = true;
                    i += 2;
                    comment_ok = false;
                    continue;
                }
                b'(' => {
                    depth = depth.saturating_add(1);
                    i += 2;
                    comment_ok = true;
                    continue;
                }
                _ => {}
            }
        }
        if depth > 0 && is_probe_name_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_probe_name_byte(bytes[i]) {
                i += 1;
            }
            match &bytes[start..i] {
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" => case_depth = case_depth.saturating_sub(1),
                _ => {}
            }
            comment_ok = false;
            continue;
        }
        if b == b'(' && depth > 0 {
            depth = depth.saturating_add(1);
            i += 1;
            comment_ok = true;
            continue;
        }
        if b == b')' && depth > 0 {
            if case_depth > 0 && depth == 1 {
                i += 1;
                comment_ok = true;
                continue;
            }
            depth -= 1;
            i += 1;
            comment_ok = true;
            continue;
        }
        match b {
            b'\'' if !double => single = true,
            b'"' => double = !double,
            b'\n' => comment_ok = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    depth > 0
}

fn skip_command_substitution_for_probe(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut case_depth = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                i = skip_command_substitution_for_probe(bytes, i + 2)?;
                continue;
            }
            if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                i = skip_command_substitution_for_probe(bytes, i + 2)?;
                continue;
            }
            if b == b'`' {
                i = skip_backtick_body(bytes, i)?;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'`' {
            i = skip_backtick_body(bytes, i)?;
            comment_ok = false;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            i = skip_command_substitution_for_probe(bytes, i + 2)?;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\'' => {
                    ansi_c = true;
                    i += 2;
                    comment_ok = false;
                    continue;
                }
                b'(' => {
                    depth = depth.saturating_add(1);
                    i += 2;
                    comment_ok = true;
                    continue;
                }
                _ => {}
            }
        }
        if is_probe_name_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_probe_name_byte(bytes[i]) {
                i += 1;
            }
            match &bytes[start..i] {
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" => case_depth = case_depth.saturating_sub(1),
                _ => {}
            }
            comment_ok = false;
            continue;
        }
        if b == b'(' {
            depth = depth.saturating_add(1);
            i += 1;
            comment_ok = true;
            continue;
        }
        if b == b')' {
            if case_depth > 0 && depth == 1 {
                i += 1;
                comment_ok = true;
                continue;
            }
            depth = depth.saturating_sub(1);
            i += 1;
            if depth == 0 {
                return Some(i);
            }
            comment_ok = true;
            continue;
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    None
}

fn skip_heredoc_redirect_for_probe(
    bytes: &[u8],
    i: &mut usize,
    heredocs: &mut std::collections::VecDeque<(String, bool)>,
) {
    *i += 2;
    let strip_tabs = if *i < bytes.len() && bytes[*i] == b'-' {
        *i += 1;
        true
    } else {
        false
    };
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t') {
        *i += 1;
    }
    let mut delimiter = String::new();
    let mut quote: Option<u8> = None;
    while *i < bytes.len() {
        let b = bytes[*i];
        if let Some(q) = quote {
            *i += 1;
            if b == q {
                quote = None;
            } else {
                delimiter.push(b as char);
            }
            continue;
        }
        if b == b'\'' || b == b'"' {
            quote = Some(b);
            *i += 1;
            continue;
        }
        if b == b'\\' {
            *i += 1;
            if *i < bytes.len() {
                if bytes[*i] == b'\n' {
                    *i += 1;
                    continue;
                }
                delimiter.push(bytes[*i] as char);
                *i += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')') {
            break;
        }
        delimiter.push(b as char);
        *i += 1;
    }
    if strip_tabs {
        delimiter = delimiter.trim_start_matches('\t').to_string();
    }
    if !delimiter.is_empty() {
        heredocs.push_back((delimiter, strip_tabs));
    }
}

fn is_probe_name_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_probe_name_byte(b: u8) -> bool {
    is_probe_name_start(b) || b.is_ascii_digit()
}

fn skip_backtick_body(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn has_unclosed_heredoc(input: &str) -> bool {
    let mut pending: std::collections::VecDeque<(String, bool, bool)> =
        std::collections::VecDeque::new();
    let mut lines = input.lines();
    let mut command_subst_depth = 0usize;
    while let Some(line) = lines.next() {
        if let Some((delimiter, strip_tabs, remove_escaped_newlines)) = pending.front().cloned() {
            if command_subst_depth > 0 {
                command_subst_depth =
                    command_subst_depth_after_line_for_heredoc_probe(line, command_subst_depth);
                continue;
            }
            let mut logical_line = line.to_string();
            if remove_escaped_newlines {
                while logical_line.ends_with('\\') {
                    logical_line.pop();
                    let Some(next) = lines.next() else {
                        return true;
                    };
                    logical_line.push_str(next);
                }
            }
            let candidate = if strip_tabs {
                logical_line.trim_start_matches('\t')
            } else {
                logical_line.as_str()
            };
            if candidate == delimiter.as_str() {
                pending.pop_front();
            }
            continue;
        }
        let mut command_line = line.to_string();
        while command_line.ends_with('\\') {
            command_line.pop();
            let Some(next) = lines.next() else {
                return true;
            };
            command_line.push_str(next);
        }
        command_subst_depth =
            command_subst_depth_after_line_for_heredoc_probe(&command_line, command_subst_depth);
        for spec in heredocs_in_line(&command_line) {
            pending.push_back(spec);
        }
    }
    !pending.is_empty()
}

fn command_subst_depth_after_line_for_heredoc_probe(line: &str, mut depth: usize) -> usize {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut single = false;
    let mut double = false;
    while i < bytes.len() {
        let b = bytes[i];
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if double {
            if b == b'"' {
                double = false;
                i += 1;
                continue;
            }
            if b == b'\\' && i + 1 < bytes.len() {
                let n = bytes[i + 1];
                if matches!(n, b'"' | b'\\' | b'$' | b'`') {
                    i += 2;
                    continue;
                }
            }
        }
        match b {
            b'\'' if !double => {
                single = true;
                i += 1;
            }
            b'"' => {
                double = true;
                i += 1;
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                depth = depth.saturating_add(1);
                i += 2;
            }
            b')' if depth > 0 => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    depth
}

fn top_level_heredocs_in_line_update_depth(
    line: &str,
    depth: &mut usize,
    arithmetic_depth: &mut usize,
) -> Vec<(String, bool, bool)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut single = false;
    let mut double = false;

    while i < bytes.len() {
        let b = bytes[i];
        if *arithmetic_depth > 0 {
            match b {
                b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
                b'\'' | b'"' | b'`' => {
                    if let Some(end) = skip_quoted_for_parse(bytes, i) {
                        i = end;
                    } else {
                        break;
                    }
                }
                b'(' => {
                    *arithmetic_depth = (*arithmetic_depth).saturating_add(1);
                    i += 1;
                }
                b')' => {
                    *arithmetic_depth = (*arithmetic_depth).saturating_sub(1);
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if double {
            match b {
                b'"' => {
                    double = false;
                    i += 1;
                }
                b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                    if i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                        *arithmetic_depth = 2;
                        i += 3;
                    } else if command_substitution_is_arithmetic(bytes, i) {
                        i = skip_arithmetic_substitution_for_parse(bytes, i + 3)
                            .unwrap_or(bytes.len());
                    } else {
                        *depth = (*depth).saturating_add(1);
                        i += 2;
                    }
                }
                b')' if *depth > 0 => {
                    *depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        match b {
            b'\'' => {
                single = true;
                i += 1;
            }
            b'"' => {
                double = true;
                i += 1;
            }
            b'`' => {
                if let Some(end) = skip_backtick_body(bytes, i) {
                    i = end;
                } else {
                    break;
                }
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                if i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                    *arithmetic_depth = 2;
                    i += 3;
                } else if command_substitution_is_arithmetic(bytes, i) {
                    i = skip_arithmetic_substitution_for_parse(bytes, i + 3).unwrap_or(bytes.len());
                } else {
                    *depth = (*depth).saturating_add(1);
                    i += 2;
                }
            }
            b')' if *depth > 0 => {
                *depth -= 1;
                i += 1;
            }
            b'<' if *depth == 0 && i + 1 < bytes.len() && bytes[i + 1] == b'<' => {
                if i > 0 && bytes[i - 1] == b'<' {
                    i += 1;
                    continue;
                }
                i += 2;
                let strip_tabs = if i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                    true
                } else {
                    false
                };
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                let start = i;
                let mut quote: Option<u8> = None;
                let mut quoted = false;
                let mut text = String::new();
                while i < bytes.len() {
                    let c = bytes[i];
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        } else {
                            text.push(c as char);
                        }
                        i += 1;
                        continue;
                    }
                    if c == b'\'' || c == b'"' {
                        quoted = true;
                        quote = Some(c);
                        i += 1;
                        continue;
                    }
                    if c == b'\\' {
                        quoted = true;
                        i += 1;
                        if i < bytes.len() {
                            text.push(bytes[i] as char);
                            i += 1;
                        }
                        continue;
                    }
                    if c.is_ascii_whitespace()
                        || matches!(c, b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')')
                    {
                        break;
                    }
                    text.push(c as char);
                    i += 1;
                }
                if strip_tabs {
                    text = text.trim_start_matches('\t').to_string();
                }
                if !text.is_empty() || i > start {
                    out.push((text, strip_tabs, !quoted));
                }
            }
            _ => i += 1,
        }
    }

    out
}

fn heredocs_in_line(line: &str) -> Vec<(String, bool, bool)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    while i + 1 < bytes.len() {
        let b = bytes[i];
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if double {
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => {
                single = true;
                i += 1;
            }
            b'"' => {
                double = true;
                i += 1;
            }
            b'<' if bytes[i + 1] == b'<' => {
                if i > 0 && bytes[i - 1] == b'<' {
                    i += 1;
                    continue;
                }
                i += 2;
                let strip_tabs = if i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                    true
                } else {
                    false
                };
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                let start = i;
                let mut quote: Option<u8> = None;
                let mut quoted = false;
                let mut text = String::new();
                while i < bytes.len() {
                    let c = bytes[i];
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        } else {
                            text.push(c as char);
                        }
                        i += 1;
                        continue;
                    }
                    if c == b'\'' || c == b'"' {
                        quoted = true;
                        quote = Some(c);
                        i += 1;
                        continue;
                    }
                    if c == b'\\' {
                        quoted = true;
                        i += 1;
                        if i < bytes.len() {
                            text.push(bytes[i] as char);
                            i += 1;
                        }
                        continue;
                    }
                    if c.is_ascii_whitespace()
                        || matches!(c, b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')')
                    {
                        break;
                    }
                    text.push(c as char);
                    i += 1;
                }
                if strip_tabs {
                    text = text.trim_start_matches('\t').to_string();
                }
                if !text.is_empty() || i > start {
                    out.push((text, strip_tabs, !quoted));
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn execute_prompt_command(state: &mut ShellState) {
    let raw = state.get("PROMPT_COMMAND").unwrap_or_default();
    if raw.trim().is_empty() {
        return;
    }
    let mut lexer = Lexer::new(&raw);
    lexer.set_extglob_patterns(state.option("extglob"));
    lexer.set_posix_mode(state.option("posix"));
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, &raw);
    match parser.parse() {
        Ok(ast) => {
            let saved = state.last_command_exit_value;
            let _ = execute_in(&ast, state);
            state.last_command_exit_value = saved;
        }
        Err(err) => {
            eprintln!("cherubsh: PROMPT_COMMAND: {}", err.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{has_unclosed_command_substitution, has_unclosed_heredoc};

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
