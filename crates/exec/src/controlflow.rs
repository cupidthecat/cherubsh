use cherubsh_builtins::common::{array_reference, is_valid_name};
use cherubsh_common::{AssignError, VarAttrs, CASEPAT_FALLTHROUGH, CASEPAT_TESTNEXT, W_ASSIGNMENT};
use cherubsh_expander::pattern::{fnmatch, GlobOpts};
use cherubsh_expander::{expand_case_pattern_bytes, quote::shell_string_to_bytes, ExpandError};
use cherubsh_parser::{
    ArithForCommand, CaseCommand, Command, CommandData, Connection, ForCommand, IfCommand,
    PatternList, SelectCommand, CONN_AMP, CONN_AND_AND, CONN_BAR_AND, CONN_NEWLINE, CONN_OR_OR,
    CONN_PIPE, CONN_SEMI,
};
use std::borrow::Cow;

use crate::pipeline;
use crate::util::{expand_one, expand_words};
use crate::{ExecContext, ExecMode, Unwind};

pub(crate) fn execute_connection<'a>(
    ctx: &mut ExecContext<'a>,
    conn: &Connection,
    mode: ExecMode,
) -> i32 {
    match conn.connector {
        CONN_AND_AND => {
            ctx.errexit_suppressed += 1;
            let status = ctx.execute_command(&conn.first, mode);
            ctx.errexit_suppressed -= 1;
            if posix_special_redirection_failure(ctx, &conn.first, status) {
                ctx.pending = Some(Unwind::Exit(status));
                return status;
            }
            if ctx.pending.is_some() {
                return status;
            }
            if status == 0 {
                ctx.execute_command(&conn.second, mode)
            } else {
                status
            }
        }
        CONN_OR_OR => {
            ctx.errexit_suppressed += 1;
            let status = ctx.execute_command(&conn.first, mode);
            ctx.errexit_suppressed -= 1;
            if posix_special_redirection_failure(ctx, &conn.first, status) {
                ctx.pending = Some(Unwind::Exit(status));
                return status;
            }
            if ctx.pending.is_some() {
                return status;
            }
            if status != 0 {
                ctx.execute_command(&conn.second, mode)
            } else {
                status
            }
        }
        CONN_SEMI => {
            let _ = ctx.execute_command(&conn.first, mode);
            if ctx.pending.is_some() {
                return ctx.last_status;
            }
            ctx.execute_command(&conn.second, mode)
        }
        CONN_NEWLINE => {
            let _ = ctx.execute_command(&conn.first, mode);
            if matches!(ctx.pending, Some(Unwind::AbortLine(_))) && ctx.abort_line_depth == 0 {
                if let Some(Unwind::AbortLine(status)) = ctx.pending.take() {
                    ctx.last_status = status;
                    ctx.env.set_last_status(status);
                }
            } else if ctx.pending.is_some() {
                return ctx.last_status;
            }
            ctx.execute_command(&conn.second, mode)
        }
        CONN_AMP => {
            if let CommandData::Connection(left) = &conn.first.data {
                if left.connector == CONN_AMP {
                    let mut async_commands = Vec::new();
                    collect_async_chain(&conn.first, &mut async_commands);
                    for command in async_commands {
                        let _ = pipeline::spawn_background(ctx, &command);
                        if ctx.pending.is_some() {
                            return ctx.last_status;
                        }
                    }
                    return ctx.execute_command(&conn.second, mode);
                }
                if left.connector == CONN_SEMI || left.connector == CONN_NEWLINE {
                    let _ = ctx.execute_command(&left.first, mode);
                    if ctx.pending.is_some() {
                        return ctx.last_status;
                    }
                    let reassociated = Connection {
                        first: left.second.clone(),
                        second: conn.second.clone(),
                        connector: CONN_AMP,
                    };
                    return execute_connection(ctx, &reassociated, mode);
                }
            }
            let _ = pipeline::spawn_background(ctx, &conn.first);
            ctx.execute_command(&conn.second, mode)
        }
        CONN_PIPE | CONN_BAR_AND => {
            let mut commands = Vec::new();
            collect_pipeline(conn, &mut commands);
            pipeline::execute(ctx, commands, conn.connector == CONN_BAR_AND)
        }
        _ => ctx.unsupported("connection"),
    }
}

fn posix_special_redirection_failure(
    ctx: &ExecContext<'_>,
    command: &Command,
    status: i32,
) -> bool {
    status != 0 && ctx.env.option("posix") && simple_special_has_redirects(command)
}

fn simple_special_has_redirects(command: &Command) -> bool {
    let CommandData::Simple(simple) = &command.data else {
        return false;
    };
    if command.redirects.is_empty() && simple.redirects.is_empty() {
        return false;
    }
    let Some(word) = simple
        .words
        .iter()
        .find(|word| word.flags & W_ASSIGNMENT == 0)
    else {
        return false;
    };
    cherubsh_builtins::is_special(&word.text)
}

fn collect_async_chain(command: &Command, out: &mut Vec<Command>) {
    if let CommandData::Connection(conn) = &command.data {
        if conn.connector == CONN_AMP {
            collect_async_chain(&conn.first, out);
            collect_async_chain(&conn.second, out);
            return;
        }
    }
    out.push(command.clone());
}

fn collect_pipeline<'a>(conn: &'a Connection, out: &mut Vec<&'a Command>) {
    if let CommandData::Connection(inner) = &conn.first.data {
        if inner.connector == CONN_PIPE || inner.connector == CONN_BAR_AND {
            collect_pipeline(inner, out);
        } else {
            out.push(&conn.first);
        }
    } else {
        out.push(&conn.first);
    }
    out.push(&conn.second);
}

pub(crate) fn execute_if<'a>(ctx: &mut ExecContext<'a>, if_cmd: &IfCommand, mode: ExecMode) -> i32 {
    ctx.errexit_suppressed += 1;
    let test_status = ctx.execute_command(&if_cmd.test, mode);
    ctx.errexit_suppressed -= 1;
    if ctx.pending.is_some() {
        return test_status;
    }
    if test_status == 0 {
        ctx.execute_command(&if_cmd.true_case, mode)
    } else if let Some(false_case) = &if_cmd.false_case {
        ctx.execute_command(false_case, mode)
    } else {
        0
    }
}

pub(crate) fn execute_while_or_until<'a>(
    ctx: &mut ExecContext<'a>,
    test: &Command,
    action: &Command,
    mode: ExecMode,
    invert: bool,
) -> i32 {
    ctx.loop_depth += 1;
    let mut status = 0;
    loop {
        ctx.errexit_suppressed += 1;
        let test_status = ctx.execute_command(test, mode);
        ctx.errexit_suppressed -= 1;
        if ctx.pending.is_some() {
            break;
        }
        let ok = if invert {
            test_status != 0
        } else {
            test_status == 0
        };
        if !ok {
            break;
        }
        status = ctx.execute_command(action, mode);
        if handle_loop_unwind(ctx) {
            break;
        }
    }
    ctx.loop_depth -= 1;
    status
}

pub(crate) fn execute_for<'a>(
    ctx: &mut ExecContext<'a>,
    for_cmd: &ForCommand,
    mode: ExecMode,
) -> i32 {
    ctx.loop_depth += 1;
    let items: Vec<String> = if let Some(map_list) = for_cmd.map_list.as_ref() {
        expand_words(map_list, ctx)
    } else {
        // iterate over positionals $1..$N
        let mut out = Vec::new();
        let mut i = 1;
        while let Some(p) = ctx.env.positional(i) {
            out.push(p);
            i += 1;
        }
        out
    };
    let mut status = 0;
    for item in items {
        ctx.run_debug_trap_for_command(mode);
        trace_for(ctx, for_cmd);
        if let Err(err) = assign_for_value(ctx, &for_cmd.name.text, item) {
            cherubsh_builtins::common::report_assign_error(ctx.env, &err);
            if ctx.env.option("posix") && matches!(err, AssignError::ReadOnly(_)) {
                ctx.pending = Some(Unwind::Exit(1));
            }
            break;
        }
        status = ctx.execute_command(&for_cmd.action, mode);
        if handle_loop_unwind(ctx) {
            break;
        }
    }
    ctx.loop_depth -= 1;
    status
}

fn assign_for_value(
    ctx: &mut ExecContext<'_>,
    name: &str,
    value: String,
) -> Result<(), AssignError> {
    if !ctx.env.attrs(name).contains(VarAttrs::NAMEREF) {
        return ctx.env.assign(name, value);
    }
    if ctx.env.is_readonly(name) {
        return Err(AssignError::ReadOnly(name.to_string()));
    }
    if !nameref_loop_target_is_valid(&value) {
        return Err(AssignError::InvalidName(value));
    }
    ctx.env.set_attr(name, VarAttrs::NAMEREF, false);
    let result = ctx.env.assign(name, value);
    if result.is_ok() {
        ctx.env.set_attr(name, VarAttrs::NAMEREF, true);
    }
    result
}

fn nameref_loop_target_is_valid(value: &str) -> bool {
    is_valid_name(value) || array_reference(value).is_some()
}

pub(crate) fn execute_select<'a>(
    ctx: &mut ExecContext<'a>,
    select_cmd: &SelectCommand,
    mode: ExecMode,
) -> i32 {
    let items: Vec<String> = if let Some(map_list) = select_cmd.map_list.as_ref() {
        expand_words(map_list, ctx)
    } else {
        let mut out = Vec::new();
        let mut i = 1;
        while let Some(p) = ctx.env.positional(i) {
            out.push(p);
            i += 1;
        }
        out
    };

    ctx.loop_depth += 1;
    let mut status = 0;
    loop {
        if items.is_empty() {
            break;
        }
        print_select_menu(&items);
        print_select_prompt(ctx);

        let Some(line) = read_select_line() else {
            break;
        };
        let reply = line.trim_end_matches(['\n', '\r']).to_string();
        ctx.env.set("REPLY", reply.clone());

        let selected = reply
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|idx| items.get(idx))
            .cloned()
            .unwrap_or_default();
        if let Err(err) = assign_for_value(ctx, &select_cmd.name.text, selected) {
            cherubsh_builtins::common::report_assign_error(ctx.env, &err);
            break;
        }

        status = ctx.execute_command(&select_cmd.action, mode);
        if handle_loop_unwind(ctx) {
            break;
        }
    }
    ctx.loop_depth -= 1;
    status
}

fn print_select_menu(items: &[String]) {
    for (index, item) in items.iter().enumerate() {
        eprintln!("{}) {}", index + 1, item);
    }
}

fn print_select_prompt(ctx: &mut ExecContext<'_>) {
    let prompt = ctx.env.get("PS3").unwrap_or_else(|| "#? ".to_string());
    use std::io::Write;
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(prompt.as_bytes());
    let _ = stderr.flush();
}

fn read_select_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

pub(crate) fn execute_arith_for<'a>(
    ctx: &mut ExecContext<'a>,
    for_cmd: &ArithForCommand,
    mode: ExecMode,
) -> i32 {
    if let Some(init) = &for_cmd.init {
        trace_arith_for_word(ctx, init, false);
        if eval_arith_word(ctx, init).is_none() {
            return 1;
        }
    }

    ctx.loop_depth += 1;
    let mut status = 0;
    loop {
        if let Some(test) = &for_cmd.test {
            ctx.run_debug_trap_for_command(mode);
            trace_arith_for_word(ctx, test, false);
            ctx.errexit_suppressed += 1;
            let test_value = eval_arith_word(ctx, test).unwrap_or(0);
            ctx.errexit_suppressed -= 1;
            if test_value == 0 || ctx.pending.is_some() {
                break;
            }
        }

        status = ctx.execute_command(&for_cmd.action, mode);
        if handle_loop_unwind(ctx) {
            break;
        }

        if let Some(step) = &for_cmd.step {
            ctx.run_debug_trap_for_command(mode);
            trace_arith_for_word(ctx, step, true);
            let _ = eval_arith_word(ctx, step);
        }
    }
    ctx.loop_depth -= 1;
    status
}

fn trace_arith_for_word(ctx: &mut ExecContext<'_>, word: &cherubsh_parser::WordDesc, step: bool) {
    if !ctx.env.option("xtrace") {
        return;
    }
    let expr = sanitize_arith_for_expr(&word.text);
    if step {
        crate::xtrace::trace(ctx, &format!("(( {expr}  ))"));
    } else {
        crate::xtrace::trace(ctx, &format!("(( {expr} ))"));
    }
}

fn eval_arith_word(ctx: &mut ExecContext<'_>, word: &cherubsh_parser::WordDesc) -> Option<i64> {
    use cherubsh_expander::expand_for_arith;
    let mut runner = crate::runner::ExecRunner::with_functions(&ctx.functions);
    let expr = sanitize_arith_for_expr(&word.text);
    match expand_for_arith(expr.as_ref(), ctx.env, &mut runner) {
        Ok(v) => Some(v),
        Err(err) => {
            crate::report_arith_command_error(ctx.env, err, expr.as_ref());
            None
        }
    }
}

fn sanitize_arith_for_expr(text: &str) -> Cow<'_, str> {
    let trimmed = text.trim();
    if !trimmed.ends_with(')') {
        return Cow::Borrowed(trimmed);
    }

    let (mut opens, mut closes) = (0usize, 0usize);
    for b in trimmed.bytes() {
        match b {
            b'(' => opens += 1,
            b')' => closes += 1,
            _ => {}
        }
    }
    if closes <= opens {
        return Cow::Borrowed(trimmed);
    }

    let mut out = trimmed.to_string();
    loop {
        let (mut opens, mut closes) = (0usize, 0usize);
        for b in out.bytes() {
            match b {
                b'(' => opens += 1,
                b')' => closes += 1,
                _ => {}
            }
        }
        if closes <= opens || !out.ends_with(')') {
            break;
        }
        out.pop();
        out = out.trim_end().to_string();
    }
    Cow::Owned(out)
}

pub(crate) fn execute_case<'a>(
    ctx: &mut ExecContext<'a>,
    case_cmd: &CaseCommand,
    mode: ExecMode,
) -> i32 {
    let word_value = expand_one(&case_cmd.word, ctx);
    crate::xtrace::trace(ctx, &format!("case {word_value} in"));
    let mut matched = false;
    let mut status = 0;
    for clause in &case_cmd.clauses {
        if matched {
            // arrived here via ;& fall-through - run unconditionally
            if let Some(action) = &clause.action {
                status = ctx.execute_command(action, mode);
            }
            if clause.flags & CASEPAT_FALLTHROUGH != 0 {
                continue;
            }
            return status;
        }
        let matched_clause = match pattern_list_matches(clause, &word_value, ctx) {
            Ok(value) => value,
            Err(status) => return status,
        };
        if matched_clause {
            matched = true;
            if let Some(action) = &clause.action {
                status = ctx.execute_command(action, mode);
            }
            if clause.flags & CASEPAT_FALLTHROUGH != 0 {
                continue;
            }
            if clause.flags & CASEPAT_TESTNEXT != 0 {
                matched = false;
                continue;
            }
            return status;
        }
    }
    status
}

fn trace_for(ctx: &mut ExecContext<'_>, for_cmd: &ForCommand) {
    let words = for_cmd
        .map_list
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "\"$@\"".to_string());
    crate::xtrace::trace(ctx, &format!("for {} in {words}", for_cmd.name.text));
}

fn pattern_list_matches(
    clause: &PatternList,
    value: &str,
    ctx: &mut ExecContext,
) -> Result<bool, i32> {
    let opts = GlobOpts {
        nocaseglob: ctx.env.option("nocasematch"),
        extglob: ctx.env.option("extglob"),
        globasciiranges: ctx.env.option("globasciiranges"),
    };
    for pat in &clause.patterns {
        let mut runner = crate::runner::ExecRunner::with_functions(&ctx.functions);
        let expanded = match expand_case_pattern_bytes(pat, ctx.env, &mut runner) {
            Ok(value) => value,
            Err(err) => {
                report_case_pattern_error(ctx, err);
                return Err(1);
            }
        };
        if fnmatch(&expanded, &shell_string_to_bytes(value), opts) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn report_case_pattern_error(ctx: &ExecContext<'_>, err: ExpandError) {
    if let ExpandError::AssignToReadonly(name) = &err {
        if let (Some(source), Some(line)) =
            (ctx.env.diagnostic_source_name(), ctx.env.diagnostic_line())
        {
            eprintln!("{source}: line {line}: {name}: readonly variable");
            return;
        }
    }
    err.into_shell_error(None).report();
}

fn handle_loop_unwind(ctx: &mut ExecContext) -> bool {
    match ctx.pending.clone() {
        Some(Unwind::Break(1)) => {
            ctx.pending = None;
            true
        }
        Some(Unwind::Break(n)) => {
            ctx.pending = Some(Unwind::Break(n - 1));
            true
        }
        Some(Unwind::Continue(1)) => {
            ctx.pending = None;
            false
        }
        Some(Unwind::Continue(n)) => {
            ctx.pending = Some(Unwind::Continue(n - 1));
            true
        }
        Some(Unwind::AbortLine(_)) => true,
        Some(Unwind::Return(_)) | Some(Unwind::Exit(_)) => true,
        None => false,
    }
}
