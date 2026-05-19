//! Trap dispatch.
//!
//! Mirrors bash trap.c `run_pending_traps`: invoked at every safe point
//! (between commands, after a builtin, in the `wait` loop) to drain the
//! atomic pending counters set by `signals::generic_counter_handler` and
//! execute the user-registered trap bodies.
//!
//! Re-entry guard: `state.running_trap_sig` is set while a trap body runs;
//! recursive `run_pending_traps` calls bail out (parity with trap.c:317).

use cherubsh_common::jobs::JobState;
use cherubsh_common::signals::{SignalMaskGuard, TrapAction, TrapKind, NSIG};
use cherubsh_common::Environment;
use cherubsh_exec::{execute_in, ExecResult};
use cherubsh_lexer::Lexer;
use cherubsh_parser::{Ast, Command, CommandData, Parser};

use crate::signals::{
    catch_flag_clear, catch_flag_set, pending_signal_take, pending_signals_snapshot,
};
use crate::state::ShellState;

/// Drain pending signal counters and dispatch traps. Safe to call from any
/// "checkpoint": top of reader loop, before each command, after each
/// command, inside `wait`. No-op when no signals are pending.
pub fn run_pending_traps(state: &mut ShellState) {
    if state.running_trap_sig.is_some() {
        // Mirrors bash trap.c:317 - prevent nested trap dispatch.
        return;
    }
    if !catch_flag_set() {
        return;
    }
    catch_flag_clear();

    // Loop in case new signals arrive during a trap body.
    for _round in 0..4 {
        let pending = pending_signals_snapshot();
        if pending.is_empty() {
            break;
        }
        for sig in pending {
            let count = pending_signal_take(sig);
            if count == 0 {
                continue;
            }
            handle_signal(state, sig, count);
        }
    }
}

fn handle_signal(state: &mut ShellState, sig: i32, count: u32) {
    // SIGCHLD: reap children under the SIGCHLD mask, then run optional trap.
    if sig == libc::SIGCHLD {
        let reaped = {
            let _guard = SignalMaskGuard::block_sigchld();
            match state.jobs_table_mut() {
                Some(table) => table.reap_all(),
                None => Vec::new(),
            }
        };
        if !reaped.is_empty() {
            if let Some(TrapAction::Command(body)) =
                state.trap_action(TrapKind::Numeric(libc::SIGCHLD))
            {
                for _ in 0..reaped.len() {
                    run_trap_body(state, sig, &body);
                }
            }
        }
        let _ = count;
        return;
    }

    match state
        .trap_action(TrapKind::Numeric(sig))
        .unwrap_or(TrapAction::Default)
    {
        TrapAction::Default => {
            // Default disposition already handled by the kernel handler
            // re-raising. For SIGINT in interactive shells, the reader
            // loop converts the pending state via `check_signals()`.
        }
        TrapAction::Ignore => {
            // No-op.
        }
        TrapAction::Command(body) => {
            run_trap_body(state, sig, &body);
        }
    }
}

fn run_trap_body(state: &mut ShellState, sig: i32, body: &str) {
    state.running_trap_sig = Some(sig);
    let saved_status = state.last_command_exit_value;
    let _ = parse_and_run(state, body);
    // bash restores $? after the trap unless it ran a command that updated
    // $? non-trivially. We follow the simpler semantics: restore unless the
    // trap exit was non-zero, mirroring bash 5.2's "trap returns 0" path.
    state.last_command_exit_value = saved_status;
    state.running_trap_sig = None;
}

fn parse_and_run(state: &mut ShellState, source: &str) -> ExecResult {
    if source.is_empty() {
        return ExecResult {
            status: 0,
            exit_shell: false,
        };
    }
    let mut lexer = Lexer::new(source);
    lexer.set_extglob_patterns(state.option("extglob"));
    lexer.set_posix_mode(state.option("posix"));
    let mut tokens = Vec::new();
    while let Some(t) = lexer.next_token() {
        tokens.push(t);
    }
    let mut parser = Parser::new(tokens, source);
    match parser.parse() {
        Ok(mut ast) => {
            if let Some(line) = state.diagnostic_line() {
                offset_command_lines(&mut ast.root, line.saturating_sub(1));
            }
            execute_in(&Ast { root: ast.root }, state)
        }
        Err(err) => {
            eprintln!("cherubsh: trap: {}", err.message);
            ExecResult {
                status: 2,
                exit_shell: false,
            }
        }
    }
}

fn offset_command_lines(command: &mut Command, delta: u32) {
    if command.line > 0 {
        command.line = command.line.saturating_add(delta);
    }
    match &mut command.data {
        CommandData::For(c) => {
            offset_line(&mut c.line, delta);
            offset_command_lines(&mut c.action, delta);
        }
        CommandData::Case(c) => {
            offset_line(&mut c.line, delta);
            for clause in &mut c.clauses {
                if let Some(action) = &mut clause.action {
                    offset_command_lines(action, delta);
                }
            }
        }
        CommandData::While(c) => {
            offset_command_lines(&mut c.test, delta);
            offset_command_lines(&mut c.action, delta);
        }
        CommandData::Until(c) => {
            offset_command_lines(&mut c.test, delta);
            offset_command_lines(&mut c.action, delta);
        }
        CommandData::If(c) => {
            offset_command_lines(&mut c.test, delta);
            offset_command_lines(&mut c.true_case, delta);
            if let Some(false_case) = &mut c.false_case {
                offset_command_lines(false_case, delta);
            }
        }
        CommandData::Connection(c) => {
            offset_command_lines(&mut c.first, delta);
            offset_command_lines(&mut c.second, delta);
        }
        CommandData::FunctionDef(c) => {
            offset_command_lines(std::sync::Arc::make_mut(&mut c.command), delta);
        }
        CommandData::Group(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        CommandData::Select(c) => {
            offset_line(&mut c.line, delta);
            offset_command_lines(&mut c.action, delta);
        }
        CommandData::ArithFor(c) => {
            offset_command_lines(&mut c.action, delta);
        }
        CommandData::Subshell(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        CommandData::Coproc(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        CommandData::Simple(_) | CommandData::Arith(_) | CommandData::Cond(_) => {}
    }
}

fn offset_line(line: &mut u32, delta: u32) {
    if *line > 0 {
        *line = (*line).saturating_add(delta);
    }
}

/// Fire the EXIT trap, if any. Called exactly once from `exit_shell`.
/// Bash unsets the EXIT trap inside the body so `exit` from within doesn't
/// re-enter; we do the same.
pub fn run_exit_trap(state: &mut ShellState) -> Option<i32> {
    if state.inherited_exit_trap_suppressed() {
        return None;
    }
    if let Some(TrapAction::Command(body)) = state.trap_action(TrapKind::Exit) {
        state.trap_clear(TrapKind::Exit);
        let saved = state.last_command_exit_value;
        let result = parse_and_run(state, &body);
        if result.exit_shell {
            return Some(result.status);
        }
        state.last_command_exit_value = saved;
    }
    None
}

/// Fire the ERR trap when a simple command exits non-zero outside a
/// conditional context. Caller is responsible for the "outside conditional"
/// check; this entry point just executes the body.
pub fn run_err_trap(state: &mut ShellState) {
    if state.running_trap_sig.is_some() {
        return;
    }
    if let Some(TrapAction::Command(body)) = state.trap_action(TrapKind::Err) {
        let saved = state.last_command_exit_value;
        state.running_trap_sig = Some(0);
        let _ = parse_and_run(state, &body);
        state.last_command_exit_value = saved;
        state.running_trap_sig = None;
    }
}

/// Fire DEBUG trap before each simple command (interactive / extdebug).
pub fn run_debug_trap(state: &mut ShellState) {
    if state.running_trap_sig.is_some() {
        return;
    }
    if let Some(TrapAction::Command(body)) = state.trap_action(TrapKind::Debug) {
        let saved = state.last_command_exit_value;
        state.running_trap_sig = Some(0);
        let _ = parse_and_run(state, &body);
        state.last_command_exit_value = saved;
        state.running_trap_sig = None;
    }
}

/// Fire RETURN trap on function return / source completion.
pub fn run_return_trap(state: &mut ShellState) {
    if state.running_trap_sig.is_some() {
        return;
    }
    if let Some(TrapAction::Command(body)) = state.trap_action(TrapKind::Return) {
        let saved = state.last_command_exit_value;
        state.running_trap_sig = Some(0);
        let _ = parse_and_run(state, &body);
        state.last_command_exit_value = saved;
        state.running_trap_sig = None;
    }
}

/// Drain finished jobs from the table and emit "[N]+ Done command" notices
/// to stderr. Called before the prompt in interactive shells.
pub fn notify_completed_jobs(state: &mut ShellState) {
    let _guard = SignalMaskGuard::block_sigchld();
    let Some(table) = state.jobs_table_mut() else {
        return;
    };
    let pending = table.pending_notifications();
    for job in pending {
        let label = match job.state {
            JobState::Done => "Done",
            JobState::Stopped => "Stopped",
            JobState::Running => continue,
        };
        eprintln!(
            "[{}]+  {}                {}",
            job.id.raw(),
            label,
            job.command_line
        );
    }
    table.purge_done();
    let _ = NSIG;
}
