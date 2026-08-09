use cherubsh_common::{expand_aliases_for_parse, histexpand, Environment, ShellJump, ShellResult};
use cherubsh_exec::{execute_with_state, ExecState};
use cherubsh_lexer::{Lexer, TokenKind};
use cherubsh_lineedit::{Completion, CompletionProvider, EditError, HistoryProvider, LineEditor};
use cherubsh_parser::{Ast, Command, CommandData, ParseError, Parser};

use crate::completion::{self, CompRequest, CompletionQuote};
use crate::prompt::{expand_prompt_string, prompt_again};
use crate::signals::{arm_alarm, check_signals, disarm_alarm, winch_taken};
use crate::state::ShellState;
use crate::traps::{notify_completed_jobs, run_pending_traps};

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

        let parsed = match read_command(state, exec_state) {
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
                    if !state.interactive {
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
                    let decoded = expand_prompt_string(state, exec_state, &ps0);
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
        state.update_window_size();
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
pub fn run_until_eof_with_exec_state(state: &mut ShellState, exec_state: &mut ExecState) {
    let saved_eof = state.eof_reached;
    let saved_just_one = state.just_one_command;
    state.eof_reached = false;
    state.just_one_command = false;
    let _ = reader_loop_with_exec_state(state, exec_state);
    state.just_one_command = saved_just_one;
    state.eof_reached = saved_eof;
}

/// read_command: port of eval.c:360-401.
pub fn read_command(
    state: &mut ShellState,
    exec_state: &mut ExecState,
) -> ShellResult<Option<Command>> {
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
    let result = parse_command(state, exec_state);
    if tmout_armed {
        disarm_alarm();
    }
    result
}

/// parse_command: port of eval.c:323-354.
pub fn parse_command(
    state: &mut ShellState,
    exec_state: &mut ExecState,
) -> ShellResult<Option<Command>> {
    state.need_here_doc = false;
    refresh_window_after_signal(state);
    // Dispatch any queued traps + reap completed jobs before prompting.
    run_pending_traps(state);
    if state.interactive && !state.input.is_stream() {
        notify_completed_jobs(state);
    }
    if state.interactive && !state.input.is_string() && !state.input.is_stream() {
        state.check_mail();
        execute_prompt_command(state, exec_state);
    }
    let input_text = read_logical_command(state, exec_state)?;
    refresh_window_after_signal(state);
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
    if (state.interactive && !state.input.is_string() && !state.input.is_stream())
        || (state.option("history") && !(state.interactive && state.input.is_string()))
    {
        let record_line = history_record_line(state, &input_text);
        if should_record_history(state, &record_line) {
            let control = state.histcontrol_flags;
            state.history_last_line_added = state.history_table.add(&record_line, control);
        }
    }
    let parse_input = expand_aliases_for_parse(&input_text, state);
    let comments_enabled = !state.interactive || state.option("interactive_comments");
    match parse_text(
        &parse_input,
        state.option("extglob"),
        state.option("posix"),
        comments_enabled,
    ) {
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

fn refresh_window_after_signal(state: &mut ShellState) {
    if state.interactive && winch_taken() {
        state.update_window_size();
    }
}

include!("history.rs");
include!("diagnostics.rs");
include!("parsing.rs");
include!("input.rs");
include!("history_provider.rs");
include!("completion.rs");
include!("continuation_probes.rs");
include!("substitution_probes.rs");
include!("heredoc_probes.rs");
include!("prompt.rs");
include!("tests.rs");
