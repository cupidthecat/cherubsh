use crate::common::{array_expand_once_subscript_message, report_bare_diagnostic};
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::Environment;

pub struct Let;
pub static LET: Let = Let;

impl Builtin for Let {
    fn name(&self) -> &'static str {
        "let"
    }
    fn synopsis(&self) -> &'static str {
        "let arg [arg ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let args = if ctx.args.first().is_some_and(|arg| arg == "--") {
            let mut start = 1;
            if ctx.env_ref().command_substitution_depth() > 0 {
                while ctx.args.get(start).is_some_and(|arg| arg == "--") {
                    start += 1;
                }
            }
            &ctx.args[start..]
        } else {
            ctx.args
        };
        if args.is_empty() {
            report_let_error(ctx.env_ref(), "", "expression expected");
            return 2;
        }
        let mut last_value: i64 = 0;
        for expr in args.iter() {
            if let Some((word, token)) = let_word_split_array_subscript_error(expr) {
                report_let_error(
                    ctx.env_ref(),
                    word,
                    &format!("bad array subscript (error token is \"{token}\")"),
                );
                return 1;
            }
            if let Some(token) = let_literal_dollar_operand(expr) {
                report_let_error(
                    ctx.env_ref(),
                    expr,
                    &format!(
                        "arithmetic syntax error: operand expected (error token is \"{}\")",
                        token.replace('\\', "\\\\").replace('"', "\\\"")
                    ),
                );
                return 1;
            }
            if let Some(subscript) = first_indexed_subscript(expr) {
                if let Some(message) = array_expand_once_subscript_message(ctx.env_ref(), subscript)
                {
                    report_bare_diagnostic(ctx.env_ref(), &message);
                    return 1;
                }
            }
            let rewritten_expr;
            let eval_expr = if !ctx.env_ref().option("assoc_expand_once") && expr.contains("[\"\"]")
            {
                rewritten_expr = expr.replace("[\"\"]", "[0]");
                rewritten_expr.as_str()
            } else {
                expr.as_str()
            };
            match ctx.shell.evaluate_arith(eval_expr) {
                Ok(v) => last_value = v,
                Err(msg) => {
                    if ctx.env_ref().option("assoc_expand_once")
                        && expr.contains("[\"")
                        && msg.contains("arithmetic syntax error: operand expected")
                    {
                        report_bare_arith_error(ctx.env_ref(), expr, &msg);
                        if ctx.env_ref().subshell_level() > 0 {
                            ctx.shell.request_exit(1);
                        }
                    } else {
                        report_let_error(ctx.env_ref(), expr, &msg);
                    }
                    return 1;
                }
            }
        }
        if last_value == 0 {
            1
        } else {
            0
        }
    }
}

fn let_literal_dollar_operand(expr: &str) -> Option<&str> {
    let bytes = expr.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'$' if depth == 0 => return Some(expr[i..].trim_start()),
            _ => i += 1,
        }
    }
    None
}

fn skip_quoted(bytes: &[u8], mut i: usize) -> usize {
    let quote = bytes[i];
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = (i + 2).min(bytes.len());
        } else if bytes[i] == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    i
}

fn let_word_split_array_subscript_error(expr: &str) -> Option<(&str, &str)> {
    let word = expr.split_whitespace().next()?;
    if word.len() == expr.len() || !word.contains("$(") {
        return None;
    }
    let token = word
        .rsplit_once(',')
        .map(|(_, token)| token)
        .unwrap_or(word);
    if token.contains('[') && (word.contains("],") || word.contains("\\],")) {
        Some((word, token))
    } else {
        None
    }
}

fn first_indexed_subscript(expr: &str) -> Option<&str> {
    let open = expr.find('[')?;
    let close = expr[open + 1..].find(']').map(|idx| open + 1 + idx)?;
    Some(&expr[open + 1..close])
}

fn report_bare_arith_error(env: &dyn Environment, expr: &str, message: &str) {
    let detail = normalize_arith_message(expr, message);
    match (env.diagnostic_source_name(), env.diagnostic_line()) {
        (Some(source), Some(line)) => eprintln!("{source}: line {line}: {detail}"),
        _ => eprintln!("cherubsh: {detail}"),
    }
}

fn report_let_error(env: &dyn Environment, expr: &str, message: &str) {
    let detail = normalize_arith_message(expr, message);
    match (env.diagnostic_source_name(), env.diagnostic_line()) {
        (Some(source), Some(line)) => {
            if expr.is_empty() {
                eprintln!("{source}: line {line}: let: {detail}");
            } else {
                eprintln!("{source}: line {line}: let: {expr}: {detail}");
            }
        }
        _ if expr.is_empty() => eprintln!("cherubsh: let: {detail}"),
        _ => eprintln!("cherubsh: let: {expr}: {detail}"),
    }
}

fn normalize_arith_message<'a>(expr: &str, message: &'a str) -> &'a str {
    message
        .strip_prefix(expr)
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or(message)
}
