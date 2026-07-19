//! POSIX `test` / `[` implementation. Mirrors bash 5.2.21 `test.c`:
//! handles unary file/string ops, string comparison, arithmetic comparison,
//! binary file ops (-nt/-ot/-ef), logical operators with bash precedence,
//! and parenthesization.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::common::{
    indexed_target_array_expand_once_message, report_bare_diagnostic, report_diagnostic,
};
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::{Environment, VarKind, W_ARRAYREF, W_QUOTED};

pub struct Test;
pub static TEST: Test = Test;
pub struct LBracket;
pub static LBRACKET: LBracket = LBracket;

impl Builtin for Test {
    fn name(&self) -> &'static str {
        "test"
    }
    fn synopsis(&self) -> &'static str {
        "test [expr] / [ expr ]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_test(ctx.args, ctx.arg_flags, false, Some(ctx.env_ref()))
    }
}

impl Builtin for LBracket {
    fn name(&self) -> &'static str {
        "["
    }
    fn synopsis(&self) -> &'static str {
        "[ expr ]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_test(ctx.args, ctx.arg_flags, true, Some(ctx.env_ref()))
    }
}

fn run_test(
    raw: &[String],
    raw_flags: &[u32],
    require_bracket: bool,
    env: Option<&dyn Environment>,
) -> i32 {
    let mut args: Vec<String> = raw.to_vec();
    let mut flags: Vec<u32> = raw_flags.to_vec();
    let subject = if require_bracket { "[" } else { "test" };
    if require_bracket {
        if args.last().map(|s| s.as_str()) != Some("]") {
            report_test_error(env, subject, "missing `]'");
            return 2;
        }
        args.pop();
        flags.pop();
    }

    let n = args.len();
    match n {
        0 => 1,
        1 => bool_to_status(!args[0].is_empty()),
        2 => binary_or_unary_2(&args, &flags, env, subject),
        3 => ternary(&args, env, subject),
        4 => quaternary(&args, env, require_bracket, subject),
        _ => {
            let mut parser = Parser::new(&args, env, require_bracket);
            match parser.parse_or() {
                Ok(val) => {
                    if parser.idx != args.len() {
                        let msg = parser
                            .peek()
                            .filter(|tok| tok.starts_with('-'))
                            .map(|tok| format!("syntax error: `{tok}' unexpected"))
                            .unwrap_or_else(|| "too many arguments".to_string());
                        report_test_error(env, subject, &msg);
                        2
                    } else {
                        bool_to_status(val)
                    }
                }
                Err(msg) => {
                    report_test_error(env, subject, &msg);
                    2
                }
            }
        }
    }
}

fn binary_or_unary_2(
    args: &[String],
    flags: &[u32],
    env: Option<&dyn Environment>,
    subject: &str,
) -> i32 {
    if args[0] == "!" {
        return bool_to_status(args[1].is_empty());
    }
    if let Some(op) = args[0].strip_prefix('-') {
        if op.len() == 1 && is_unary_op(op.chars().next().unwrap()) {
            let ch = op.chars().next().unwrap();
            if ch == 'v'
                && flags.get(1).copied().unwrap_or(0) & W_ARRAYREF != 0
                && flags.get(1).copied().unwrap_or(0) & W_QUOTED == 0
                && args[1].contains("$(")
                && args[1].contains(char::is_whitespace)
            {
                let first = args[1].split_whitespace().next().unwrap_or(&args[1]);
                report_test_error(env, subject, &format!("{first}: binary operator expected"));
                return 2;
            }
            if ch == 'v' {
                if let Some(env) = env {
                    if let Some(message) = indexed_target_array_expand_once_message(env, &args[1]) {
                        report_bare_diagnostic(env, &message);
                        return 2;
                    }
                }
            }
            return match eval_unary(ch, &args[1], env) {
                Ok(val) => bool_to_status(val),
                Err(msg) => {
                    report_test_error(env, subject, &msg);
                    2
                }
            };
        }
    }
    report_test_error(
        env,
        subject,
        &format!("{}: unary operator expected", args[0]),
    );
    2
}

fn ternary(args: &[String], env: Option<&dyn Environment>, subject: &str) -> i32 {
    if args[1] == "-a" {
        return bool_to_status(!args[0].is_empty() && !args[2].is_empty());
    }
    if args[1] == "-o" {
        return bool_to_status(!args[0].is_empty() || !args[2].is_empty());
    }
    if is_binary_op(&args[1]) {
        return match eval_binary_op(&args[1], &args[0], &args[2]) {
            Ok(val) => bool_to_status(val),
            Err(msg) => {
                report_test_error(env, subject, &msg);
                2
            }
        };
    }
    if args[0] == "!" {
        return invert(binary_or_unary_2(&args[1..], &[], env, subject));
    }
    if args[0] == "(" && args[2] == ")" {
        return bool_to_status(!args[1].is_empty());
    }
    report_test_error(
        env,
        subject,
        &format!("{}: binary operator expected", args[1]),
    );
    2
}

fn quaternary(
    args: &[String],
    env: Option<&dyn Environment>,
    require_bracket: bool,
    subject: &str,
) -> i32 {
    if args[0] == "!" {
        return invert(ternary(&args[1..], env, subject));
    }
    if args[0] == "(" && args[3] == ")" {
        return binary_or_unary_2(&args[1..3], &[], env, subject);
    }
    let mut parser = Parser::new(args, env, require_bracket);
    match parser.parse_or() {
        Ok(val) => {
            if parser.idx != args.len() {
                let msg = parser
                    .peek()
                    .filter(|tok| tok.starts_with('-'))
                    .map(|tok| format!("syntax error: `{tok}' unexpected"))
                    .unwrap_or_else(|| "too many arguments".to_string());
                report_test_error(env, subject, &msg);
                2
            } else {
                bool_to_status(val)
            }
        }
        Err(msg) => {
            report_test_error(env, subject, &msg);
            2
        }
    }
}

struct Parser<'a> {
    args: &'a [String],
    idx: usize,
    env: Option<&'a dyn Environment>,
    require_bracket: bool,
}
impl<'a> Parser<'a> {
    fn new(args: &'a [String], env: Option<&'a dyn Environment>, require_bracket: bool) -> Self {
        Self {
            args,
            idx: 0,
            env,
            require_bracket,
        }
    }
    fn peek(&self) -> Option<&str> {
        self.args.get(self.idx).map(String::as_str)
    }
    fn eat(&mut self) -> Option<&str> {
        let v = self.args.get(self.idx).map(String::as_str);
        if v.is_some() {
            self.idx += 1;
        }
        v
    }
    fn parse_or(&mut self) -> Result<bool, String> {
        let mut lhs = self.parse_and()?;
        while self.peek() == Some("-o") {
            self.eat();
            let rhs = self.parse_and()?;
            lhs = lhs || rhs;
        }
        Ok(lhs)
    }
    fn parse_and(&mut self) -> Result<bool, String> {
        let mut lhs = self.parse_unary()?;
        while self.peek() == Some("-a") {
            self.eat();
            let rhs = self.parse_unary()?;
            lhs = lhs && rhs;
        }
        Ok(lhs)
    }
    fn parse_unary(&mut self) -> Result<bool, String> {
        if self.peek() == Some("!") {
            self.eat();
            let v = self.parse_unary()?;
            return Ok(!v);
        }
        if self.peek() == Some("(") {
            self.eat();
            let v = self.parse_or()?;
            let closer = self.eat().map(str::to_string);
            if closer.as_deref() != Some(")") {
                if self.require_bracket && closer.is_none() {
                    return Err("`)' expected, found ]".into());
                }
                return Err("`)' expected".into());
            }
            return Ok(v);
        }
        let tok = self
            .eat()
            .ok_or_else(|| "argument expected".to_string())?
            .to_string();
        // Unary?
        if let Some(op) = tok.strip_prefix('-') {
            if op.len() == 1 && is_unary_op(op.chars().next().unwrap()) {
                let Some(arg) = self.eat().map(str::to_string) else {
                    return Ok(!tok.is_empty());
                };
                if arg == ")" {
                    self.idx = self.idx.saturating_sub(1);
                    return Ok(!tok.is_empty());
                }
                return eval_unary_expr(op.chars().next().unwrap(), &arg, self.env);
            }
        }
        // Then check for binary
        let op_owned = self.peek().map(|s| s.to_string());
        if let Some(op) = op_owned {
            if is_binary_op(&op) {
                self.eat();
                let rhs = self
                    .eat()
                    .ok_or_else(|| format!("syntax error: `{op}' unexpected"))?
                    .to_string();
                return eval_binary_op(&op, &tok, &rhs);
            }
        }
        Ok(!tok.is_empty())
    }
}

fn is_unary_op(c: char) -> bool {
    matches!(
        c,
        'a' | 'b'
            | 'c'
            | 'd'
            | 'e'
            | 'f'
            | 'g'
            | 'h'
            | 'k'
            | 'L'
            | 'N'
            | 'O'
            | 'G'
            | 'p'
            | 'r'
            | 's'
            | 'S'
            | 't'
            | 'u'
            | 'w'
            | 'x'
            | 'z'
            | 'n'
            | 'o'
            | 'v'
            | 'R'
    )
}

fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "=="
            | "!="
            | "<"
            | ">"
            | "-eq"
            | "-ne"
            | "-lt"
            | "-le"
            | "-gt"
            | "-ge"
            | "-nt"
            | "-ot"
            | "-ef"
    )
}

fn eval_binary_op(op: &str, lhs: &str, rhs: &str) -> Result<bool, String> {
    match op {
        "=" | "==" => Ok(lhs == rhs),
        "!=" => Ok(lhs != rhs),
        "<" => Ok(lhs < rhs),
        ">" => Ok(lhs > rhs),
        "-eq" => Ok(parse_test_int(lhs)? == parse_test_int(rhs)?),
        "-ne" => Ok(parse_test_int(lhs)? != parse_test_int(rhs)?),
        "-lt" => Ok(parse_test_int(lhs)? < parse_test_int(rhs)?),
        "-le" => Ok(parse_test_int(lhs)? <= parse_test_int(rhs)?),
        "-gt" => Ok(parse_test_int(lhs)? > parse_test_int(rhs)?),
        "-ge" => Ok(parse_test_int(lhs)? >= parse_test_int(rhs)?),
        "-nt" => Ok(newer_than(lhs, rhs)),
        "-ot" => Ok(older_than(lhs, rhs)),
        "-ef" => Ok(same_file(lhs, rhs)),
        _ => Err(format!("{op}: binary operator expected")),
    }
}

fn eval_unary(op: char, arg: &str, env: Option<&dyn Environment>) -> Result<bool, String> {
    if op == 't' && arg.trim().parse::<i32>().is_err() {
        return Err(format!("{arg}: integer expected"));
    }
    Ok(unary(op, arg, env))
}

fn eval_unary_expr(op: char, arg: &str, env: Option<&dyn Environment>) -> Result<bool, String> {
    if op == 't' && arg == "0" {
        return Ok(true);
    }
    eval_unary(op, arg, env)
}

fn parse_test_int(s: &str) -> Result<i64, String> {
    s.trim().parse().map_err(|_| {
        if std::env::var("CHERUBSH_BASH_COMPAT_VERSION")
            .ok()
            .is_some_and(|version| version.starts_with("5.2"))
        {
            format!("{s}: integer expression expected")
        } else {
            format!("{s}: integer expected")
        }
    })
}

fn unary(op: char, arg: &str, env: Option<&dyn Environment>) -> bool {
    let path = Path::new(arg);
    let meta = std::fs::metadata(path);
    let symlink_meta = std::fs::symlink_metadata(path);
    match op {
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
        'O' => meta
            .map(|m| m.uid() == unsafe { libc::geteuid() } as u32)
            .unwrap_or(false),
        'G' => meta
            .map(|m| m.gid() == unsafe { libc::getegid() } as u32)
            .unwrap_or(false),
        'N' => meta.map(|m| m.mtime() > m.atime()).unwrap_or(false),
        'S' => meta
            .map(|m| (m.mode() & 0o170000) == 0o140000)
            .unwrap_or(false),
        't' => {
            let fd: i32 = arg.trim().parse().unwrap_or(-1);
            unsafe { libc::isatty(fd) == 1 }
        }
        'z' => arg.is_empty(),
        'n' => !arg.is_empty(),
        'o' => env.map(|env| env.option(arg)).unwrap_or(false),
        'v' => env.map(|env| variable_is_set(env, arg)).unwrap_or(false),
        'R' => env
            .map(|env| env.kind(arg) == VarKind::Nameref)
            .unwrap_or(false),
        _ => false,
    }
}

fn variable_is_set(env: &dyn Environment, arg: &str) -> bool {
    if let Some((name, subscript)) = arg.split_once('[') {
        if let Some(key) = subscript.strip_suffix(']') {
            if key == "@" || key == "*" {
                return match env.kind(name) {
                    VarKind::Assoc => env.get_array_assoc(name, key).is_some(),
                    VarKind::Indexed => env.array_len(name) > 0,
                    _ => env.get(name).is_some(),
                };
            }
            if env.kind(name) == VarKind::Assoc
                && !env.option("assoc_expand_once")
                && (key.contains("$(") || key.contains('`'))
            {
                return false;
            }
            if let Ok(index) = key.parse::<i64>() {
                return env.get_array_indexed(name, index).is_some();
            }
            return env.get_array_assoc(name, key).is_some();
        }
    }
    match env.kind(arg) {
        VarKind::Assoc => env.get_array_assoc(arg, "0").is_some(),
        VarKind::Indexed => env.get_array_indexed(arg, 0).is_some(),
        VarKind::Unset => false,
        _ => true,
    }
}

fn newer_than(a: &str, b: &str) -> bool {
    let am = std::fs::metadata(a);
    let bm = std::fs::metadata(b);
    match (am, bm) {
        (Ok(am), Ok(bm)) => am.mtime() > bm.mtime(),
        (Ok(_), Err(_)) => true,
        _ => false,
    }
}

fn older_than(a: &str, b: &str) -> bool {
    let am = std::fs::metadata(a);
    let bm = std::fs::metadata(b);
    match (am, bm) {
        (Ok(am), Ok(bm)) => am.mtime() < bm.mtime(),
        (Err(_), Ok(_)) => true,
        _ => false,
    }
}

fn same_file(a: &str, b: &str) -> bool {
    let am = std::fs::metadata(a);
    let bm = std::fs::metadata(b);
    match (am, bm) {
        (Ok(am), Ok(bm)) => am.dev() == bm.dev() && am.ino() == bm.ino(),
        _ => false,
    }
}

fn libc_access(path: &str, mode: i32) -> bool {
    let c = std::ffi::CString::new(path).unwrap_or_default();
    unsafe { libc::access(c.as_ptr(), mode) == 0 }
}

fn bool_to_status(v: bool) -> i32 {
    if v {
        0
    } else {
        1
    }
}

fn invert(status: i32) -> i32 {
    if status == 0 {
        1
    } else if status == 1 {
        0
    } else {
        status
    }
}

fn report_test_error(env: Option<&dyn Environment>, subject: &str, message: &str) {
    if let Some(env) = env {
        report_diagnostic(env, subject, message);
    } else {
        eprintln!("cherubsh: {subject}: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::run_test;

    #[test]
    fn bang_can_be_binary_operand_in_three_arg_test() {
        let args = vec!["!".to_string(), "!=".to_string(), "!".to_string()];
        assert_eq!(run_test(&args, &[0; 3], false, None), 1);
    }
}
