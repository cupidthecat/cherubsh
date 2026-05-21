//! `[[ ... ]]` evaluator. Operates on the parser's `CondCommand` AST.

use cherubsh_common::{Environment, VarKind};
use cherubsh_expander::buf::{CTLESC, CTLNUL, CTLRAW};
use cherubsh_expander::pattern::{fnmatch, GlobOpts};
use cherubsh_expander::quote::{bytes_to_shell_string, shell_string_to_bytes};
use cherubsh_expander::{
    arith, expand_case_pattern_bytes, CommandRunner, ExpCtx, ExpandError, NullRunner,
};
use cherubsh_parser::{CondCommand, CondType, WordDesc};

pub fn evaluate(cmd: &CondCommand, env: &mut dyn Environment) -> i32 {
    let mut runner = NullRunner::default();
    evaluate_with_runner(cmd, env, &mut runner)
}

pub fn evaluate_with_runner(
    cmd: &CondCommand,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> i32 {
    evaluate_with_runner_and_tracer(cmd, env, runner, None)
}

pub fn evaluate_with_runner_and_tracer(
    cmd: &CondCommand,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
    tracer: Option<&mut dyn FnMut(String)>,
) -> i32 {
    match evaluate_with_runner_and_tracer_result(cmd, env, runner, tracer) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(msg) => {
            report_cond_error(env, &msg);
            2
        }
    }
}

pub fn evaluate_with_runner_and_tracer_result(
    cmd: &CondCommand,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
    tracer: Option<&mut dyn FnMut(String)>,
) -> Result<bool, String> {
    let mut tracer = tracer;
    eval_inner(cmd, env, runner, &mut tracer)
}

fn eval_inner(
    cmd: &CondCommand,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
    tracer: &mut Option<&mut dyn FnMut(String)>,
) -> Result<bool, String> {
    match cmd.cond_type {
        CondType::And => {
            let l = cmd.left.as_ref().ok_or("missing left operand")?;
            let r = cmd.right.as_ref().ok_or("missing right operand")?;
            let lv = eval_inner(l, env, runner, tracer)?;
            if !lv {
                return Ok(false);
            }
            eval_inner(r, env, runner, tracer)
        }
        CondType::Or => {
            let l = cmd.left.as_ref().ok_or("missing left operand")?;
            let r = cmd.right.as_ref().ok_or("missing right operand")?;
            let lv = eval_inner(l, env, runner, tracer)?;
            if lv {
                return Ok(true);
            }
            eval_inner(r, env, runner, tracer)
        }
        CondType::Unary => {
            let op = cmd
                .op
                .as_ref()
                .ok_or("missing unary operator")?
                .text
                .as_str();
            let term_word = cmd
                .left
                .as_ref()
                .and_then(|c| c.term.as_ref())
                .ok_or("missing operand")?;
            let term = term_word.text.as_str();
            let val = expand_word(term, env, runner);
            if op == "!" {
                trace_cond_unary(tracer, "-n", &val);
                let inner = !val.is_empty();
                return Ok(!inner);
            }
            trace_cond_unary(tracer, op, &val);
            if let Some(rest) = op.strip_prefix('-') {
                if rest.len() == 1 {
                    let ch = rest.chars().next().unwrap();
                    match ch {
                        'v' => {
                            return Ok(var_word_is_set(env, &term, runner));
                        }
                        'R' => {
                            return Ok(env.kind(&val) == VarKind::Nameref);
                        }
                        'o' => {
                            return Ok(env.option(&val));
                        }
                        't' => {
                            if val.trim().parse::<i32>().is_err() {
                                return Err(format!("{val}: integer expected"));
                            }
                            return Ok(unary_test(ch, &val));
                        }
                        _ => {
                            return Ok(unary_test(ch, &val));
                        }
                    }
                }
            }
            Err(format!("unknown unary operator `{op}'"))
        }
        CondType::Binary => {
            let op = cmd
                .op
                .as_ref()
                .ok_or("missing binary operator")?
                .text
                .clone();
            let lhs_word = cmd
                .left
                .as_ref()
                .and_then(|c| c.term.as_ref())
                .ok_or("missing left operand")?
                .clone();
            let rhs_word = cmd
                .right
                .as_ref()
                .and_then(|c| c.term.as_ref())
                .ok_or("missing right operand")?
                .clone();
            // RHS for == / != / = / != is a pattern; for =~ it's a regex; for
            // arithmetic it's a number. Expand without word splitting either way.
            match op.as_str() {
                "==" | "=" => {
                    let lhs = expand_operand_word(&lhs_word, env, runner)?;
                    let rhs = expand_pattern_word(&rhs_word, env, runner)?;
                    trace_cond_binary(tracer, &lhs, &op, &bytes_to_shell_string(&rhs));
                    let opts = conditional_pattern_opts(env);
                    Ok(fnmatch(&rhs, &shell_string_to_bytes(&lhs), opts))
                }
                "!=" => {
                    let lhs = expand_operand_word(&lhs_word, env, runner)?;
                    let rhs = expand_pattern_word(&rhs_word, env, runner)?;
                    trace_cond_binary(tracer, &lhs, &op, &bytes_to_shell_string(&rhs));
                    let opts = conditional_pattern_opts(env);
                    Ok(!fnmatch(&rhs, &shell_string_to_bytes(&lhs), opts))
                }
                "<" => {
                    let lhs = expand_operand_word(&lhs_word, env, runner)?;
                    let rhs = expand_operand_word(&rhs_word, env, runner)?;
                    trace_cond_binary(tracer, &lhs, &op, &rhs);
                    Ok(lhs < rhs)
                }
                ">" => {
                    let lhs = expand_operand_word(&lhs_word, env, runner)?;
                    let rhs = expand_operand_word(&rhs_word, env, runner)?;
                    trace_cond_binary(tracer, &lhs, &op, &rhs);
                    Ok(lhs > rhs)
                }
                "=~" => {
                    let lhs = expand_operand_word(&lhs_word, env, runner)?;
                    let rhs = expand_regex_word(&rhs_word, env, runner)?;
                    trace_cond_binary(tracer, &lhs, &op, &rhs);
                    regex_match(&lhs, &rhs, env)
                }
                "-eq" => {
                    arithmetic_binary(&lhs_word, &rhs_word, &op, env, runner, tracer, |l, r| {
                        l == r
                    })
                }
                "-ne" => {
                    arithmetic_binary(&lhs_word, &rhs_word, &op, env, runner, tracer, |l, r| {
                        l != r
                    })
                }
                "-lt" => {
                    arithmetic_binary(&lhs_word, &rhs_word, &op, env, runner, tracer, |l, r| l < r)
                }
                "-le" => {
                    arithmetic_binary(&lhs_word, &rhs_word, &op, env, runner, tracer, |l, r| {
                        l <= r
                    })
                }
                "-gt" => {
                    arithmetic_binary(&lhs_word, &rhs_word, &op, env, runner, tracer, |l, r| l > r)
                }
                "-ge" => {
                    arithmetic_binary(&lhs_word, &rhs_word, &op, env, runner, tracer, |l, r| {
                        l >= r
                    })
                }
                "-nt" | "-ot" | "-ef" => {
                    let lhs = expand_operand_word(&lhs_word, env, runner)?;
                    let rhs = expand_operand_word(&rhs_word, env, runner)?;
                    Ok(file_binary(&op, &lhs, &rhs))
                }
                _ => Err(format!("unknown binary operator `{op}'")),
            }
        }
        CondType::Term => {
            if let Some(inner) = cmd.left.as_ref() {
                if cmd.term.is_none() {
                    return Ok(!eval_inner(inner, env, runner, tracer)?);
                }
            }
            let val = cmd
                .term
                .as_ref()
                .map(|w| w.text.clone())
                .unwrap_or_default();
            let expanded = expand_word(&val, env, runner);
            trace_cond_unary(tracer, "-n", &expanded);
            Ok(!expanded.is_empty())
        }
        CondType::Expr => {
            let inner = cmd.left.as_ref().ok_or("missing expression")?;
            eval_inner(inner, env, runner, tracer)
        }
    }
}

fn trace_cond_binary(tracer: &mut Option<&mut dyn FnMut(String)>, lhs: &str, op: &str, rhs: &str) {
    if let Some(tracer) = tracer.as_deref_mut() {
        tracer(format!(
            "[[ {} {op} {} ]]",
            trace_cond_value(lhs),
            trace_cond_value(rhs)
        ));
    }
}

fn trace_cond_unary(tracer: &mut Option<&mut dyn FnMut(String)>, op: &str, value: &str) {
    if let Some(tracer) = tracer.as_deref_mut() {
        tracer(format!("[[ {op} {} ]]", trace_cond_value(value)));
    }
}

fn trace_cond_value(value: &str) -> String {
    if value.is_empty() {
        "''".to_string()
    } else {
        value.to_string()
    }
}

pub fn report_cond_error(env: &dyn Environment, msg: &str) {
    match (env.diagnostic_source_name(), env.diagnostic_line()) {
        (Some(source), Some(line)) => eprintln!("{source}: line {line}: [[: {msg}"),
        _ => eprintln!("cherubsh: [[: {msg}"),
    }
}

fn var_is_set(env: &mut dyn Environment, name: &str) -> bool {
    if let Some((base, subscript)) = array_reference(name) {
        return array_element_is_set(env, base, subscript);
    }
    match env.kind(name) {
        VarKind::Nameref => {
            let Some(target) = env.resolve_nameref(name) else {
                return false;
            };
            if target == name {
                return env.get(name).is_some();
            }
            if array_reference(&target).is_some() {
                return false;
            }
            var_is_set(env, &target)
        }
        VarKind::Indexed => env.get_array_indexed(name, 0).is_some(),
        VarKind::Assoc => env.get_array_assoc(name, "0").is_some(),
        VarKind::Scalar | VarKind::Unset => env.get(name).is_some(),
    }
}

fn var_word_is_set(env: &mut dyn Environment, word: &str, runner: &mut dyn CommandRunner) -> bool {
    if let Some((base, subscript)) = array_reference(word) {
        return match env.kind(base) {
            VarKind::Assoc => {
                if matches!(subscript, "@" | "*") {
                    env.get_array_assoc(base, subscript).is_some()
                } else {
                    let key = expand_word(subscript, env, runner);
                    env.get_array_assoc(base, &key).is_some()
                }
            }
            VarKind::Indexed => {
                if matches!(subscript, "@" | "*") {
                    env.array_len(base) > 0
                } else {
                    let expanded = expand_word(subscript, env, runner);
                    let mut null_runner = NullRunner::default();
                    let Ok(index) =
                        arith::eval_preexpanded(&expanded, &mut ExpCtx::new(env, &mut null_runner))
                    else {
                        return false;
                    };
                    env.get_array_indexed(base, index).is_some()
                }
            }
            VarKind::Nameref => {
                let Some(target) = env.resolve_nameref(base) else {
                    return false;
                };
                if target == base || array_reference(&target).is_some() {
                    return false;
                }
                array_element_is_set(env, &target, subscript)
            }
            _ => {
                let expanded = expand_word(word, env, runner);
                var_is_set(env, &expanded)
            }
        };
    }
    let expanded = expand_word(word, env, runner);
    var_is_set(env, &expanded)
}

fn array_element_is_set(env: &mut dyn Environment, base: &str, subscript: &str) -> bool {
    match env.kind(base) {
        VarKind::Nameref => {
            let Some(target) = env.resolve_nameref(base) else {
                return false;
            };
            if target == base || array_reference(&target).is_some() {
                return false;
            }
            array_element_is_set(env, &target, subscript)
        }
        VarKind::Indexed => {
            if matches!(subscript, "@" | "*") {
                return env.array_len(base) > 0;
            }
            let mut expand_runner = NullRunner::default();
            let expanded = expand_word(subscript, env, &mut expand_runner);
            let mut runner = NullRunner::default();
            let Ok(index) = arith::eval_preexpanded(&expanded, &mut ExpCtx::new(env, &mut runner))
            else {
                return false;
            };
            env.get_array_indexed(base, index).is_some()
        }
        VarKind::Assoc => {
            if matches!(subscript, "@" | "*") {
                return env.assoc_keys(base).is_some_and(|keys| !keys.is_empty());
            }
            let mut expand_runner = NullRunner::default();
            let key = expand_word(subscript, env, &mut expand_runner);
            env.get_array_assoc(base, &key).is_some()
        }
        _ => {
            let mut expand_runner = NullRunner::default();
            let expanded = expand_word(subscript, env, &mut expand_runner);
            let mut runner = NullRunner::default();
            let Ok(index) = arith::eval_preexpanded(&expanded, &mut ExpCtx::new(env, &mut runner))
            else {
                return false;
            };
            index == 0 && env.get(base).is_some()
        }
    }
}

fn array_reference(name: &str) -> Option<(&str, &str)> {
    let open = name.find('[')?;
    let close = name.rfind(']')?;
    if close + 1 != name.len() {
        return None;
    }
    let base = &name[..open];
    if base.is_empty() {
        return None;
    }
    Some((base, &name[open + 1..close]))
}

fn conditional_pattern_opts(env: &mut dyn Environment) -> GlobOpts {
    GlobOpts {
        nocaseglob: env.option("nocasematch"),
        extglob: true,
        globasciiranges: env.option("globasciiranges"),
    }
}

fn expand_word(s: &str, env: &mut dyn Environment, runner: &mut dyn CommandRunner) -> String {
    if let Some(name) = simple_parameter_word(s) {
        return env.get(name).unwrap_or_default();
    }
    use cherubsh_expander::expand_assignment_rhs;
    expand_assignment_rhs(s, env, runner).unwrap_or_else(|_| s.to_string())
}

fn simple_parameter_word(s: &str) -> Option<&str> {
    let name = s.strip_prefix('$')?;
    if name.is_empty() {
        return None;
    }
    if matches!(name, "?" | "#" | "$" | "!" | "-") || name.bytes().all(|b| b.is_ascii_digit()) {
        return Some(name);
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    chars
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        .then_some(name)
}

fn expand_operand_word(
    word: &WordDesc,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, String> {
    let bytes = expand_case_pattern_bytes(word, env, runner)
        .map_err(|err| err.into_shell_error(None).message)?;
    Ok(bytes_to_shell_string(&dequote_expanded_bytes(&bytes)))
}

fn expand_pattern_word(
    word: &WordDesc,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Vec<u8>, String> {
    let bytes = expand_case_pattern_bytes(word, env, runner)
        .map_err(|err| err.into_shell_error(None).message)?;
    Ok(strip_quoted_nulls(&bytes))
}

fn expand_regex_word(
    word: &WordDesc,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, String> {
    let bytes = expand_case_pattern_bytes(word, env, runner)
        .map_err(|err| err.into_shell_error(None).message)?;
    Ok(regex_quote_marked_bytes(&bytes))
}

fn strip_quoted_nulls(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == CTLESC && bytes[i + 1] == CTLRAW {
            out.extend_from_slice(&bytes[i..i + 3]);
            i += 3;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == CTLESC && bytes[i + 1] == CTLNUL {
            out.extend_from_slice(&bytes[i..i + 2]);
            i += 2;
            continue;
        }
        let b = bytes[i];
        if b == CTLNUL && i > 0 && bytes[i - 1] == b'\\' {
            out.push(b);
        } else if b != CTLNUL {
            out.push(b);
        }
        i += 1;
    }
    out
}

fn dequote_expanded_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            CTLESC if i + 2 < bytes.len() && bytes[i + 1] == CTLRAW => {
                out.push(bytes[i + 2]);
                i += 3;
            }
            CTLESC if i + 1 < bytes.len() => {
                out.push(bytes[i + 1]);
                i += 2;
            }
            CTLESC | CTLNUL => {
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn regex_quote_marked_bytes(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    let mut bracket = false;
    let mut bracket_index = 0usize;
    let mut bracket_leading_caret = false;
    while i < bytes.len() {
        match bytes[i] {
            CTLESC if i + 2 < bytes.len() && bytes[i + 1] == CTLRAW => {
                push_regex_literal_byte(&mut out, bytes[i + 2], bracket);
                i += 3;
            }
            CTLESC if i + 1 < bytes.len() => {
                push_regex_literal_byte(&mut out, bytes[i + 1], bracket);
                i += 2;
            }
            CTLESC | CTLNUL => {
                i += 1;
            }
            b => {
                out.push(b);
                update_regex_bracket_state(
                    b,
                    &mut bracket,
                    &mut bracket_index,
                    &mut bracket_leading_caret,
                );
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn update_regex_bracket_state(
    b: u8,
    bracket: &mut bool,
    bracket_index: &mut usize,
    bracket_leading_caret: &mut bool,
) {
    if *bracket {
        let literal_closing_bracket =
            b == b']' && (*bracket_index == 0 || (*bracket_index == 1 && *bracket_leading_caret));
        if b == b']' && !literal_closing_bracket {
            *bracket = false;
            *bracket_index = 0;
            *bracket_leading_caret = false;
            return;
        }
        if *bracket_index == 0 && b == b'^' {
            *bracket_leading_caret = true;
        }
        *bracket_index += 1;
    } else if b == b'[' {
        *bracket = true;
        *bracket_index = 0;
        *bracket_leading_caret = false;
    }
}

fn push_regex_literal_byte(out: &mut Vec<u8>, b: u8, bracket: bool) {
    if !bracket
        && matches!(
            b,
            b'.' | b'^'
                | b'$'
                | b'*'
                | b'+'
                | b'?'
                | b'('
                | b')'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'|'
                | b'\\'
        )
    {
        out.push(b'\\');
    }
    out.push(b);
}

fn arithmetic_binary<F>(
    lhs: &WordDesc,
    rhs: &WordDesc,
    op: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
    tracer: &mut Option<&mut dyn FnMut(String)>,
    cmp: F,
) -> Result<bool, String>
where
    F: FnOnce(i64, i64) -> bool,
{
    let lhs_expr = lhs.text.as_str();
    let lhs = match parse_arithmetic_value(lhs_expr, env, runner) {
        Ok(value) => value,
        Err(msg) => {
            report_cond_error(env, &msg);
            return Ok(false);
        }
    };
    let rhs_expr = rhs.text.as_str();
    let rhs = match parse_arithmetic_value(rhs_expr, env, runner) {
        Ok(value) => value,
        Err(msg) => {
            report_cond_error(env, &msg);
            return Ok(false);
        }
    };
    trace_cond_binary(tracer, &lhs.to_string(), op, &rhs.to_string());
    Ok(cmp(lhs, rhs))
}

fn parse_arithmetic_value(
    s: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<i64, String> {
    let expr = s.trim();
    if expr.is_empty() {
        return Ok(0);
    }
    let expr = quote_remove_whole_operand(expr);
    match cherubsh_expander::expand_for_arith(&expr, env, runner) {
        Ok(value) => Ok(value),
        Err(ExpandError::Other(message))
            if message == "command substitution unavailable in this context" =>
        {
            Ok(0)
        }
        Err(err) => Err(format_arithmetic_error(&expr, err)),
    }
}

fn quote_remove_whole_operand(expr: &str) -> String {
    let bytes = expr.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'\'' {
        if let Some(end) = bytes[1..].iter().position(|b| *b == b'\'') {
            let close = end + 1;
            if close + 1 == bytes.len() {
                return expr[1..close].to_string();
            }
        }
    }
    if bytes.len() >= 2 && bytes[0] == b'"' {
        let mut out = String::new();
        let mut i = 1usize;
        while i < bytes.len() {
            match bytes[i] {
                b'"' if i + 1 == bytes.len() => return out,
                b'\\' if i + 1 < bytes.len() => {
                    let next = bytes[i + 1];
                    if matches!(next, b'"' | b'\\' | b'$' | b'`' | b'\n') {
                        if next != b'\n' {
                            out.push(next as char);
                        }
                        i += 2;
                    } else {
                        out.push('\\');
                        i += 1;
                    }
                }
                b => {
                    out.push(b as char);
                    i += 1;
                }
            }
        }
    }
    expr.to_string()
}

fn format_arithmetic_error(expr: &str, err: ExpandError) -> String {
    match err {
        ExpandError::ArithSyntax(message) => {
            if expr.trim_end().ends_with('+') {
                format!("{expr}: arithmetic syntax error: operand expected (error token is \"+\")")
            } else if message.contains(": arithmetic syntax error")
                || message.contains(": syntax error")
            {
                message
            } else {
                format!("{expr}: arithmetic syntax error: {message}")
            }
        }
        other => format!("{expr}: {}", other.into_shell_error(None).message),
    }
}

fn unary_test(ch: char, arg: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    let path = Path::new(arg);
    match ch {
        'a' | 'e' => path.exists() || std::fs::symlink_metadata(path).is_ok(),
        'h' | 'L' => std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        'r' => libc_access(arg, libc::R_OK),
        'w' => libc_access(arg, libc::W_OK),
        'x' => libc_access(arg, libc::X_OK),
        'z' => arg.is_empty(),
        'n' => !arg.is_empty(),
        't' => {
            let fd: i32 = arg.trim().parse().unwrap_or(-1);
            unsafe { libc::isatty(fd) == 1 }
        }
        _ => {
            let meta = std::fs::metadata(path);
            match ch {
                'b' => meta
                    .map(|m| (m.mode() & 0o170000) == 0o060000)
                    .unwrap_or(false),
                'c' => meta
                    .map(|m| (m.mode() & 0o170000) == 0o020000)
                    .unwrap_or(false),
                'd' => meta.map(|m| m.is_dir()).unwrap_or(false),
                'f' => meta.map(|m| m.is_file()).unwrap_or(false),
                'g' => meta.map(|m| m.mode() & 0o2000 != 0).unwrap_or(false),
                'k' => meta.map(|m| m.mode() & 0o1000 != 0).unwrap_or(false),
                'p' => meta
                    .map(|m| (m.mode() & 0o170000) == 0o010000)
                    .unwrap_or(false),
                's' => meta.map(|m| m.len() > 0).unwrap_or(false),
                'u' => meta.map(|m| m.mode() & 0o4000 != 0).unwrap_or(false),
                _ => false,
            }
        }
    }
}

fn libc_access(path: &str, mode: i32) -> bool {
    let c = std::ffi::CString::new(path).unwrap_or_default();
    unsafe { libc::access(c.as_ptr(), mode) == 0 }
}

fn file_binary(op: &str, a: &str, b: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let am = std::fs::metadata(a);
    let bm = std::fs::metadata(b);
    match op {
        "-nt" => match (am, bm) {
            (Ok(am), Ok(bm)) => am.mtime() > bm.mtime(),
            (Ok(_), Err(_)) => true,
            _ => false,
        },
        "-ot" => match (am, bm) {
            (Ok(am), Ok(bm)) => am.mtime() < bm.mtime(),
            (Err(_), Ok(_)) => true,
            _ => false,
        },
        "-ef" => match (am, bm) {
            (Ok(am), Ok(bm)) => am.dev() == bm.dev() && am.ino() == bm.ino(),
            _ => false,
        },
        _ => false,
    }
}

fn regex_match(text: &str, pat: &str, env: &mut dyn Environment) -> Result<bool, String> {
    use std::ffi::CString;
    use std::os::raw::c_int;

    let c_text =
        CString::new(text).map_err(|_| "regular expression text contains NUL".to_string())?;
    let c_pat = CString::new(pat).map_err(|_| "regular expression contains NUL".to_string())?;
    let mut raw_regex = unsafe { std::mem::zeroed::<libc::regex_t>() };
    let mut flags: c_int = libc::REG_EXTENDED;
    if env.option("nocasematch") {
        flags |= libc::REG_ICASE;
    }
    let compile_status = unsafe { libc::regcomp(&mut raw_regex, c_pat.as_ptr(), flags) };
    if compile_status != 0 {
        if pat == "[." {
            env.set_array("BASH_REMATCH", Vec::new());
            return Ok(false);
        }
        let message = regex_error_message(compile_status, &raw_regex);
        return Err(format!("invalid regular expression `{pat}': {message}"));
    }

    struct RegexGuard(libc::regex_t);
    impl Drop for RegexGuard {
        fn drop(&mut self) {
            unsafe { libc::regfree(&mut self.0) };
        }
    }

    let regex = RegexGuard(raw_regex);
    let nmatch = count_ere_subexpressions(pat).saturating_add(1);
    let mut matches = vec![
        libc::regmatch_t {
            rm_so: -1,
            rm_eo: -1,
        };
        nmatch
    ];
    let exec_status = unsafe {
        libc::regexec(
            &regex.0,
            c_text.as_ptr(),
            matches.len(),
            matches.as_mut_ptr(),
            0,
        )
    };
    if exec_status == libc::REG_NOMATCH {
        env.set_array("BASH_REMATCH", Vec::new());
        return Ok(false);
    }
    if exec_status != 0 {
        return Err(regex_error_message(exec_status, &regex.0));
    }

    let mut captures = Vec::with_capacity(matches.len());
    for mat in matches {
        if mat.rm_so >= 0 && mat.rm_eo >= mat.rm_so {
            let start = mat.rm_so as usize;
            let end = mat.rm_eo as usize;
            captures.push(text.get(start..end).unwrap_or_default().to_string());
        } else {
            captures.push(String::new());
        }
    }
    env.set_array("BASH_REMATCH", captures);
    Ok(true)
}

fn regex_error_message(code: i32, regex: &libc::regex_t) -> String {
    let len = unsafe { libc::regerror(code, regex, std::ptr::null_mut(), 0) };
    if len == 0 {
        return "regular expression error".to_string();
    }
    let mut buf = vec![0u8; len];
    unsafe {
        libc::regerror(
            code,
            regex,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
        );
    }
    if buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn count_ere_subexpressions(pat: &str) -> usize {
    let mut count = 0usize;
    let mut escaped = false;
    let mut bracket = false;
    for ch in pat.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if bracket {
            if ch == ']' {
                bracket = false;
            }
            continue;
        }
        match ch {
            '[' => bracket = true,
            '(' => count += 1,
            _ => {}
        }
    }
    count
}

pub fn evaluate_status(cmd: &CondCommand, env: &mut dyn Environment) -> i32 {
    evaluate(cmd, env)
}
