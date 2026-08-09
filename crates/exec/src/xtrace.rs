use std::io::Write;

use cherubsh_expander::expand_string_to_string;

use crate::ExecContext;

pub(crate) fn trace(ctx: &mut ExecContext<'_>, line: &str) {
    if !ctx.env.option("xtrace") {
        return;
    }
    let fd_value = ctx.env.get("BASH_XTRACEFD");
    trace_with_fd_value(ctx, line, fd_value.as_deref());
}

pub(crate) fn trace_with_fd_value(ctx: &mut ExecContext<'_>, line: &str, fd_value: Option<&str>) {
    let ps4_value = ctx.env.get("PS4");
    trace_with_values(ctx, line, ps4_value.as_deref(), fd_value);
}

pub(crate) fn trace_with_values(
    ctx: &mut ExecContext<'_>,
    line: &str,
    ps4_value: Option<&str>,
    fd_value: Option<&str>,
) {
    let raw_ps4 = ps4_value.unwrap_or("+ ").to_string();
    let xtrace_enabled = ctx.env.option("xtrace");
    ctx.env.set_option("xtrace", false);
    let mut runner = crate::runner::ExecRunner::with_functions_mut_at_depth(
        &mut ctx.functions,
        &mut ctx.function_sources,
        ctx.function_depth,
        ctx.source_depth,
    );
    let expanded_ps4 = expand_string_to_string(&raw_ps4, ctx.env, &mut runner);
    ctx.env.set_option("xtrace", xtrace_enabled);
    let mut ps4 = expanded_ps4.unwrap_or(raw_ps4);
    let extra_prefixes = ctx.source_depth
        + ctx.env.command_substitution_depth()
        + u32::from(ctx.env.running_trap().is_some());
    if extra_prefixes > 0 {
        if let Some(first) = ps4.chars().next() {
            for _ in 0..extra_prefixes {
                ps4.insert(0, first);
            }
        }
    }
    let text = format!("{ps4}{line}\n");
    write_trace(ctx, text.as_bytes(), fd_value);
}

pub(crate) fn quote_word(word: &str) -> String {
    let bytes = cherubsh_expander::quote::shell_string_to_bytes(word);
    if bytes.is_empty() {
        return "''".to_string();
    }
    if bytes != word.as_bytes() || std::str::from_utf8(&bytes).is_err() {
        return cherubsh_builtins::common::ansi_c_quote(word);
    }
    if bytes.iter().any(|b| *b < 0x20 || *b == 0x7f) {
        return cherubsh_builtins::common::ansi_c_quote(word);
    }
    if bytes
        .iter()
        .all(|b| matches!(*b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'))
    {
        return word.to_string();
    }
    let mut out = String::from("'");
    for ch in word.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn write_trace(ctx: &mut ExecContext<'_>, bytes: &[u8], fd_value: Option<&str>) {
    if let Some(value) = fd_value.filter(|value| !value.is_empty()) {
        if let Ok(fd) = value.parse::<i32>() {
            if fd >= 0 && fd_is_open(fd) {
                unsafe {
                    let _ = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
                }
                return;
            }
        }
        report_invalid_xtracefd(ctx, value);
        ctx.env.unset("BASH_XTRACEFD");
    }

    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(bytes);
    let _ = stderr.flush();
}

fn fd_is_open(fd: i32) -> bool {
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

fn report_invalid_xtracefd(ctx: &ExecContext<'_>, value: &str) {
    if let (Some(source), Some(line)) =
        (ctx.env.diagnostic_source_name(), ctx.env.diagnostic_line())
    {
        eprintln!(
            "{source}: line {line}: BASH_XTRACEFD: {value}: invalid value for trace file descriptor"
        );
    } else {
        eprintln!("BASH_XTRACEFD: {value}: invalid value for trace file descriptor");
    }
}
