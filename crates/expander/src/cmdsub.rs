//! Command substitution. Runs `src` in a subshell via `CommandRunner`,
//! captures stdout, strips trailing newlines per POSIX.

use std::fs;

use crate::buf::ExpandBuf;
use crate::ctx::ExpCtx;
use crate::error::ExpandError;
use crate::ExpandFlags;
use cherubsh_common::{Span, CMD_SUBST_HEREDOC_WARN_MARKER, W_HASDOLLAR};
use cherubsh_parser::WordDesc;

/// Run `src` as a `$(...)` or `` `...` `` substitution. `quoted` is set when
/// the substitution appears inside double quotes - in that case the result is
/// CTLESC-protected so word splitting and globbing skip it.
pub fn command_substitute(
    src: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
    backquote: bool,
    close_line: Option<u32>,
) -> Result<ExpandBuf, ExpandError> {
    let (src, warn_heredoc) = if src.contains(CMD_SUBST_HEREDOC_WARN_MARKER) {
        (src.replace(CMD_SUBST_HEREDOC_WARN_MARKER, ""), true)
    } else {
        (src.to_string(), false)
    };
    if warn_heredoc {
        if let (Some(source), Some(line)) =
            (ctx.env.diagnostic_source_name(), ctx.env.diagnostic_line())
        {
            eprintln!(
                "{source}: line {line}: warning: command substitution: 1 unterminated here-document"
            );
        } else {
            eprintln!("warning: command substitution: 1 unterminated here-document");
        }
    }
    let pushed_line = if !backquote {
        if let Some(line) = close_line {
            ctx.env.push_diagnostic_line(line);
            true
        } else {
            false
        }
    } else {
        false
    };
    if !backquote {
        match try_input_file_substitution(&src, ctx, quoted) {
            Ok(Some(buf)) => {
                if pushed_line {
                    ctx.env.pop_diagnostic_line();
                }
                return Ok(buf);
            }
            Ok(None) => {}
            Err(err) => {
                if pushed_line {
                    ctx.env.pop_diagnostic_line();
                }
                return Err(err);
            }
        }
    }
    let result = if backquote {
        ctx.runner.run_backquote_subst(ctx.env, &src)
    } else {
        ctx.runner.run_subst(ctx.env, &src)
    };
    if pushed_line {
        ctx.env.pop_diagnostic_line();
    }
    let mut bytes = result?;
    // Strip trailing newlines.
    while bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    ctx.last_cmd_subst_status = ctx.env.last_status();
    Ok(bytes_to_subst_buf(bytes, quoted))
}

fn bytes_to_subst_buf(bytes: Vec<u8>, quoted: bool) -> ExpandBuf {
    let mut buf = ExpandBuf::with_capacity(bytes.len());
    if quoted && bytes.is_empty() {
        buf.push_quoted_param_null();
    } else if quoted {
        for b in bytes {
            buf.push_quoted(b);
        }
    } else {
        for b in bytes {
            buf.push_literal(b);
        }
    }
    buf
}

fn try_input_file_substitution(
    src: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
) -> Result<Option<ExpandBuf>, ExpandError> {
    let Some(word) = input_file_substitution_word(src) else {
        return Ok(None);
    };
    let flags = if word.as_bytes().iter().any(|b| matches!(b, b'$' | b'`')) {
        W_HASDOLLAR
    } else {
        0
    };
    let mut expand_flags = ExpandFlags::QUOTE_REMOVAL;
    if !ctx.env.option("posix") {
        expand_flags |= ExpandFlags::EXPAND_GLOB;
    }
    let expanded = crate::expand_word_list(
        &[WordDesc {
            text: word.to_string(),
            flags,
            span: Span::dummy(),
        }],
        ctx.env,
        ctx.runner,
        expand_flags,
    )?;
    let Some(path_word) = expanded.first() else {
        ctx.env.set_last_status(1);
        ctx.last_cmd_subst_status = 1;
        return Ok(Some(bytes_to_subst_buf(Vec::new(), quoted)));
    };
    let path = &path_word.text;
    match fs::read(path) {
        Ok(mut bytes) => {
            while bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            ctx.env.set_last_status(0);
            ctx.last_cmd_subst_status = 0;
            Ok(Some(bytes_to_subst_buf(bytes, quoted)))
        }
        Err(err) => {
            report_input_file_subst_error(ctx, path, &err);
            ctx.env.set_last_status(1);
            ctx.last_cmd_subst_status = 1;
            Ok(Some(bytes_to_subst_buf(Vec::new(), quoted)))
        }
    }
}

fn input_file_substitution_word(src: &str) -> Option<&str> {
    let trimmed = src.trim();
    let rest = trimmed.strip_prefix('<')?;
    if rest.starts_with(['<', '>', '&', '(']) {
        return None;
    }
    let word = rest.trim();
    if word.is_empty() {
        None
    } else {
        Some(word)
    }
}

fn report_input_file_subst_error(ctx: &mut ExpCtx, path: &str, err: &std::io::Error) {
    let message = errno_message(err);
    if let (Some(source), Some(line)) =
        (ctx.env.diagnostic_source_name(), ctx.env.diagnostic_line())
    {
        eprintln!("{source}: line {line}: {path}: {message}");
    } else {
        eprintln!("cherubsh: {path}: {message}");
    }
}

fn errno_message(err: &std::io::Error) -> String {
    let Some(errno) = err.raw_os_error() else {
        return err.to_string();
    };
    let ptr = unsafe { libc::strerror(errno) };
    if ptr.is_null() {
        err.to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Pre-process a backtick substitution body: POSIX requires unescaping `\$`,
/// `` \` ``, `\\`, and backslash-newline once before running.
pub fn unbackslash_backticks(src: &str, in_double_quotes: bool) -> String {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n == b'\n' {
                i += 2;
                continue;
            }
            if matches!(n, b'$' | b'`' | b'\\') || (in_double_quotes && n == b'"') {
                out.push(n);
                i += 2;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::unbackslash_backticks;
    #[test]
    fn backtick_unescape() {
        assert_eq!(unbackslash_backticks("echo \\$x", false), "echo $x");
        assert_eq!(unbackslash_backticks("echo \\\\$x", false), "echo \\$x");
        assert_eq!(
            unbackslash_backticks("echo foo\\\nbar", false),
            "echo foobar"
        );
        assert_eq!(
            unbackslash_backticks("echo foo\\\\\nbar", false),
            "echo foo\\\nbar"
        );
        assert_eq!(
            unbackslash_backticks("echo \\\"x\\\"", false),
            "echo \\\"x\\\""
        );
        assert_eq!(unbackslash_backticks("echo \\\"x\\\"", true), "echo \"x\"");
        assert_eq!(unbackslash_backticks("foo", false), "foo");
    }
}
