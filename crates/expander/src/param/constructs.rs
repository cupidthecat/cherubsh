fn dollar_bracket(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let body_start = *i + 2;
    let (body, end) = extract_legacy_arith(bytes, body_start)?;
    *i = end;
    let body_str = std::str::from_utf8(&body)
        .map_err(|_| ExpandError::ArithSyntax("non-utf8 arith".into()))?
        .to_string();
    let val = crate::eval_arith_expression_impl(&body_str, ctx)?;
    push_value(out, val.to_string().as_bytes(), quoted);
    Ok(())
}

fn dollar_paren(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    // bytes[*i]=='$', bytes[*i+1]=='('
    if *i + 2 < bytes.len() && bytes[*i + 2] == b'(' {
        // $((expr))
        match arith_dparen_scan(bytes, *i + 3) {
            DParenScan::Arithmetic(_) | DParenScan::Missing => {
                let body_start = *i + 3;
                let (body, end) = extract_double_paren(bytes, body_start)?;
                *i = end;
                let body_str = std::str::from_utf8(&body)
                    .map_err(|_| ExpandError::ArithSyntax("non-utf8 arith".into()))?
                    .to_string();
                let val = crate::eval_arith_expression_impl(&body_str, ctx)?;
                push_value(out, val.to_string().as_bytes(), quoted);
                return Ok(());
            }
            DParenScan::CommandSubstitution => {}
        }
    }
    // $(cmd)
    let body_start = *i + 2;
    let (body, end) = match extract_paren(bytes, body_start) {
        Ok(result) => result,
        Err(err) => {
            report_command_subst_heredoc_eof_warnings(bytes, body_start, bytes.len(), ctx);
            if matches!(err, ExpandError::BadSubstitution(ref message) if message == "missing ')'")
            {
                report_command_subst_unexpected_eof(bytes, body_start, ctx);
                return Err(ExpandError::AlreadyReported(2));
            }
            return Err(err);
        }
    };
    report_command_subst_heredoc_eof_warnings(bytes, body_start, end, ctx);
    *i = end;
    let src = String::from_utf8_lossy(&body).into_owned();
    let close_line = command_subst_close_line(bytes, body_start, end, ctx);
    let buf = cmdsub::command_substitute(&src, ctx, quoted, false, close_line)?;
    out.extend_from(&buf);
    Ok(())
}

fn command_subst_close_line(
    bytes: &[u8],
    body_start: usize,
    end: usize,
    ctx: &ExpCtx,
) -> Option<u32> {
    let base = ctx.env.diagnostic_line()?;
    let anchor = if compat_bash_52() { end } else { body_start };
    let before = bytes[..anchor.min(bytes.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32;
    Some(base.saturating_add(before))
}

fn compat_bash_52() -> bool {
    std::env::var("CHERUBSH_BASH_COMPAT_VERSION")
        .ok()
        .is_some_and(|version| version.starts_with("5.2"))
}

fn report_command_subst_unexpected_eof(bytes: &[u8], body_start: usize, ctx: &ExpCtx) {
    let mut line = command_subst_eof_line(&bytes[body_start.min(bytes.len())..]);
    let message = "unexpected EOF while looking for matching `)'";
    if ctx.heredoc_context {
        line = line.saturating_add(1);
        if let Some(source) = ctx.env.diagnostic_source_name() {
            eprintln!("{source}: command substitution: line {line}: {message}");
        } else {
            eprintln!("command substitution: line {line}: {message}");
        }
        return;
    }

    if ctx.param_rhs_nosplit {
        let line = ctx
            .env
            .diagnostic_line()
            .unwrap_or(1)
            .saturating_add(line)
            .saturating_add(1);
        if let Some(source) = ctx.env.diagnostic_source_name() {
            eprintln!("{source}: command substitution: line {line}: {message}");
        } else {
            eprintln!("command substitution: line {line}: {message}");
        }
        return;
    }

    if let Some(source) = ctx.env.diagnostic_source_name() {
        let base_line = ctx.env.diagnostic_line().unwrap_or(1).saturating_sub(1);
        eprintln!(
            "{}: line {}: {}",
            source,
            base_line.saturating_add(line),
            message
        );
    } else {
        eprintln!("{message}");
    }
}

fn command_subst_eof_line(body: &[u8]) -> u32 {
    1 + body.iter().filter(|byte| **byte == b'\n').count() as u32
}

#[derive(Clone)]
struct HeredocWarn {
    delimiter: Vec<u8>,
    strip_tabs: bool,
    start_line: u32,
}

fn report_command_subst_heredoc_eof_warnings(
    bytes: &[u8],
    body_start: usize,
    end: usize,
    ctx: &ExpCtx,
) {
    let Some(source) = ctx.env.diagnostic_source_name() else {
        return;
    };
    let base_line = ctx.env.diagnostic_line().unwrap_or(1).saturating_sub(1);
    let body = &bytes[body_start.min(bytes.len())..end.min(bytes.len())];
    for warning in command_subst_heredoc_eof_warnings(body) {
        eprintln!(
            "{}: line {}: warning: here-document at line {} delimited by end-of-file (wanted `{}')",
            source,
            base_line.saturating_add(warning.eof_line),
            base_line.saturating_add(warning.start_line),
            String::from_utf8_lossy(&warning.delimiter),
        );
    }
}

struct HeredocEofWarning {
    eof_line: u32,
    start_line: u32,
    delimiter: Vec<u8>,
}

fn command_subst_heredoc_eof_warnings(body: &[u8]) -> Vec<HeredocEofWarning> {
    let mut warnings = Vec::new();
    let mut pending: std::collections::VecDeque<HeredocWarn> = std::collections::VecDeque::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut at_line_start = true;
    let mut comment_ok = true;

    while i < body.len() {
        if at_line_start {
            if let Some(spec) = pending.front().cloned() {
                let line_start = i;
                while i < body.len() && body[i] != b'\n' {
                    i += 1;
                }
                let line_end = i;
                let mut candidate = &body[line_start..line_end];
                if spec.strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == spec.delimiter.as_slice() {
                    pending.pop_front();
                } else if command_subst_closes_heredoc(candidate, spec.delimiter.as_slice()) {
                    warnings.push(HeredocEofWarning {
                        eof_line: line,
                        start_line: spec.start_line,
                        delimiter: spec.delimiter,
                    });
                    pending.pop_front();
                    return warnings;
                }
                if i < body.len() && body[i] == b'\n' {
                    i += 1;
                    line = line.saturating_add(1);
                }
                at_line_start = true;
                comment_ok = true;
                continue;
            }
        }

        let b = body[i];
        if b == b'\n' {
            i += 1;
            line = line.saturating_add(1);
            at_line_start = true;
            comment_ok = true;
            continue;
        }
        at_line_start = false;

        if b == b'#' && comment_ok {
            while i < body.len() && body[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < body.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if append_ansi_c_literal(body, &mut i, &mut Vec::new()) {
            comment_ok = false;
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            skip_quoted_for_warning_scan(body, &mut i, b);
            comment_ok = false;
            continue;
        }
        if b == b'<' && i + 1 < body.len() && body[i + 1] == b'<' {
            collect_heredoc_warning_spec(body, &mut i, line, &mut pending);
            comment_ok = false;
            continue;
        }
        i += 1;
        comment_ok = b.is_ascii_whitespace() || is_shell_metacharacter(b);
    }

    let eof_line = line.saturating_sub(u32::from(body.last() == Some(&b'\n')));
    warnings.extend(pending.into_iter().map(|spec| HeredocEofWarning {
        eof_line,
        start_line: spec.start_line,
        delimiter: spec.delimiter,
    }));
    warnings
}

fn command_subst_closes_heredoc(candidate: &[u8], delimiter: &[u8]) -> bool {
    heredoc_delimiter_closing_paren(candidate, delimiter).is_some()
}

fn skip_quoted_for_warning_scan(bytes: &[u8], i: &mut usize, quote: u8) {
    *i += 1;
    while *i < bytes.len() {
        if bytes[*i] == b'\\' && *i + 1 < bytes.len() {
            *i += 2;
            continue;
        }
        if bytes[*i] == quote {
            *i += 1;
            return;
        }
        *i += 1;
    }
}

fn collect_heredoc_warning_spec(
    bytes: &[u8],
    i: &mut usize,
    line: u32,
    pending: &mut std::collections::VecDeque<HeredocWarn>,
) {
    *i += 2;
    let strip_tabs = if *i < bytes.len() && bytes[*i] == b'-' {
        *i += 1;
        true
    } else {
        false
    };
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t') {
        *i += 1;
    }
    let mut delimiter = Vec::new();
    let mut quote: Option<u8> = None;
    while *i < bytes.len() {
        let b = bytes[*i];
        if let Some(q) = quote {
            *i += 1;
            if b == q {
                quote = None;
            } else {
                delimiter.push(b);
            }
            continue;
        }
        if b == b'\'' || b == b'"' {
            quote = Some(b);
            *i += 1;
            continue;
        }
        if b == b'\\' {
            *i += 1;
            if *i < bytes.len() {
                if bytes[*i] == b'\n' {
                    *i += 1;
                    continue;
                }
                delimiter.push(bytes[*i]);
                *i += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() || is_shell_metacharacter(b) {
            break;
        }
        delimiter.push(b);
        *i += 1;
    }
    if strip_tabs {
        while delimiter.first() == Some(&b'\t') {
            delimiter.remove(0);
        }
    }
    if !delimiter.is_empty() {
        pending.push_back(HeredocWarn {
            delimiter,
            strip_tabs,
            start_line: line,
        });
    }
}

fn ansi_c_quote(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    out: &mut ExpandBuf,
) -> Result<bool, ExpandError> {
    // bytes[*i]='$', bytes[*i+1]='\''
    let start = *i + 2;
    let (body, end) = quote::scan_ansi_c_quoted(bytes, start);
    *i = end;
    let locale = ctx
        .env
        .get("LC_ALL")
        .filter(|v| !v.is_empty())
        .or_else(|| ctx.env.get("LC_CTYPE").filter(|v| !v.is_empty()))
        .or_else(|| ctx.env.get("LANG").filter(|v| !v.is_empty()));
    let decoded = quote::ansi_c_decode_for_locale(&body, locale.as_deref());
    // Output is quote-treated (CTLESC-protected) so word splitting/glob skip it.
    if decoded.is_empty() {
        out.push_quoted_null();
    } else {
        for b in decoded {
            out.push_quoted(b);
        }
    }
    Ok(true)
}

fn locale_quote(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    out: &mut ExpandBuf,
) -> Result<bool, ExpandError> {
    let content_start = *i + 2;
    let Some(content_end) = find_locale_quote_end(bytes, content_start) else {
        *i = content_start;
        crate::internal::scan_double_quoted_into(bytes, i, ctx, out)?;
        return Ok(true);
    };
    let source = String::from_utf8_lossy(&bytes[content_start..content_end]);
    let translated = gettext_translation(ctx.env, &source);
    *i = content_end + 1;

    if ctx.env.option("noexpand_translation") {
        if translated.is_empty() {
            out.push_quoted_null();
        } else {
            for byte in translated.bytes() {
                out.push_quoted(byte);
            }
        }
        return Ok(true);
    }

    let translated_quote = format!("{translated}\"");
    let mut translated_index = 0;
    crate::internal::scan_double_quoted_into(
        translated_quote.as_bytes(),
        &mut translated_index,
        ctx,
        out,
    )?;
    Ok(true)
}

fn find_locale_quote_end(bytes: &[u8], mut index: usize) -> Option<usize> {
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if index + 1 < bytes.len() => index += 2,
            b'"' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

#[cfg(unix)]
fn gettext_translation(env: &dyn Environment, source: &str) -> String {
    use std::ffi::{CStr, CString};

    unsafe extern "C" {
        fn bindtextdomain(
            domain: *const libc::c_char,
            directory: *const libc::c_char,
        ) -> *mut libc::c_char;
        fn dgettext(domain: *const libc::c_char, message: *const libc::c_char)
            -> *mut libc::c_char;
    }

    let Some(domain) = env.get("TEXTDOMAIN").filter(|value| !value.is_empty()) else {
        return source.to_string();
    };
    let Ok(domain) = CString::new(domain) else {
        return source.to_string();
    };
    let Ok(message) = CString::new(source) else {
        return source.to_string();
    };
    if let Some(directory) = env.get("TEXTDOMAINDIR").filter(|value| !value.is_empty()) {
        if let Ok(directory) = CString::new(directory) {
            unsafe {
                bindtextdomain(domain.as_ptr(), directory.as_ptr());
            }
        }
    }
    let translated = unsafe { dgettext(domain.as_ptr(), message.as_ptr()) };
    if translated.is_null() {
        source.to_string()
    } else {
        unsafe { CStr::from_ptr(translated) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(not(unix))]
fn gettext_translation(_env: &dyn Environment, source: &str) -> String {
    source.to_string()
}

pub(crate) fn dollar_quote_expand(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    out: &mut ExpandBuf,
) -> Result<bool, ExpandError> {
    if *i + 1 >= bytes.len() || bytes[*i] != b'$' {
        return Ok(false);
    }
    match bytes[*i + 1] {
        b'\'' => ansi_c_quote(bytes, i, ctx, out),
        b'"' => locale_quote(bytes, i, ctx, out),
        _ => Ok(false),
    }
}
