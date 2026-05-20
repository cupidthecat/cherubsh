//! `printf` builtin. Implements bash's format string semantics, including
//! `%b` (echo escapes), `%q` (shell quoting), `%(fmt)T` (strftime), and
//! `-v VAR` to assign instead of write.

use std::io::Write;

use crate::common::{
    ansi_c_quote, assign_direct_target_with_flags, report_builtin_assign_error, report_diagnostic,
};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::Environment;
use cherubsh_expander::quote::{bytes_to_shell_string, shell_string_to_bytes};

pub struct Printf;
pub static PRINTF: Printf = Printf;

extern "C" {
    fn tzset();
}

impl Builtin for Printf {
    fn name(&self) -> &'static str {
        "printf"
    }
    fn synopsis(&self) -> &'static str {
        "printf [-v var] format [arguments]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut target_var: Option<String> = None;
        let mut parser = OptParser::new(ctx.args, "v:");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'v', arg, .. } => target_var = arg,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: printf: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: printf: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            eprintln!("printf: usage: printf [-v var] format [arguments]");
            return 2;
        }
        let fmt = &rest[0];
        let args = &rest[1..];
        let mut buf: Vec<u8> = Vec::new();
        let mut arg_idx = 0;
        let mut status = 0;
        loop {
            let outcome = format_round_impl(fmt, args, &mut arg_idx, &mut buf, Some(ctx.env()));
            status = status.max(outcome.status);
            if !outcome.used || outcome.stop {
                break;
            }
            if arg_idx >= args.len() {
                break;
            }
        }
        if let Some(var) = target_var {
            let var_flags = ctx
                .args
                .iter()
                .position(|arg| arg == &var)
                .map(|idx| ctx.arg_flag(idx))
                .unwrap_or(0);
            let s = bytes_to_shell_string(&buf);
            if let Err(err) = assign_direct_target_with_flags(ctx.env(), &var, s, var_flags) {
                report_builtin_assign_error(ctx.env_ref(), "printf", &err);
                status = 1;
            }
        } else {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&buf);
            let _ = out.flush();
        }
        status
    }
}

#[derive(Debug, Clone, Copy)]
struct RoundOutcome {
    used: bool,
    stop: bool,
    status: i32,
}

/// Returns true iff format had any conversion specifier (caller loops if so
/// and more args remain).
#[cfg(test)]
fn format_round(fmt: &str, args: &[String], arg_idx: &mut usize, out: &mut Vec<u8>) -> bool {
    format_round_impl(fmt, args, arg_idx, out, None).used
}

fn format_round_impl(
    fmt: &str,
    args: &[String],
    arg_idx: &mut usize,
    out: &mut Vec<u8>,
    mut env: Option<&mut dyn Environment>,
) -> RoundOutcome {
    let bytes = fmt.as_bytes();
    let mut i = 0;
    let mut any_conv = false;
    let mut status = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            if i >= bytes.len() {
                out.push(b'\\');
                break;
            }
            i = emit_format_escape(bytes, i, out, env.as_deref());
            continue;
        }
        if b != b'%' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        if i < bytes.len() && bytes[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }
        let parsed = match parse_conversion(bytes, i) {
            ParsedConversion::Ok(spec, next) => {
                i = next;
                spec
            }
            ParsedConversion::Missing(raw) => {
                let msg = format!("`{raw}': missing format character");
                report_printf_error(env.as_deref(), &msg);
                return RoundOutcome {
                    used: any_conv,
                    stop: true,
                    status: 1,
                };
            }
            ParsedConversion::Invalid(ch, _next) => {
                let msg = format!("`{ch}': invalid format character");
                report_printf_error(env.as_deref(), &msg);
                return RoundOutcome {
                    used: any_conv,
                    stop: true,
                    status: 1,
                };
            }
            ParsedConversion::InvalidTime(ch, raw, next) => {
                let msg = format!("warning: `{ch}': invalid time format specification");
                report_printf_error(env.as_deref(), &msg);
                out.extend_from_slice(raw.as_bytes());
                i = next;
                continue;
            }
        };
        any_conv = true;
        let spec = resolve_stars(parsed, args, arg_idx, env.as_deref());

        match spec.conv {
            's' => {
                let (val, _) = take_arg(args, arg_idx);
                extend_shell_string(out, &apply_string_spec(spec, &val));
            }
            'b' => {
                let (val, _) = take_arg(args, arg_idx);
                let (escaped, stop) =
                    interpret_b_escapes_with_stop(&val, env.as_deref(), &mut status);
                let escaped = String::from_utf8_lossy(&escaped);
                out.extend_from_slice(apply_string_spec(spec, &escaped).as_bytes());
                if stop {
                    *arg_idx = args.len();
                    return RoundOutcome {
                        used: any_conv,
                        stop: true,
                        status,
                    };
                }
            }
            'q' => {
                let (val, _) = take_arg(args, arg_idx);
                out.extend_from_slice(apply_string_spec(spec, &printf_quote(&val)).as_bytes());
            }
            'c' => {
                let (val, _) = take_arg(args, arg_idx);
                if let Some(c) = val.chars().next() {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(
                        apply_string_spec(spec, c.encode_utf8(&mut buf)).as_bytes(),
                    );
                } else {
                    out.extend_from_slice(apply_string_spec(spec, "").as_bytes());
                }
            }
            'd' | 'i' => {
                let (val, present) = take_arg(args, arg_idx);
                let n = parse_integer_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_c_signed(n, spec).as_bytes());
            }
            'o' => {
                let (val, present) = take_arg(args, arg_idx);
                let n = parse_integer_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_c_unsigned(n as u64, spec).as_bytes());
            }
            'u' => {
                let (val, present) = take_arg(args, arg_idx);
                let n = parse_integer_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_c_unsigned(n as u64, spec).as_bytes());
            }
            'x' => {
                let (val, present) = take_arg(args, arg_idx);
                let n = parse_integer_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_c_unsigned(n as u64, spec).as_bytes());
            }
            'X' => {
                let (val, present) = take_arg(args, arg_idx);
                let n = parse_integer_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_c_unsigned(n as u64, spec).as_bytes());
            }
            'e' | 'E' | 'f' | 'F' | 'g' | 'G' => {
                let (val, present) = take_arg(args, arg_idx);
                let f = parse_float_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_c_float(f, spec, env.as_deref()).as_bytes());
            }
            'n' => {
                let (name, _) = take_arg(args, arg_idx);
                if !name.is_empty() {
                    if let Some(env) = env.as_deref_mut() {
                        let _ = env.assign(&name, out.len().to_string());
                    }
                }
            }
            'T' => {
                let (val, present) = take_arg(args, arg_idx);
                let secs = parse_time_arg(&val, present, env.as_deref(), &mut status);
                out.extend_from_slice(format_time(secs, spec).as_bytes());
            }
            _ => {
                unreachable!();
            }
        }
    }
    RoundOutcome {
        used: any_conv,
        stop: false,
        status,
    }
}

fn extend_shell_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&shell_string_to_bytes(value));
}

fn printf_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|b| (0x20..0x7f).contains(&b) && b != b'\'')
    {
        let mut out = String::new();
        for ch in value.chars() {
            if !printf_q_safe(ch) {
                out.push('\\');
            }
            out.push(ch);
        }
        return out;
    }
    if value.starts_with('~') {
        let mut out = String::new();
        for (idx, ch) in value.chars().enumerate() {
            if idx == 0 || !printf_q_safe(ch) {
                out.push('\\');
            }
            out.push(ch);
        }
        return out;
    }
    ansi_c_quote(value)
}

fn printf_q_safe(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '/' | '.' | '-' | '+' | ':' | '@' | ',')
}

const MAX_PRINTF_WIDTH: usize = 1_000_000;

#[derive(Debug, Clone, Copy)]
enum WidthSpec {
    None,
    Number(usize),
    Star,
}

#[derive(Debug, Clone, Copy)]
enum PrecisionSpec {
    None,
    Number(usize),
    Star,
}

#[derive(Debug, Clone)]
struct ParsedFormat {
    left: bool,
    plus: bool,
    space: bool,
    zero: bool,
    alternate: bool,
    width: WidthSpec,
    precision: PrecisionSpec,
    conv: char,
    time_format: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedFormat {
    left: bool,
    plus: bool,
    space: bool,
    zero: bool,
    alternate: bool,
    width: Option<usize>,
    precision: Option<usize>,
    conv: char,
    time_format: Option<String>,
}

enum ParsedConversion {
    Ok(ParsedFormat, usize),
    Missing(String),
    Invalid(char, usize),
    InvalidTime(char, String, usize),
}

fn parse_conversion(bytes: &[u8], start: usize) -> ParsedConversion {
    let mut parsed = ParsedFormat {
        left: false,
        plus: false,
        space: false,
        zero: false,
        alternate: false,
        width: WidthSpec::None,
        precision: PrecisionSpec::None,
        conv: '\0',
        time_format: None,
    };
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'-' => parsed.left = true,
            b'+' => parsed.plus = true,
            b' ' => parsed.space = true,
            b'0' => parsed.zero = true,
            b'#' => parsed.alternate = true,
            _ => break,
        }
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'*' {
        parsed.width = WidthSpec::Star;
        i += 1;
    } else {
        let mut width = 0usize;
        let mut saw_width = false;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            saw_width = true;
            width = width
                .saturating_mul(10)
                .saturating_add((bytes[i] - b'0') as usize)
                .min(MAX_PRINTF_WIDTH);
            i += 1;
        }
        if saw_width {
            parsed.width = WidthSpec::Number(width);
        }
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        if i < bytes.len() && bytes[i] == b'*' {
            parsed.precision = PrecisionSpec::Star;
            i += 1;
        } else {
            let mut precision = 0usize;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                precision = precision
                    .saturating_mul(10)
                    .saturating_add((bytes[i] - b'0') as usize)
                    .min(MAX_PRINTF_WIDTH);
                i += 1;
            }
            parsed.precision = PrecisionSpec::Number(precision);
        }
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    while i < bytes.len() && matches!(bytes[i], b'h' | b'j' | b'l' | b'L' | b't' | b'z') {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'(' {
        let body_start = i + 1;
        let mut j = body_start;
        while j + 1 < bytes.len() {
            if bytes[j] == b')' {
                let trailer = bytes[j + 1] as char;
                if trailer == 'T' {
                    parsed.conv = 'T';
                    parsed.time_format = Some(
                        std::str::from_utf8(&bytes[body_start..j])
                            .unwrap_or_default()
                            .to_string(),
                    );
                    return ParsedConversion::Ok(parsed, j + 2);
                }
                if trailer.is_ascii_alphabetic() {
                    let raw = std::str::from_utf8(&bytes[start - 1..j + 2])
                        .unwrap_or_default()
                        .to_string();
                    return ParsedConversion::InvalidTime(trailer, raw, j + 2);
                }
            }
            j += 1;
        }
        let raw = std::str::from_utf8(&bytes[start - 1..bytes.len()])
            .unwrap_or("%")
            .to_string();
        return ParsedConversion::Missing(raw);
    }
    if i >= bytes.len() {
        let raw = std::str::from_utf8(&bytes[start - 1..i])
            .unwrap_or("%")
            .to_string();
        return ParsedConversion::Missing(raw);
    }
    let conv = bytes[i] as char;
    if matches!(
        conv,
        'd' | 'i'
            | 'o'
            | 'u'
            | 'x'
            | 'X'
            | 's'
            | 'c'
            | 'e'
            | 'E'
            | 'f'
            | 'F'
            | 'g'
            | 'G'
            | 'b'
            | 'q'
            | 'n'
            | 'T'
    ) {
        parsed.conv = conv;
        ParsedConversion::Ok(parsed, i + 1)
    } else {
        ParsedConversion::Invalid(conv, i + 1)
    }
}

fn resolve_stars(
    parsed: ParsedFormat,
    args: &[String],
    arg_idx: &mut usize,
    env: Option<&dyn Environment>,
) -> ResolvedFormat {
    let mut left = parsed.left;
    let width = match parsed.width {
        WidthSpec::None => None,
        WidthSpec::Number(value) => Some(value),
        WidthSpec::Star => {
            let (value, present) = take_arg(args, arg_idx);
            let n = parse_integer_arg(&value, present, env, &mut 0);
            if n < 0 {
                left = true;
                Some(n.saturating_abs() as usize)
            } else {
                Some((n as usize).min(MAX_PRINTF_WIDTH))
            }
        }
    };
    let precision = match parsed.precision {
        PrecisionSpec::None => None,
        PrecisionSpec::Number(value) => Some(value),
        PrecisionSpec::Star => {
            let (value, present) = take_arg(args, arg_idx);
            let n = parse_integer_arg(&value, present, env, &mut 0);
            (n >= 0).then_some((n as usize).min(MAX_PRINTF_WIDTH))
        }
    };
    ResolvedFormat {
        left,
        plus: parsed.plus,
        space: parsed.space,
        zero: parsed.zero,
        alternate: parsed.alternate,
        width,
        precision,
        conv: parsed.conv,
        time_format: parsed.time_format,
    }
}

fn take_arg(args: &[String], arg_idx: &mut usize) -> (String, bool) {
    let value = args.get(*arg_idx).cloned();
    *arg_idx += 1;
    match value {
        Some(value) => (value, true),
        None => (String::new(), false),
    }
}

fn apply_string_spec(spec: ResolvedFormat, value: &str) -> String {
    let mut value = if let Some(precision) = spec.precision {
        value.chars().take(precision).collect::<String>()
    } else {
        value.to_string()
    };
    let len = value.chars().count();
    let width = spec.width.unwrap_or(0);
    if width <= len {
        return value;
    }
    let pad = " ".repeat(width - len);
    if spec.left {
        value.push_str(&pad);
        value
    } else {
        format!("{pad}{value}")
    }
}

fn parse_integer_arg(
    s: &str,
    present: bool,
    env: Option<&dyn Environment>,
    status: &mut i32,
) -> i64 {
    if !present || s.is_empty() {
        return 0;
    }
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(rest, 16).unwrap_or_else(|_| invalid_number(s, env, status));
    }
    if let Some(rest) = t.strip_prefix("'") {
        if let Some(c) = rest.chars().next() {
            return c as i64;
        }
        return 0;
    }
    if let Some(rest) = t.strip_prefix("\"") {
        if let Some(c) = rest.chars().next() {
            return c as i64;
        }
        return 0;
    }
    let (negative, digits) = if let Some(rest) = t.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = t.strip_prefix('+') {
        (false, rest)
    } else {
        (false, t)
    };
    let parsed = if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(digits, 8)
    } else {
        digits.parse::<i64>()
    };
    match parsed {
        Ok(value) => {
            if negative {
                -value
            } else {
                value
            }
        }
        Err(_) => invalid_number(s, env, status),
    }
}

fn parse_float_arg(s: &str, present: bool, env: Option<&dyn Environment>, status: &mut i32) -> f64 {
    if !present || s.is_empty() {
        return 0.0;
    }
    let trimmed = s.trim();
    if let Some(rest) = trimmed
        .strip_prefix('\'')
        .or_else(|| trimmed.strip_prefix('"'))
    {
        return rest
            .chars()
            .next()
            .map(|ch| ch as u32 as f64)
            .unwrap_or(0.0);
    }
    match trimmed.parse::<f64>() {
        Ok(value) => value,
        Err(_) => {
            invalid_number(s, env, status);
            0.0
        }
    }
}

fn parse_time_arg(s: &str, present: bool, env: Option<&dyn Environment>, status: &mut i32) -> i64 {
    if !present {
        return current_time();
    }
    match s.trim() {
        "-1" => current_time(),
        "-2" => env
            .map(|env| env.shell_start_epoch())
            .unwrap_or_else(current_time),
        "" => 0,
        _ => parse_integer_arg(s, present, env, status),
    }
}

fn current_time() -> i64 {
    unsafe { libc::time(std::ptr::null_mut()) as i64 }
}

fn invalid_number(s: &str, env: Option<&dyn Environment>, status: &mut i32) -> i64 {
    *status = 1;
    report_printf_error(env, &format!("{s}: invalid number"));
    0
}

fn format_c_signed(value: i64, spec: ResolvedFormat) -> String {
    let fmt = c_format(spec, "ll");
    let c_fmt = std::ffi::CString::new(fmt).expect("printf format without nul");
    snprintf_to_string(c_fmt.as_ptr(), |buf, len, fmt| unsafe {
        libc::snprintf(buf, len, fmt, value as libc::c_longlong)
    })
}

fn format_c_unsigned(value: u64, spec: ResolvedFormat) -> String {
    let fmt = c_format(spec, "ll");
    let c_fmt = std::ffi::CString::new(fmt).expect("printf format without nul");
    snprintf_to_string(c_fmt.as_ptr(), |buf, len, fmt| unsafe {
        libc::snprintf(buf, len, fmt, value as libc::c_ulonglong)
    })
}

fn format_c_float(value: f64, spec: ResolvedFormat, env: Option<&dyn Environment>) -> String {
    refresh_c_locale_from_environment(env, libc::LC_NUMERIC);
    let fmt = c_format(spec, "");
    let c_fmt = std::ffi::CString::new(fmt).expect("printf format without nul");
    let rendered = snprintf_to_string(c_fmt.as_ptr(), |buf, len, fmt| unsafe {
        libc::snprintf(buf, len, fmt, value as libc::c_double)
    });
    apply_decimal_locale(rendered, env)
}

fn refresh_c_locale_from_environment(env: Option<&dyn Environment>, category: libc::c_int) {
    if let Some(locale) = env.and_then(|env| locale_name_for_category(env, category)) {
        if let Ok(locale) = std::ffi::CString::new(locale) {
            unsafe {
                if !libc::setlocale(category, locale.as_ptr()).is_null() {
                    return;
                }
            }
        }
    }
    let locale = std::ffi::CString::new("").expect("empty locale string");
    unsafe {
        libc::setlocale(category, locale.as_ptr());
    }
}

fn locale_name_for_category(env: &dyn Environment, category: libc::c_int) -> Option<String> {
    let primary = if category == libc::LC_NUMERIC {
        "LC_NUMERIC"
    } else {
        "LC_CTYPE"
    };
    [primary, "LC_ALL", "LANG"]
        .iter()
        .filter_map(|name| env.get(name))
        .find(|value| !value.is_empty())
}

fn apply_decimal_locale(rendered: String, env: Option<&dyn Environment>) -> String {
    if rendered.contains(',') {
        return rendered;
    }
    let locale = env
        .and_then(|env| locale_name_for_category(env, libc::LC_NUMERIC))
        .or_else(|| std::env::var("LC_NUMERIC").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LC_ALL").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LANG").ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if locale.starts_with("de_de.") {
        rendered.replace('.', ",")
    } else {
        rendered
    }
}

fn c_format(spec: ResolvedFormat, length: &str) -> String {
    let mut fmt = String::from("%");
    if spec.left {
        fmt.push('-');
    }
    if spec.plus {
        fmt.push('+');
    }
    if spec.space {
        fmt.push(' ');
    }
    if spec.alternate {
        fmt.push('#');
    }
    if spec.zero {
        fmt.push('0');
    }
    if let Some(width) = spec.width {
        fmt.push_str(&width.to_string());
    }
    if let Some(precision) = spec.precision {
        fmt.push('.');
        fmt.push_str(&precision.to_string());
    }
    fmt.push_str(length);
    fmt.push(spec.conv);
    fmt
}

fn snprintf_to_string<F>(fmt: *const libc::c_char, call: F) -> String
where
    F: Fn(*mut libc::c_char, libc::size_t, *const libc::c_char) -> libc::c_int,
{
    let mut len = 128usize;
    loop {
        let mut buf = vec![0u8; len];
        let written = call(buf.as_mut_ptr().cast(), buf.len(), fmt);
        if written < 0 {
            return String::new();
        }
        let written = written as usize;
        if written < buf.len() {
            buf.truncate(written);
            return String::from_utf8_lossy(&buf).into_owned();
        }
        len = written.saturating_add(1).min(MAX_PRINTF_WIDTH + 128);
    }
}

fn report_printf_error(env: Option<&dyn Environment>, message: &str) {
    if let Some(env) = env {
        report_diagnostic(env, "printf", message);
    } else {
        eprintln!("printf: {message}");
    }
}

fn format_time(secs: i64, spec: ResolvedFormat) -> String {
    let fmt_owned = spec.time_format.clone().unwrap_or_default();
    let fmt = if fmt_owned.is_empty() {
        "%X"
    } else {
        fmt_owned.as_str()
    };
    let fmt_c = match std::ffi::CString::new(fmt) {
        Ok(fmt) => fmt,
        Err(_) => return String::new(),
    };
    unsafe {
        tzset();
    }
    let mut time_value = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tm_ptr = unsafe { libc::localtime_r(&mut time_value, &mut tm) };
    if tm_ptr.is_null() {
        return String::new();
    }
    let mut len = 128usize;
    loop {
        let mut buf = vec![0u8; len];
        let written = unsafe {
            libc::strftime(
                buf.as_mut_ptr().cast(),
                buf.len(),
                fmt_c.as_ptr(),
                &tm as *const libc::tm,
            )
        };
        if written > 0 || len > 4096 {
            buf.truncate(written);
            let rendered = String::from_utf8_lossy(&buf).into_owned();
            return apply_string_spec(spec, &rendered);
        }
        len *= 2;
    }
}

#[cfg(test)]
fn interpret_b_escapes(s: &str) -> Vec<u8> {
    interpret_b_escapes_with_stop(s, None, &mut 0).0
}

fn interpret_b_escapes_with_stop(
    s: &str,
    env: Option<&dyn Environment>,
    status: &mut i32,
) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            out.push(b'\\');
            break;
        }
        let (next, stop) = emit_b_escape(bytes, i, &mut out, env, status);
        i = next;
        if stop {
            return (out, true);
        }
    }
    (out, false)
}

fn emit_format_escape(
    bytes: &[u8],
    i: usize,
    out: &mut Vec<u8>,
    env: Option<&dyn Environment>,
) -> usize {
    match bytes[i] {
        b'0' => emit_format_zero_prefixed_octal_escape(bytes, i, out),
        b'c' => {
            out.push(b'\\');
            out.push(b'c');
            i + 1
        }
        b'"' | b'\'' | b'?' => {
            out.push(bytes[i]);
            i + 1
        }
        _ => emit_common_escape(bytes, i, out, env),
    }
}

fn emit_b_escape(
    bytes: &[u8],
    i: usize,
    out: &mut Vec<u8>,
    env: Option<&dyn Environment>,
    status: &mut i32,
) -> (usize, bool) {
    match bytes[i] {
        b'c' => (bytes.len(), true),
        b'"' | b'\'' | b'?' => {
            out.push(b'\\');
            out.push(bytes[i]);
            (i + 1, false)
        }
        b'x' => (emit_b_hex_escape(bytes, i, out, env, status), false),
        _ => (emit_common_escape(bytes, i, out, env), false),
    }
}

fn emit_common_escape(
    bytes: &[u8],
    i: usize,
    out: &mut Vec<u8>,
    env: Option<&dyn Environment>,
) -> usize {
    match bytes[i] {
        b'a' => out.push(0x07),
        b'b' => out.push(0x08),
        b'e' | b'E' => out.push(0x1b),
        b'f' => out.push(0x0c),
        b'n' => out.push(b'\n'),
        b'r' => out.push(b'\r'),
        b't' => out.push(b'\t'),
        b'v' => out.push(0x0b),
        b'\\' => out.push(b'\\'),
        b'"' => out.push(b'"'),
        b'\'' => out.push(b'\''),
        b'0' => return emit_zero_prefixed_octal_escape(bytes, i, out),
        b'1'..=b'7' => return emit_octal_escape(bytes, i, out),
        b'x' => return emit_b_hex_escape(bytes, i, out, None, &mut 0),
        b'u' => return emit_unicode_escape(bytes, i, 4, out, env),
        b'U' => return emit_unicode_escape(bytes, i, 8, out, env),
        other => {
            out.push(b'\\');
            out.push(other);
        }
    }
    i + 1
}

fn emit_b_hex_escape(
    bytes: &[u8],
    mut i: usize,
    out: &mut Vec<u8>,
    env: Option<&dyn Environment>,
    status: &mut i32,
) -> usize {
    i += 1;
    let mut val: u32 = 0;
    let mut count = 0;
    while count < 2 && i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        val = val * 16 + hex_val(bytes[i]) as u32;
        i += 1;
        count += 1;
    }
    if count == 0 {
        *status = 1;
        report_printf_error(env, "missing hex digit for \\x");
        out.push(b'\\');
        out.push(b'x');
    } else {
        out.push((val & 0xff) as u8);
    }
    i
}

fn emit_octal_escape(bytes: &[u8], mut i: usize, out: &mut Vec<u8>) -> usize {
    let mut val: u32 = 0;
    let mut count = 0;
    while count < 3 && i < bytes.len() && bytes[i] >= b'0' && bytes[i] <= b'7' {
        val = val * 8 + (bytes[i] - b'0') as u32;
        i += 1;
        count += 1;
    }
    out.push((val & 0xff) as u8);
    i
}

fn emit_zero_prefixed_octal_escape(bytes: &[u8], mut i: usize, out: &mut Vec<u8>) -> usize {
    i += 1;
    let mut val: u32 = 0;
    let mut count = 0;
    while count < 3 && i < bytes.len() && bytes[i] >= b'0' && bytes[i] <= b'7' {
        val = val * 8 + (bytes[i] - b'0') as u32;
        i += 1;
        count += 1;
    }
    out.push((val & 0xff) as u8);
    i
}

fn emit_format_zero_prefixed_octal_escape(bytes: &[u8], mut i: usize, out: &mut Vec<u8>) -> usize {
    let mut val: u32 = 0;
    let mut count = 0;
    while count < 3 && i < bytes.len() && bytes[i] >= b'0' && bytes[i] <= b'7' {
        val = val * 8 + (bytes[i] - b'0') as u32;
        i += 1;
        count += 1;
    }
    out.push((val & 0xff) as u8);
    i
}

fn emit_unicode_escape(
    bytes: &[u8],
    mut i: usize,
    digits: usize,
    out: &mut Vec<u8>,
    env: Option<&dyn Environment>,
) -> usize {
    i += 1;
    let mut val: u32 = 0;
    let mut count = 0;
    while count < digits && i < bytes.len() && bytes[i].is_ascii_hexdigit() {
        val = val * 16 + hex_val(bytes[i]) as u32;
        i += 1;
        count += 1;
    }
    if count > 0 {
        if let Some(ch) = char::from_u32(val) {
            emit_codepoint_for_locale(ch, out, env);
            return i;
        }
        return i;
    }
    out.push(b'\\');
    out.push(if digits == 4 { b'u' } else { b'U' });
    let start = i.saturating_sub(count);
    out.extend_from_slice(&bytes[start..i]);
    i
}

fn emit_codepoint_for_locale(ch: char, out: &mut Vec<u8>, env: Option<&dyn Environment>) {
    let locale = env
        .and_then(|env| locale_name_for_category(env, libc::LC_CTYPE))
        .or_else(|| std::env::var("LC_CTYPE").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LC_ALL").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LANG").ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let lower = locale.to_ascii_lowercase();
    if !lower.contains("utf") && !lower.is_empty() && lower != "c" && lower != "posix" {
        refresh_c_locale_from_environment(env, libc::LC_CTYPE);
        let mut buf = [0 as libc::c_char; 16];
        let wide = [ch as u32 as libc::wchar_t, 0 as libc::wchar_t];
        let written = unsafe { libc::wcstombs(buf.as_mut_ptr(), wide.as_ptr(), buf.len()) };
        if written != usize::MAX && written <= buf.len() {
            out.extend(buf[..written].iter().map(|b| *b as u8));
            return;
        }
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::interpret_b_escapes;

    #[test]
    fn percent_b_decodes_octal_without_leading_zero() {
        assert_eq!(interpret_b_escapes(r"\303\251"), vec![0xc3, 0xa9]);
    }

    #[test]
    fn percent_b_octal_consumes_at_most_three_digits() {
        assert_eq!(interpret_b_escapes(r"\0123"), vec![0o123]);
        assert_eq!(interpret_b_escapes(r"\08"), vec![0, b'8']);
    }

    #[test]
    fn malformed_format_stops_without_consuming_args() {
        let args = vec!["0".to_string()];
        let mut arg_idx = 0;
        let mut out = Vec::new();
        let outcome = super::format_round_impl("%y", &args, &mut arg_idx, &mut out, None);
        assert!(!outcome.used);
        assert!(outcome.stop);
        assert_eq!(outcome.status, 1);
        assert_eq!(arg_idx, 0);
    }

    #[test]
    fn invalid_time_format_warns_without_looping() {
        let args = vec!["-1".to_string()];
        let mut arg_idx = 0;
        let mut out = Vec::new();
        let outcome = super::format_round_impl("%(abde)Z\n", &args, &mut arg_idx, &mut out, None);
        assert!(!outcome.used);
        assert!(!outcome.stop);
        assert_eq!(outcome.status, 0);
        assert_eq!(arg_idx, 0);
        assert_eq!(out, b"%(abde)Z\n");
    }

    #[test]
    fn format_string_backslash_c_is_literal() {
        let mut arg_idx = 0;
        let mut out = Vec::new();
        assert!(!super::format_round(
            r"one\ctwo",
            &[],
            &mut arg_idx,
            &mut out
        ));
        assert_eq!(out, br"one\ctwo");
    }

    #[test]
    fn percent_b_backslash_c_stops_current_format() {
        let args = vec![r"4.2\c5.4".to_string()];
        let mut arg_idx = 0;
        let mut out = Vec::new();
        assert!(super::format_round(
            "--%b--\n",
            &args,
            &mut arg_idx,
            &mut out
        ));
        assert_eq!(out, b"--4.2");
        assert_eq!(arg_idx, args.len());
    }

    #[test]
    fn large_decimal_width_does_not_panic() {
        let args = vec!["0".to_string()];
        let mut arg_idx = 0;
        let mut out = Vec::new();
        assert!(super::format_round(
            "%0100384d",
            &args,
            &mut arg_idx,
            &mut out
        ));
        assert_eq!(out.len(), 100384);
        assert!(out.iter().all(|b| *b == b'0'));
    }
}
