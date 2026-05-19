use cherubsh_common::signals::{TrapAction, TrapKind};
use cherubsh_common::Environment;
use cherubsh_lexer::Lexer;
use cherubsh_parser::{Command, CommandData, Parser};

use crate::{ExecContext, ExecMode};

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
    if run_trap_body(ctx, RUNNING_EXIT_TRAP, &body).is_none() {
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

fn run_trap_body(ctx: &mut ExecContext<'_>, sig: i32, body: &str) -> Option<i32> {
    if body.is_empty() {
        return Some(0);
    }
    let saved_trap = ctx.env.running_trap();
    ctx.env.set_running_trap(Some(sig));
    let saved_status = ctx.last_status;
    let ast = parse_trap_body(ctx.env, body);
    let status = match ast {
        Some(ast) => ctx.execute_command(&ast.root, ExecMode::Parent),
        None => 2,
    };
    ctx.last_status = saved_status;
    ctx.env.set_last_status(saved_status);
    ctx.env.set_running_trap(saved_trap);
    Some(status)
}

fn parse_trap_body(env: &dyn Environment, source: &str) -> Option<cherubsh_parser::Ast> {
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
            if let Some(line) = env.diagnostic_line() {
                offset_command_lines(&mut ast.root, line.saturating_sub(1));
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
