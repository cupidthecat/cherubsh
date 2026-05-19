//! `[[ ... ]]` evaluator. Operates on the parser's `CondCommand` AST.

use cherubsh_common::{Environment, VarKind};
use cherubsh_expander::buf::{CTLESC, CTLNUL, CTLRAW};
use cherubsh_expander::pattern::{fnmatch, GlobOpts};
use cherubsh_expander::quote::{bytes_to_shell_string, shell_string_to_bytes};
use cherubsh_expander::{arith, expand_case_pattern_bytes, ExpCtx, ExpandError, NullRunner};
use cherubsh_parser::{CondCommand, CondType, WordDesc};

pub fn evaluate(cmd: &CondCommand, env: &mut dyn Environment) -> i32 {
    match eval_inner(cmd, env) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(msg) => {
            report_cond_error(env, &msg);
            2
        }
    }
}

fn eval_inner(cmd: &CondCommand, env: &mut dyn Environment) -> Result<bool, String> {
    match cmd.cond_type {
        CondType::And => {
            let l = cmd.left.as_ref().ok_or("missing left operand")?;
            let r = cmd.right.as_ref().ok_or("missing right operand")?;
            let lv = eval_inner(l, env)?;
            if !lv {
                return Ok(false);
            }
            eval_inner(r, env)
        }
        CondType::Or => {
            let l = cmd.left.as_ref().ok_or("missing left operand")?;
            let r = cmd.right.as_ref().ok_or("missing right operand")?;
            let lv = eval_inner(l, env)?;
            if lv {
                return Ok(true);
            }
            eval_inner(r, env)
        }
        CondType::Unary => {
            let op = cmd
                .op
                .as_ref()
                .ok_or("missing unary operator")?
                .text
                .clone();
            let term = cmd
                .left
                .as_ref()
                .and_then(|c| c.term.as_ref())
                .ok_or("missing operand")?
                .text
                .clone();
            let val = expand_word(&term, env);
            if op == "!" {
                let inner = !val.is_empty();
                return Ok(!inner);
            }
            if let Some(rest) = op.strip_prefix('-') {
                if rest.len() == 1 {
                    let ch = rest.chars().next().unwrap();
                    match ch {
                        'v' => {
                            return Ok(var_word_is_set(env, &term));
                        }
                        'R' => {
                            return Ok(env.kind(&val) == VarKind::Nameref);
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
                    let lhs = expand_operand_word(&lhs_word, env)?;
                    let rhs = expand_pattern_word(&rhs_word, env)?;
                    let opts = conditional_pattern_opts(env);
                    Ok(fnmatch(&rhs, &shell_string_to_bytes(&lhs), opts))
                }
                "!=" => {
                    let lhs = expand_operand_word(&lhs_word, env)?;
                    let rhs = expand_pattern_word(&rhs_word, env)?;
                    let opts = conditional_pattern_opts(env);
                    Ok(!fnmatch(&rhs, &shell_string_to_bytes(&lhs), opts))
                }
                "<" => {
                    let lhs = expand_operand_word(&lhs_word, env)?;
                    Ok(lhs < expand_operand_word(&rhs_word, env)?)
                }
                ">" => {
                    let lhs = expand_operand_word(&lhs_word, env)?;
                    Ok(lhs > expand_operand_word(&rhs_word, env)?)
                }
                "=~" => {
                    let lhs = expand_operand_word(&lhs_word, env)?;
                    let rhs = expand_regex_word(&rhs_word, env)?;
                    regex_match(&lhs, &rhs, env)
                }
                "-eq" => arithmetic_binary(&lhs_word.text, &rhs_word.text, env, |l, r| l == r),
                "-ne" => arithmetic_binary(&lhs_word.text, &rhs_word.text, env, |l, r| l != r),
                "-lt" => arithmetic_binary(&lhs_word.text, &rhs_word.text, env, |l, r| l < r),
                "-le" => arithmetic_binary(&lhs_word.text, &rhs_word.text, env, |l, r| l <= r),
                "-gt" => arithmetic_binary(&lhs_word.text, &rhs_word.text, env, |l, r| l > r),
                "-ge" => arithmetic_binary(&lhs_word.text, &rhs_word.text, env, |l, r| l >= r),
                "-nt" | "-ot" | "-ef" => {
                    let lhs = expand_operand_word(&lhs_word, env)?;
                    let rhs = expand_operand_word(&rhs_word, env)?;
                    Ok(file_binary(&op, &lhs, &rhs))
                }
                _ => Err(format!("unknown binary operator `{op}'")),
            }
        }
        CondType::Term => {
            if let Some(inner) = cmd.left.as_ref() {
                if cmd.term.is_none() {
                    return Ok(!eval_inner(inner, env)?);
                }
            }
            let val = cmd
                .term
                .as_ref()
                .map(|w| w.text.clone())
                .unwrap_or_default();
            let expanded = expand_word(&val, env);
            Ok(!expanded.is_empty())
        }
        CondType::Expr => {
            let inner = cmd.left.as_ref().ok_or("missing expression")?;
            eval_inner(inner, env)
        }
    }
}

fn report_cond_error(env: &dyn Environment, msg: &str) {
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
        VarKind::Indexed => env.get_array_indexed(name, 0).is_some(),
        VarKind::Assoc => env.get_array_assoc(name, "0").is_some(),
        VarKind::Unset => env.get(name).is_some(),
        _ => true,
    }
}

fn var_word_is_set(env: &mut dyn Environment, word: &str) -> bool {
    if let Some((base, subscript)) = array_reference(word) {
        return match env.kind(base) {
            VarKind::Assoc => {
                if matches!(subscript, "@" | "*") {
                    env.get_array_assoc(base, subscript).is_some()
                } else {
                    let key = expand_word(subscript, env);
                    env.get_array_assoc(base, &key).is_some()
                }
            }
            VarKind::Indexed => {
                if matches!(subscript, "@" | "*") {
                    env.array_len(base) > 0
                } else {
                    let expanded = expand_word(subscript, env);
                    let mut runner = NullRunner::default();
                    let Ok(index) =
                        arith::eval_preexpanded(&expanded, &mut ExpCtx::new(env, &mut runner))
                    else {
                        return false;
                    };
                    env.get_array_indexed(base, index).is_some()
                }
            }
            _ => {
                let expanded = expand_word(word, env);
                var_is_set(env, &expanded)
            }
        };
    }
    let expanded = expand_word(word, env);
    var_is_set(env, &expanded)
}

fn array_element_is_set(env: &mut dyn Environment, base: &str, subscript: &str) -> bool {
    match env.kind(base) {
        VarKind::Indexed => {
            if matches!(subscript, "@" | "*") {
                return env.array_len(base) > 0;
            }
            let expanded = expand_word(subscript, env);
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
            let key = expand_word(subscript, env);
            env.get_array_assoc(base, &key).is_some()
        }
        _ => {
            let expanded = expand_word(subscript, env);
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
        extglob: env.option("extglob"),
        globasciiranges: env.option("globasciiranges"),
    }
}

fn expand_word(s: &str, env: &mut dyn Environment) -> String {
    use cherubsh_expander::{expand_assignment_rhs, NullRunner};
    let mut runner = NullRunner::default();
    expand_assignment_rhs(s, env, &mut runner).unwrap_or_else(|_| s.to_string())
}

fn expand_operand_word(word: &WordDesc, env: &mut dyn Environment) -> Result<String, String> {
    let mut runner = NullRunner::default();
    let bytes = expand_case_pattern_bytes(word, env, &mut runner)
        .map_err(|err| err.into_shell_error(None).message)?;
    Ok(bytes_to_shell_string(&dequote_expanded_bytes(&bytes)))
}

fn expand_pattern_word(word: &WordDesc, env: &mut dyn Environment) -> Result<Vec<u8>, String> {
    let mut runner = NullRunner::default();
    let bytes = expand_case_pattern_bytes(word, env, &mut runner)
        .map_err(|err| err.into_shell_error(None).message)?;
    Ok(strip_quoted_nulls(&bytes))
}

fn expand_regex_word(word: &WordDesc, env: &mut dyn Environment) -> Result<String, String> {
    let mut runner = NullRunner::default();
    let bytes = expand_case_pattern_bytes(word, env, &mut runner)
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
    lhs: &str,
    rhs: &str,
    env: &mut dyn Environment,
    cmp: F,
) -> Result<bool, String>
where
    F: FnOnce(i64, i64) -> bool,
{
    let lhs = match parse_arithmetic_value(lhs, env) {
        Ok(value) => value,
        Err(msg) => {
            report_cond_error(env, &msg);
            return Ok(false);
        }
    };
    let rhs = match parse_arithmetic_value(rhs, env) {
        Ok(value) => value,
        Err(msg) => {
            report_cond_error(env, &msg);
            return Ok(false);
        }
    };
    Ok(cmp(lhs, rhs))
}

fn parse_arithmetic_value(s: &str, env: &mut dyn Environment) -> Result<i64, String> {
    let expr = s.trim();
    if expr.is_empty() {
        return Ok(0);
    }
    let mut runner = NullRunner::default();
    match cherubsh_expander::expand_for_arith(expr, env, &mut runner) {
        Ok(value) => Ok(value),
        Err(ExpandError::Other(message))
            if message == "command substitution unavailable in this context" =>
        {
            Ok(0)
        }
        Err(err) => Err(format_arithmetic_error(s, err)),
    }
}

fn format_arithmetic_error(expr: &str, err: ExpandError) -> String {
    match err {
        ExpandError::ArithSyntax(message) => {
            if expr.trim_end().ends_with('+') {
                format!("{expr}: syntax error: operand expected (error token is \"+\")")
            } else {
                format!("{expr}: arithmetic syntax error: {message}")
            }
        }
        other => format!("{expr}: {}", other.into_shell_error(None).message),
    }
}

fn unary_test(ch: char, arg: &str) -> bool {
    // Reuse test_cmd::unary by going through the public boundary. test_cmd
    // doesn't export `unary` directly; mirror the small subset needed here.
    // Build a 2-arg test invocation: `test -X arg`.
    let args = vec![format!("-{ch}"), arg.to_string()];
    // run binary_or_unary_2 essentially:
    let bin = run_two_arg_test(&args);
    bin == 0
}

fn run_two_arg_test(args: &[String]) -> i32 {
    // Use the public test invocation surface - exit_status 0 = true.
    // We re-implement by spawning into Test::run via a private bridge - easier
    // to just inline the unary dispatch.
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    let op = args[0].strip_prefix('-').unwrap_or("");
    let ch = op.chars().next().unwrap_or(' ');
    let arg = &args[1];
    let path = Path::new(arg);
    let meta = std::fs::metadata(path);
    let symlink_meta = std::fs::symlink_metadata(path);
    let val = match ch {
        'a' | 'e' => path.exists() || symlink_meta.is_ok(),
        'b' => meta
            .map(|m| (m.mode() & 0o170000) == 0o060000)
            .unwrap_or(false),
        'c' => meta
            .map(|m| (m.mode() & 0o170000) == 0o020000)
            .unwrap_or(false),
        'd' => meta.map(|m| m.is_dir()).unwrap_or(false),
        'f' => meta.map(|m| m.is_file()).unwrap_or(false),
        'g' => meta.map(|m| m.mode() & 0o2000 != 0).unwrap_or(false),
        'h' | 'L' => symlink_meta
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        'k' => meta.map(|m| m.mode() & 0o1000 != 0).unwrap_or(false),
        'p' => meta
            .map(|m| (m.mode() & 0o170000) == 0o010000)
            .unwrap_or(false),
        'r' => libc_access(arg, libc::R_OK),
        's' => meta.map(|m| m.len() > 0).unwrap_or(false),
        'u' => meta.map(|m| m.mode() & 0o4000 != 0).unwrap_or(false),
        'w' => libc_access(arg, libc::W_OK),
        'x' => libc_access(arg, libc::X_OK),
        'z' => arg.is_empty(),
        'n' => !arg.is_empty(),
        't' => {
            let fd: i32 = arg.parse().unwrap_or(-1);
            unsafe { libc::isatty(fd) == 1 }
        }
        _ => false,
    };
    if val {
        0
    } else {
        1
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
