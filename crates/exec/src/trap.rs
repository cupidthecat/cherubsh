use cherubsh_common::signals::{SignalMaskGuard, TrapAction, TrapKind, NSIG};
use cherubsh_common::Environment;
use cherubsh_lexer::Lexer;
use cherubsh_parser::{Command, CommandData, Parser};

use crate::{ExecContext, ExecMode, Unwind};

const RUNNING_DEBUG_TRAP: i32 = -1;
const RUNNING_ERR_TRAP: i32 = -2;
const RUNNING_RETURN_TRAP: i32 = -3;
const RUNNING_EXIT_TRAP: i32 = -4;

pub(crate) fn run_debug_trap(ctx: &mut ExecContext<'_>) -> Option<i32> {
    if ctx.env.running_trap() == Some(RUNNING_DEBUG_TRAP) {
        return None;
    }
    let body = match ctx.env.trap_action(TrapKind::Debug) {
        Some(TrapAction::Command(body)) => body,
        _ => return None,
    };
    run_trap_body(ctx, RUNNING_DEBUG_TRAP, &body)
}

pub(crate) fn run_err_trap(ctx: &mut ExecContext<'_>) -> Option<i32> {
    if ctx.env.running_trap().is_some() {
        return None;
    }
    let body = match ctx.env.trap_action(TrapKind::Err) {
        Some(TrapAction::Command(body)) => body,
        _ => return None,
    };
    run_trap_body(ctx, RUNNING_ERR_TRAP, &body)
}

pub(crate) fn run_return_trap(ctx: &mut ExecContext<'_>) -> Option<i32> {
    if matches!(
        ctx.env.running_trap(),
        Some(RUNNING_RETURN_TRAP | RUNNING_DEBUG_TRAP)
    ) {
        return None;
    }
    let body = match ctx.env.trap_action(TrapKind::Return) {
        Some(TrapAction::Command(body)) => body,
        _ => return None,
    };
    run_trap_body(ctx, RUNNING_RETURN_TRAP, &body)
}

pub(crate) fn run_exit_trap(ctx: &mut ExecContext<'_>) -> Option<i32> {
    if ctx.env.inherited_exit_trap_suppressed() {
        return None;
    }
    let body = match ctx.env.trap_action(TrapKind::Exit) {
        Some(TrapAction::Command(body)) => body,
        _ => return None,
    };
    ctx.env.trap_clear(TrapKind::Exit);
    let saved_status = ctx.last_status;
    let saved_pending = ctx.pending.take();
    if run_trap_body_with_line_offset(ctx, RUNNING_EXIT_TRAP, &body, false).is_none() {
        ctx.pending = saved_pending;
        return None;
    }
    if let Some(crate::Unwind::Exit(status)) = ctx.pending.take() {
        return Some(status);
    }
    ctx.pending = saved_pending;
    ctx.last_status = saved_status;
    ctx.env.set_last_status(saved_status);
    None
}

pub(crate) fn run_pending_traps(ctx: &mut ExecContext<'_>) {
    if ctx.env.running_trap().is_some() {
        return;
    }
    for sig in 1..NSIG {
        let sig = sig as i32;
        let count = ctx.env.pending_signal_take(sig);
        if count == 0 {
            continue;
        }
        handle_signal(ctx, sig, count);
    }
}

pub(crate) fn run_sigchld_trap_once(ctx: &mut ExecContext<'_>) {
    if ctx.env.running_trap().is_some() {
        return;
    }
    if let Some(TrapAction::Command(body)) = ctx.env.trap_action(TrapKind::Numeric(libc::SIGCHLD)) {
        let _ = run_trap_body(ctx, libc::SIGCHLD, &body);
    }
}

pub(crate) fn run_signal_trap(ctx: &mut ExecContext<'_>, sig: i32) -> Option<i32> {
    if ctx.env.running_trap().is_some() {
        return None;
    }
    match ctx.env.trap_action(TrapKind::Numeric(sig)) {
        Some(TrapAction::Command(body)) => run_trap_body(ctx, sig, &body),
        Some(TrapAction::Ignore) => Some(0),
        _ => None,
    }
}

fn handle_signal(ctx: &mut ExecContext<'_>, sig: i32, count: u32) {
    if sig == libc::SIGCHLD {
        let reaped = {
            let _guard = SignalMaskGuard::block_sigchld();
            match ctx.env.jobs_table_mut() {
                Some(table) => table.reap_all(),
                None => Vec::new(),
            }
        };
        if !reaped.is_empty() {
            if let Some(TrapAction::Command(body)) =
                ctx.env.trap_action(TrapKind::Numeric(libc::SIGCHLD))
            {
                for _ in 0..reaped.len() {
                    let _ = run_trap_body(ctx, sig, &body);
                }
            }
        }
        let _ = count;
        return;
    }

    if let Some(TrapAction::Command(body)) = ctx.env.trap_action(TrapKind::Numeric(sig)) {
        for _ in 0..count {
            let _ = run_trap_body(ctx, sig, &body);
        }
    }
}

fn run_trap_body(ctx: &mut ExecContext<'_>, sig: i32, body: &str) -> Option<i32> {
    run_trap_body_with_line_offset(ctx, sig, body, true)
}

fn run_trap_body_with_line_offset(
    ctx: &mut ExecContext<'_>,
    sig: i32,
    body: &str,
    offset_lines: bool,
) -> Option<i32> {
    if body.is_empty() {
        return Some(0);
    }
    let saved_trap = ctx.env.running_trap();
    ctx.env.set_running_trap(Some(sig));
    let saved_trap_base = ctx.trap_base_function_depth;
    ctx.trap_base_function_depth = Some(ctx.function_depth);
    let saved_status = ctx.last_status;
    let ast = parse_trap_body(ctx.env, body, offset_lines);
    let mut status = match ast {
        Some(ast) => ctx.execute_command(&ast.root, ExecMode::Parent),
        None => 2,
    };
    let mut restore_status = true;
    match ctx.pending.take() {
        Some(Unwind::Return { status: -1, .. }) => {
            status = saved_status;
        }
        Some(Unwind::Return { status: n, .. })
            if ctx.trap_base_function_depth == Some(0) && ctx.function_depth == 0 =>
        {
            status = n;
            restore_status = false;
        }
        other => {
            ctx.pending = other;
        }
    }
    if restore_status {
        ctx.last_status = saved_status;
        ctx.env.set_last_status(saved_status);
    } else {
        ctx.last_status = status;
        ctx.env.set_last_status(status);
    }
    ctx.trap_base_function_depth = saved_trap_base;
    ctx.env.set_running_trap(saved_trap);
    Some(status)
}

fn parse_trap_body(
    env: &dyn Environment,
    source: &str,
    offset_lines: bool,
) -> Option<cherubsh_parser::Ast> {
    let mut lexer = Lexer::new(source);
    lexer.set_extglob_patterns(env.option("extglob"));
    lexer.set_posix_mode(env.option("posix"));
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, source);
    match parser.parse() {
        Ok(mut ast) => {
            if offset_lines {
                if let Some(line) = env.diagnostic_line() {
                    offset_command_lines(&mut ast.root, line.saturating_sub(1));
                }
            }
            Some(ast)
        }
        Err(err) => {
            eprintln!("cherubsh: trap: {}", err.message);
            None
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
