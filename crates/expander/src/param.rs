//! Parameter expansion. Handles `$NAME`, `$N`, special parameters, all
//! `${...}` operator forms, and dollar-prefixed constructs (`$(...)`,
//! `$((...))`, `$'...'`, `$"..."`).
//!
//! Entry point is `param_expand`, called from `internal::expand_word_internal`
//! when it sees a `$` byte.

use std::borrow::Cow;
use std::ffi::CStr;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_common::{AssignError, Environment, ProcSubstDir, VarAttrs, VarKind};

use crate::arith;
use crate::buf::{is_ctl, ExpandBuf, CTLESC, CTLFIELD, CTLNUL, CTLRAW};
use crate::cmdsub;
use crate::ctx::{CurrentSubstMode, ExpCtx};
use crate::error::ExpandError;
use crate::nameref;
use crate::pattern::{
    casemod_with_opts as pat_casemod_with_opts, pat_subst_with_opts, pat_subst_with_replacer,
    remove_pattern_with_opts as pat_remove_with_opts, CaseModMode, GlobOpts, PatSubMode,
};
use crate::procsub;
use crate::quote;

/// Try to parse a `$`-prefixed construct starting at `bytes[*i]` (which points
/// AT the `$`). On success advances `*i` and pushes the expansion into
/// `out`. Returns `Ok(true)` when handled; `Ok(false)` to indicate "not a
/// special construct, push literal $".
pub fn param_expand(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<bool, ExpandError> {
    if *i >= bytes.len() || bytes[*i] != b'$' {
        return Ok(false);
    }
    if *i + 1 >= bytes.len() {
        return Ok(false);
    }
    let next = bytes[*i + 1];
    match next {
        b'{' => parameter_brace_expand(bytes, i, ctx, quoted, out).map(|_| true),
        b'(' => dollar_paren(bytes, i, ctx, quoted, out).map(|_| true),
        b'[' => dollar_bracket(bytes, i, ctx, quoted, out).map(|_| true),
        b'\'' if !quoted => ansi_c_quote(bytes, i, ctx, out),
        b'"' if !quoted => locale_quote(bytes, i, ctx, out),
        b'?' | b'#' | b'$' | b'!' | b'-' | b'*' | b'@' | b'0' => {
            let name = (next as char).to_string();
            *i += 2;
            expand_special(&name, ctx, quoted, out)?;
            Ok(true)
        }
        b if b.is_ascii_digit() => {
            let name = (b as char).to_string();
            *i += 2;
            expand_special(&name, ctx, quoted, out).map_err(|err| match err {
                ExpandError::UnboundVariable(var) if var == name => {
                    ExpandError::UnboundVariable(format!("${var}"))
                }
                other => other,
            })?;
            Ok(true)
        }
        b if b == b'_' || b.is_ascii_alphabetic() => {
            let start = *i + 1;
            let mut j = start;
            while j < bytes.len() && (bytes[j] == b'_' || bytes[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            let name = std::str::from_utf8(&bytes[start..j])
                .map_err(|_| ExpandError::Other("invalid identifier".into()))?;
            *i = j;
            let is_nameref = ctx.env.attrs(name).contains(VarAttrs::NAMEREF);
            if is_nameref {
                if let Some(target) = nameref::resolve(ctx.env, name) {
                    if array_all_ref(&target).is_some() {
                        push_parameter_value(ctx, &target, quoted, out)?;
                        return Ok(true);
                    }
                }
            }
            let value_bytes = if !ctx.eval_unbound_error && !is_nameref {
                let value = ctx.env.get_cow(name).unwrap_or(Cow::Borrowed(""));
                quote::shell_string_to_bytes(value.as_ref())
            } else {
                let value = read_scalar(ctx, name, false)?;
                quote::shell_string_to_bytes(value.as_ref())
            };
            push_value(out, &value_bytes, quoted);
            Ok(true)
        }
        _ => Ok(false),
    }
}

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
    // Treat $"..." as a double-quoted run (no gettext translation - parity
    // caveat). Step past `$"` and delegate.
    *i += 2;
    crate::internal::scan_double_quoted_into(bytes, i, ctx, out)?;
    Ok(true)
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

/// Implements the dispatcher at `subst.c:9539 parameter_brace_expand`.
fn parameter_brace_expand(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let start = *i + 2; // past ${
    if let Some(mode) = current_subst_mode(bytes, start) {
        let body_start = if mode == CurrentSubstMode::Reply {
            start + 1
        } else {
            start
        };
        let (body, end) = extract_current_brace_body(bytes, body_start)?;
        *i = end;
        let src = String::from_utf8_lossy(&body).into_owned();
        let buf = cmdsub::current_substitute(&src, ctx, quoted, mode)?;
        out.extend_from(&buf);
        return Ok(());
    }
    let posix_single_quote = quoted && ctx.env.option("posix");
    if !quoted && !posix_single_quote {
        if let Some((body, end)) = simple_brace_body(bytes, start) {
            if try_fast_literal_brace_body(body, ctx, out)? {
                *i = end;
                return Ok(());
            }
        }
    }
    let (body, end) = extract_brace_body(bytes, start, posix_single_quote)?;
    *i = end;
    expand_brace_body(&body, ctx, quoted, out)
}

fn current_subst_mode(bytes: &[u8], start: usize) -> Option<CurrentSubstMode> {
    match bytes.get(start).copied() {
        Some(b'|') => Some(CurrentSubstMode::Reply),
        Some(b) if b.is_ascii_whitespace() => Some(CurrentSubstMode::Output),
        _ => None,
    }
}

fn simple_brace_body(bytes: &[u8], start: usize) -> Option<(&[u8], usize)> {
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'}' => return Some((&bytes[start..i], i + 1)),
            b'{' | b'$' | b'`' | b'\'' | b'"' | b'\\' => return None,
            _ => i += 1,
        }
    }
    None
}

fn try_fast_literal_brace_body(
    body: &[u8],
    ctx: &mut ExpCtx,
    out: &mut ExpandBuf,
) -> Result<bool, ExpandError> {
    if let Some(bytes) = try_fast_literal_brace_body_bytes(body, ctx.env, ctx.eval_unbound_error)? {
        out.push_raw(&bytes);
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn try_fast_literal_braced_scalar(
    word: &str,
    env: &mut dyn Environment,
) -> Result<Option<String>, ExpandError> {
    let bytes = word.as_bytes();
    if !bytes.starts_with(b"${") {
        return Ok(None);
    }
    let Some((body, end)) = simple_brace_body(bytes, 2) else {
        return Ok(None);
    };
    if end != bytes.len() {
        return Ok(None);
    }
    let eval_unbound_error = env.option("nounset");
    let Some(bytes) = try_fast_literal_brace_body_bytes(body, env, eval_unbound_error)? else {
        return Ok(None);
    };
    Ok(Some(quote::bytes_to_shell_string(&bytes)))
}

fn try_fast_literal_brace_body_bytes(
    body: &[u8],
    env: &dyn Environment,
    eval_unbound_error: bool,
) -> Result<Option<Vec<u8>>, ExpandError> {
    let Some(first) = body.first().copied() else {
        return Ok(None);
    };
    if !(first == b'_' || first.is_ascii_alphabetic()) {
        return Ok(None);
    }
    let mut p = 1;
    while p < body.len() && (body[p] == b'_' || body[p].is_ascii_alphanumeric()) {
        p += 1;
    }
    let name = std::str::from_utf8(&body[..p])
        .map_err(|_| ExpandError::BadSubstitution("non-utf8 name".into()))?;
    let opts = pattern_match_opts_env(env);
    match body.get(p).copied() {
        Some(b'#') | Some(b'%') => {
            let pat_start = if body.get(p + 1) == body.get(p) {
                p + 2
            } else {
                p + 1
            };
            let pattern = &body[pat_start..];
            let Some(pattern) = raw_literal_pattern(pattern, opts) else {
                return Ok(None);
            };
            let Some(value) = read_plain_scalar_cow_env(env, eval_unbound_error, name)? else {
                return Ok(None);
            };
            let Some(bytes) = plain_shell_string_bytes(value.as_ref()) else {
                return Ok(None);
            };
            Ok(Some(
                literal_remove_slice(bytes, pattern, body[p] == b'#').to_vec(),
            ))
        }
        Some(b'/') if body.get(p + 1) == Some(&b'/') => {
            if env.option("patsub_replacement") {
                return Ok(None);
            }
            let rest = &body[p + 2..];
            let (pat_raw, rep_raw) = match rest.iter().position(|b| *b == b'/') {
                Some(sep) => (&rest[..sep], &rest[sep + 1..]),
                None => (rest, &b""[..]),
            };
            let (Some(pattern), Some(replacement)) = (
                raw_literal_pattern(pat_raw, opts),
                raw_literal_replacement(rep_raw),
            ) else {
                return Ok(None);
            };
            let Some(value) = read_plain_scalar_cow_env(env, eval_unbound_error, name)? else {
                return Ok(None);
            };
            let Some(bytes) = plain_shell_string_bytes(value.as_ref()) else {
                return Ok(None);
            };
            Ok(Some(literal_substitute(
                bytes,
                pattern,
                replacement,
                PatSubMode::All,
            )))
        }
        _ => Ok(None),
    }
}

fn expand_brace_body(
    body: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    if body.is_empty() {
        return Err(ExpandError::BadSubstitution("${}".into()));
    }
    let mut p = 0;
    // ${#NAME} - length, ${#@}, ${#*}. Bash lets the special `#`
    // parameter use operators too, so `${#:-x}` is not a length expansion.
    if body[0] == b'#' && body.len() > 1 && !hash_prefix_is_parameter(body) {
        let name = &body[1..];
        if !valid_length_parameter_name(name) {
            return Err(bad_substitution_body(body));
        }
        let len = compute_length(name, ctx)?;
        push_value(out, len.to_string().as_bytes(), quoted);
        return Ok(());
    }
    // ${!...}; when `!` is followed by an operator, it is the special `$!`
    // parameter rather than indirect expansion.
    if body[0] == b'!'
        && body.len() > 1
        && !bang_prefix_is_parameter(body)
        && !(ctx.env.option("posix") && matches!(body[1], b'?' | b'-'))
    {
        return handle_indirection(&body[1..], ctx, quoted, out);
    }
    // Read the variable name (possibly with array subscript). With extquote,
    // Bash lets an ANSI-C quoted word supply the parameter name, as in
    // `${$'name'%pattern}`.
    let name = if let Some((quoted_name, quoted_end)) = ansi_c_quoted_parameter_name(body, ctx)? {
        p = quoted_end;
        Cow::Owned(quoted_name)
    } else {
        let name_start = p;
        while p < body.len() {
            let b = body[p];
            if b == b'[' {
                p = skip_parameter_subscript(body, p);
                break;
            }
            if b == b'_' || b.is_ascii_alphanumeric() {
                p += 1;
                continue;
            }
            break;
        }
        // Handle leading single special-char names like $? in ${?}
        if p == name_start {
            // Could be a one-char special: # $ ? ! - * @ 0-9
            if matches!(
                body[0],
                b'#' | b'?' | b'$' | b'!' | b'-' | b'*' | b'@' | b'0'..=b'9'
            ) {
                p = 1;
            }
        }
        if p == name_start {
            return Err(bad_substitution_body(body));
        }
        Cow::Borrowed(
            std::str::from_utf8(&body[name_start..p])
                .map_err(|_| ExpandError::BadSubstitution("non-utf8 name".into()))?,
        )
    };
    let name = name.as_ref();
    if p >= body.len() {
        push_parameter_value(ctx, &name, quoted, out)?;
        return Ok(());
    }
    // Operator dispatch.
    let op_byte = body[p];
    if op_byte == b':' && p + 1 >= body.len() {
        return Err(bad_substitution_body(body));
    }
    let (colon, op_byte, op_pos) = if op_byte == b':' && p + 1 < body.len() {
        let next = body[p + 1];
        if matches!(next, b'-' | b'=' | b'?' | b'+') {
            (true, next, p + 2)
        } else {
            // ${var:offset[:len]} - substring
            if p + 1 >= body.len() {
                return Err(bad_substitution_body(body));
            }
            return handle_substring(&name, &body[p + 1..], ctx, quoted, out);
        }
    } else {
        (false, op_byte, p + 1)
    };
    if name == "#" && op_pos >= body.len() && matches!(op_byte, b'/' | b'%' | b'=' | b'+') {
        return Err(bad_substitution_body(body));
    }
    match op_byte {
        b'-' | b'=' | b'?' | b'+' => {
            let word = &body[op_pos..];
            handle_default_alt(&name, op_byte, colon, word, ctx, quoted, out)
        }
        b'#' => {
            let longest = body.get(op_pos) == Some(&b'#');
            let pat_offset = if longest { op_pos + 1 } else { op_pos };
            handle_remove(&name, &body[pat_offset..], true, longest, ctx, quoted, out)
        }
        b'%' => {
            let longest = body.get(op_pos) == Some(&b'%');
            let pat_offset = if longest { op_pos + 1 } else { op_pos };
            handle_remove(&name, &body[pat_offset..], false, longest, ctx, quoted, out)
        }
        b'/' => handle_patsub(&name, &body[op_pos..], ctx, quoted, out),
        b'^' => {
            let all = body.get(op_pos) == Some(&b'^');
            let pat_offset = if all { op_pos + 1 } else { op_pos };
            handle_casemod(
                &name,
                &body[pat_offset..],
                if all {
                    CaseModMode::UpperAll
                } else {
                    CaseModMode::UpperFirst
                },
                ctx,
                quoted,
                out,
            )
        }
        b',' => {
            let all = body.get(op_pos) == Some(&b',');
            let pat_offset = if all { op_pos + 1 } else { op_pos };
            handle_casemod(
                &name,
                &body[pat_offset..],
                if all {
                    CaseModMode::LowerAll
                } else {
                    CaseModMode::LowerFirst
                },
                ctx,
                quoted,
                out,
            )
        }
        b'~' => {
            let all = body.get(op_pos) == Some(&b'~');
            let pat_offset = if all { op_pos + 1 } else { op_pos };
            handle_casemod(
                &name,
                &body[pat_offset..],
                if all {
                    CaseModMode::ToggleAll
                } else {
                    CaseModMode::ToggleFirst
                },
                ctx,
                quoted,
                out,
            )
        }
        b'@' => handle_transform(&name, &body[op_pos..], ctx, quoted, out),
        _ => Err(bad_substitution_body(body)),
    }
}

fn skip_parameter_subscript(body: &[u8], mut p: usize) -> usize {
    let mut depth = 1usize;
    p += 1;
    while p < body.len() && depth > 0 {
        match body[p] {
            b'\\' => p = (p + 2).min(body.len()),
            b'\'' => {
                p += 1;
                while p < body.len() {
                    if body[p] == b'\'' {
                        p += 1;
                        break;
                    }
                    p += 1;
                }
            }
            b'"' => {
                p += 1;
                while p < body.len() {
                    match body[p] {
                        b'\\' => p = (p + 2).min(body.len()),
                        b'"' => {
                            p += 1;
                            break;
                        }
                        b'$' if body.get(p + 1) == Some(&b'(') => {
                            p = extract_paren(body, p + 2)
                                .map(|(_, end)| end)
                                .unwrap_or(body.len());
                        }
                        b'$' if body.get(p + 1) == Some(&b'{') => {
                            p = skip_balanced_param_construct(body, p + 1, b'{', b'}');
                        }
                        _ => p += 1,
                    }
                }
            }
            b'$' if body.get(p + 1) == Some(&b'(') => {
                p = extract_paren(body, p + 2)
                    .map(|(_, end)| end)
                    .unwrap_or(body.len());
            }
            b'$' if body.get(p + 1) == Some(&b'{') => {
                p = skip_balanced_param_construct(body, p + 1, b'{', b'}');
            }
            b'[' => {
                depth += 1;
                p += 1;
            }
            b']' => {
                depth -= 1;
                p += 1;
            }
            _ => p += 1,
        }
    }
    p
}

fn skip_balanced_param_construct(body: &[u8], mut p: usize, open: u8, close: u8) -> usize {
    let mut depth = 1usize;
    p += 1;
    while p < body.len() && depth > 0 {
        match body[p] {
            b'\\' => p = (p + 2).min(body.len()),
            b'\'' | b'"' => {
                let quote = body[p];
                p += 1;
                while p < body.len() {
                    if body[p] == b'\\' && quote == b'"' {
                        p = (p + 2).min(body.len());
                    } else if body[p] == quote {
                        p += 1;
                        break;
                    } else {
                        p += 1;
                    }
                }
            }
            b if b == open => {
                depth += 1;
                p += 1;
            }
            b if b == close => {
                depth -= 1;
                p += 1;
            }
            _ => p += 1,
        }
    }
    p
}

fn compute_length(name: &[u8], ctx: &mut ExpCtx) -> Result<usize, ExpandError> {
    if name == b"@" || name == b"*" {
        return Ok(ctx.env.positional_count());
    }
    let nm =
        std::str::from_utf8(name).map_err(|_| ExpandError::BadSubstitution("non-utf8".into()))?;
    // Strip subscript for length
    if let Some(bracket) = nm.find('[') {
        let base = &nm[..bracket];
        let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
        let kind = ctx.env.kind(&target);
        let close = nm.rfind(']').unwrap_or(nm.len());
        let subscript = &nm[bracket + 1..close];
        if ctx.eval_unbound_error
            && matches!(kind, VarKind::Unset)
            && ctx.env.get(&target).is_none()
        {
            return Err(ExpandError::UnboundVariable(format!("{base}[{subscript}]")));
        }
        if subscript == "@" || subscript == "*" {
            if target != base && target.contains('[') {
                return Ok(0);
            }
            return Ok(match kind {
                VarKind::Assoc => ctx.env.assoc_len(&target),
                VarKind::Indexed => ctx.env.array_len(&target),
                _ => usize::from(ctx.env.get(base).is_some()),
            });
        }
        if kind == VarKind::Assoc {
            let key = crate::expand_string_to_string_impl(subscript, ctx)?;
            if key.is_empty() {
                return Err(ExpandError::InvalidArraySubscript(format!("[{subscript}]")));
            }
            return Ok(ctx
                .env
                .get_array_assoc(&target, &key)
                .map(|v| scalar_length(&v, ctx.env))
                .unwrap_or(0));
        }
        let label = format!("[{subscript}]");
        let idx = parse_indexed_subscript(ctx, &target, subscript, &label)?;
        if kind != VarKind::Indexed {
            return Ok(if idx == 0 {
                ctx.env
                    .get(&target)
                    .map(|v| scalar_length(&v, ctx.env))
                    .unwrap_or(0)
            } else {
                0
            });
        }
        return Ok(ctx
            .env
            .get_array_indexed(&target, idx)
            .map(|v| scalar_length(&v, ctx.env))
            .unwrap_or(0));
    }
    if let Some(target) = nameref::resolve(ctx.env, nm) {
        if target != nm {
            if let Some((base, _)) = array_all_ref(&target) {
                return Ok(match ctx.env.kind(base) {
                    VarKind::Assoc => ctx.env.assoc_len(base),
                    VarKind::Indexed => ctx.env.array_len(base),
                    _ => usize::from(ctx.env.get(base).is_some()),
                });
            }
            if target.contains('[') {
                let value = read_scalar(ctx, &target, true)?;
                return Ok(scalar_length(&value, ctx.env));
            }
        }
    }
    Ok(scalar_length(&read_scalar(ctx, nm, true)?, ctx.env))
}

fn scalar_length(value: &str, env: &dyn Environment) -> usize {
    let bytes = crate::quote::shell_string_to_bytes(value);
    if locale_is_utf8(env) {
        std::str::from_utf8(&bytes)
            .map(|value| value.chars().count())
            .unwrap_or(bytes.len())
    } else {
        bytes.len()
    }
}

fn locale_is_utf8(env: &dyn Environment) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|name| env.get(name))
        .find(|value| !value.is_empty())
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("utf-8") || lower.contains("utf8")
        })
        .unwrap_or(false)
}

fn bad_substitution_body(body: &[u8]) -> ExpandError {
    let display = String::from_utf8_lossy(body).replace("$'", "'");
    ExpandError::BadSubstitution(format!("${{{}}}", display))
}

fn ansi_c_quoted_parameter_name(
    body: &[u8],
    ctx: &ExpCtx,
) -> Result<Option<(String, usize)>, ExpandError> {
    if !ctx.env.option("extquote") || !body.starts_with(b"$'") {
        return Ok(None);
    }
    let (raw, end) = quote::scan_ansi_c_quoted(body, 2);
    if body.get(end.saturating_sub(1)) != Some(&b'\'') {
        return Ok(None);
    }
    let decoded = quote::ansi_c_decode(&raw);
    if !valid_length_parameter_name(&decoded) {
        return Err(bad_substitution_body(body));
    }
    let name = String::from_utf8(decoded)
        .map_err(|_| ExpandError::BadSubstitution("non-utf8 name".into()))?;
    Ok(Some((name, end)))
}

fn hash_prefix_is_parameter(body: &[u8]) -> bool {
    if body.len() < 2 || body[0] != b'#' {
        return false;
    }
    match body[1] {
        b':' | b'/' | b'%' | b'=' | b'+' => true,
        b'-' | b'?' => body.len() > 2,
        _ => false,
    }
}

fn bang_prefix_is_parameter(body: &[u8]) -> bool {
    if body.len() < 2 || body[0] != b'!' {
        return false;
    }
    match body[1] {
        b':' | b'/' | b'%' | b'=' | b'+' => true,
        b'-' | b'?' => body.len() > 2,
        _ => false,
    }
}

fn valid_length_parameter_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    if matches!(name, b"@" | b"*" | b"?" | b"$" | b"!" | b"-" | b"#") {
        return true;
    }
    if name.iter().all(u8::is_ascii_digit) {
        return true;
    }
    if name[0].is_ascii_digit() {
        return false;
    }
    if !(name[0] == b'_' || name[0].is_ascii_alphabetic()) {
        return false;
    }
    let mut i = 1;
    while i < name.len() && (name[i] == b'_' || name[i].is_ascii_alphanumeric()) {
        i += 1;
    }
    if i == name.len() {
        return true;
    }
    name[i] == b'[' && name.ends_with(b"]")
}

fn handle_indirection(
    rest: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    if rest.is_empty() {
        return Err(ExpandError::BadSubstitution("${!}".into()));
    }
    // Prefix: ${!pfx*} ${!pfx@}
    if rest.last() == Some(&b'*') || rest.last() == Some(&b'@') {
        let star_or_at = *rest.last().unwrap();
        let prefix = &rest[..rest.len() - 1];
        if prefix.is_empty() {
            return Ok(());
        }
        if is_name_start(prefix[0]) && prefix[1..].iter().all(|b| is_name_byte(*b)) {
            let p = std::str::from_utf8(prefix).unwrap_or("");
            let names = ctx.env.all_var_names_with_prefix(p);
            let sep = if star_or_at == b'*' {
                ctx.ifs.join_separator().unwrap_or(b' ')
            } else {
                b' '
            };
            if quoted && star_or_at == b'*' {
                let sep_s = (sep as char).to_string();
                push_value(out, names.join(&sep_s).as_bytes(), true);
                return Ok(());
            }
            if quoted {
                out.buf_record_dollar_at();
                for (i, n) in names.iter().enumerate() {
                    if i > 0 {
                        out.push_field_sep();
                    }
                    push_value(out, n.as_bytes(), true);
                }
                return Ok(());
            }
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    out.push_literal(sep);
                }
                push_value(out, n.as_bytes(), false);
            }
            return Ok(());
        }
    }
    // Array keys: ${!arr[@]} ${!arr[*]}
    if let Some(bracket) = rest.iter().position(|&b| b == b'[') {
        let base = std::str::from_utf8(&rest[..bracket])
            .map_err(|_| ExpandError::BadSubstitution("non-utf8".into()))?;
        let inner = &rest[bracket + 1..];
        if inner == b"@]" || inner == b"*]" {
            let key_base = if ctx.env.attrs(base).contains(VarAttrs::NAMEREF) {
                nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string())
            } else {
                base.to_string()
            };
            let kind = ctx.env.kind(&key_base);
            let sep = if inner.starts_with(b"*") {
                ctx.ifs.join_separator()
            } else {
                Some(b' ')
            };
            match kind {
                VarKind::Indexed => {
                    if let Some(ks) = ctx.env.array_keys(&key_base) {
                        if quoted && inner.starts_with(b"*") {
                            let sep_s = sep.map(|b| (b as char).to_string()).unwrap_or_default();
                            let joined = ks
                                .iter()
                                .map(i64::to_string)
                                .collect::<Vec<_>>()
                                .join(&sep_s);
                            push_value(out, joined.as_bytes(), true);
                            return Ok(());
                        }
                        for (i, k) in ks.iter().enumerate() {
                            if i > 0 {
                                if let Some(sep) = sep {
                                    out.push_literal(sep);
                                }
                            }
                            push_value(out, k.to_string().as_bytes(), quoted);
                        }
                    }
                }
                VarKind::Assoc => {
                    if let Some(ks) = ctx.env.assoc_keys(&key_base) {
                        if quoted && inner.starts_with(b"*") {
                            let sep_s = sep.map(|b| (b as char).to_string()).unwrap_or_default();
                            let joined = ks.join(&sep_s);
                            push_value(out, joined.as_bytes(), true);
                            return Ok(());
                        }
                        for (i, k) in ks.iter().enumerate() {
                            if i > 0 {
                                if let Some(sep) = sep {
                                    out.push_literal(sep);
                                }
                            }
                            push_value(out, k.as_bytes(), quoted);
                        }
                    }
                }
                VarKind::Scalar | VarKind::Nameref => {
                    push_value(out, b"0", quoted);
                }
                VarKind::Unset => {}
            }
            return Ok(());
        }
    }
    // Plain indirection ${!name}
    let name_end = indirect_name_end(rest)
        .ok_or_else(|| ExpandError::BadSubstitution(format!("${{!{}}}", bytes_display(rest))))?;
    let nm = std::str::from_utf8(&rest[..name_end])
        .map_err(|_| ExpandError::BadSubstitution("non-utf8".into()))?;
    if name_end < rest.len() {
        let op_byte = rest[name_end];
        if matches!(
            op_byte,
            b'@' | b'[' | b'#' | b'%' | b'/' | b'^' | b',' | b'~'
        ) {
            return handle_indirect_target_operation(nm, &rest[name_end..], ctx, quoted, out);
        }
        let (colon, op_byte, op_pos) = if op_byte == b':' && name_end + 1 < rest.len() {
            let next = rest[name_end + 1];
            if matches!(next, b'-' | b'=' | b'?' | b'+') {
                (true, next, name_end + 2)
            } else {
                return Err(ExpandError::BadSubstitution(format!(
                    "${{!{}}}",
                    bytes_display(rest)
                )));
            }
        } else {
            (false, op_byte, name_end + 1)
        };
        if !matches!(op_byte, b'-' | b'=' | b'?' | b'+') {
            return Err(ExpandError::BadSubstitution(format!(
                "${{!{}}}",
                bytes_display(rest)
            )));
        }
        return handle_indirect_default_alt(nm, op_byte, colon, &rest[op_pos..], ctx, quoted, out);
    }
    if ctx.env.attrs(nm).contains(VarAttrs::NAMEREF) {
        let target = nameref::resolve(ctx.env, nm).unwrap_or_else(|| nm.to_string());
        if target.is_empty() {
            return Err(ExpandError::Other("invalid variable name".into()));
        }
        push_value(out, target.as_bytes(), quoted);
        return Ok(());
    }
    if !parameter_is_set(ctx, nm) && is_valid_name_str(nm) {
        return Err(ExpandError::Other(format!(
            "{nm}: invalid indirect expansion"
        )));
    }
    let target = read_scalar(ctx, nm, false)?;
    if target.is_empty() {
        return Err(ExpandError::Other("invalid variable name".into()));
    }
    if !valid_length_parameter_name(target.as_bytes()) {
        return Err(ExpandError::Other(format!(
            "{target}: invalid variable name"
        )));
    }
    push_parameter_value(ctx, &target, quoted, out)?;
    Ok(())
}

fn handle_indirect_target_operation(
    source_name: &str,
    suffix: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let source_all_ref = suffix.starts_with(b"[@]") || suffix.starts_with(b"[*]");
    if suffix.starts_with(b"[")
        && !source_all_ref
        && ctx.env.attrs(source_name).contains(VarAttrs::NAMEREF)
    {
        let target =
            nameref::resolve(ctx.env, source_name).unwrap_or_else(|| source_name.to_string());
        if matches!(ctx.env.kind(&target), VarKind::Unset) {
            return Err(ExpandError::Other(format!(
                "{source_name}{}: invalid indirect expansion",
                bytes_display(suffix)
            )));
        }
        return Ok(());
    }
    let target = if source_all_ref
        && matches!(ctx.env.kind(source_name), VarKind::Indexed | VarKind::Assoc)
    {
        join_array(ctx, source_name, b' ')
    } else {
        read_scalar(ctx, source_name, false)?
    };
    if target.is_empty() {
        return Err(ExpandError::Other("invalid variable name".into()));
    }

    let mut op = suffix;
    if source_all_ref {
        if !valid_length_parameter_name(target.as_bytes()) {
            return Err(ExpandError::Other(format!(
                "{target}: invalid variable name"
            )));
        }
        op = &suffix[3..];
    }
    if !valid_length_parameter_name(target.as_bytes()) {
        return Err(ExpandError::Other(format!(
            "{target}: invalid variable name"
        )));
    }
    if op.is_empty() {
        return push_parameter_value(ctx, &target, quoted, out);
    }
    match op[0] {
        b'@' => {
            if matches!(op.get(1), Some(b'a' | b'A'))
                && ctx.eval_unbound_error
                && !parameter_has_transform_value(ctx, &target)
            {
                return Err(ExpandError::UnboundVariable(format!("!{source_name}")));
            }
            handle_transform(&target, &op[1..], ctx, quoted, out)
        }
        b'#' => {
            let longest = op.get(1) == Some(&b'#');
            let pat_offset = if longest { 2 } else { 1 };
            handle_remove(&target, &op[pat_offset..], true, longest, ctx, quoted, out)
        }
        b'%' => {
            let longest = op.get(1) == Some(&b'%');
            let pat_offset = if longest { 2 } else { 1 };
            handle_remove(&target, &op[pat_offset..], false, longest, ctx, quoted, out)
        }
        b'/' => handle_patsub(&target, &op[1..], ctx, quoted, out),
        b'^' => {
            let all = op.get(1) == Some(&b'^');
            let pat_offset = if all { 2 } else { 1 };
            handle_casemod(
                &target,
                &op[pat_offset..],
                if all {
                    CaseModMode::UpperAll
                } else {
                    CaseModMode::UpperFirst
                },
                ctx,
                quoted,
                out,
            )
        }
        b',' => {
            let all = op.get(1) == Some(&b',');
            let pat_offset = if all { 2 } else { 1 };
            handle_casemod(
                &target,
                &op[pat_offset..],
                if all {
                    CaseModMode::LowerAll
                } else {
                    CaseModMode::LowerFirst
                },
                ctx,
                quoted,
                out,
            )
        }
        b'~' => {
            let all = op.get(1) == Some(&b'~');
            let pat_offset = if all { 2 } else { 1 };
            handle_casemod(
                &target,
                &op[pat_offset..],
                if all {
                    CaseModMode::ToggleAll
                } else {
                    CaseModMode::ToggleFirst
                },
                ctx,
                quoted,
                out,
            )
        }
        _ => Err(ExpandError::BadSubstitution(format!(
            "${{!{}{}}}",
            source_name,
            bytes_display(suffix)
        ))),
    }
}

fn indirect_name_end(rest: &[u8]) -> Option<usize> {
    if rest.is_empty() {
        return None;
    }
    if is_name_start(rest[0]) {
        let mut p = 1;
        while p < rest.len() && is_name_byte(rest[p]) {
            p += 1;
        }
        Some(p)
    } else if matches!(
        rest[0],
        b'#' | b'?' | b'$' | b'!' | b'-' | b'*' | b'@' | b'0'..=b'9'
    ) {
        Some(1)
    } else {
        None
    }
}

fn bytes_display(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn handle_indirect_default_alt(
    name: &str,
    op: u8,
    colon: bool,
    word: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let source_is_set = parameter_is_set(ctx, name);
    if !source_is_set {
        if is_valid_name_str(name) {
            return Err(ExpandError::Other(format!(
                "{name}: invalid indirect expansion"
            )));
        }
        return handle_unset_indirect_default(name, op, word, ctx, quoted, out);
    }
    let target = read_scalar(ctx, name, false)?;
    if target.is_empty() {
        return Err(ExpandError::Other("invalid variable name".into()));
    }
    if !valid_length_parameter_name(target.as_bytes()) {
        return Err(ExpandError::Other(format!(
            "{target}: invalid variable name"
        )));
    }
    handle_default_alt(&target, op, colon, word, ctx, quoted, out)
}

fn handle_unset_indirect_default(
    name: &str,
    op: u8,
    word: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    match op {
        b'-' => {
            let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
            if quoted && word.is_empty() {
                out.push_quoted_null();
            } else {
                let expanded = expand_default_word(&word_str, ctx, quoted)?;
                extend_default_expansion(out, &expanded, quoted);
            }
            Ok(())
        }
        b'+' => {
            if quoted {
                out.push_quoted_null();
            }
            Ok(())
        }
        b'=' => {
            let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
            let expanded = expand_assignment_default_word(&word_str, ctx, quoted)?;
            let s = assignment_value_from_expanded(expanded.as_bytes());
            let assigned = assign_default_parameter(ctx, name, s)?;
            push_value(out, assigned.as_bytes(), quoted);
            Ok(())
        }
        b'?' => {
            let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
            let msg = if word.is_empty() {
                String::new()
            } else {
                let expanded = expand_default_word(&word_str, ctx, quoted)?;
                String::from_utf8_lossy(&crate::buf::dequote_bytes(expanded.as_bytes()))
                    .into_owned()
            };
            Err(ExpandError::UnboundColonError(name.into(), msg))
        }
        _ => unreachable!(),
    }
}

fn handle_default_alt(
    name: &str,
    op: u8,
    colon: bool,
    word: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let mut value = None;
    let is_set = match read_element_value_if_set(ctx, name)? {
        ElementLookup::Set(v) => {
            value = Some(v);
            true
        }
        ElementLookup::Unset => false,
        ElementLookup::NotElement => parameter_is_set(ctx, name),
    };
    match op {
        b'-' => {
            let use_default = if !is_set {
                true
            } else if colon {
                default_test_string(
                    read_value_cached(ctx, name, quoted, &mut value)?,
                    ctx.ifs.join_separator(),
                )
                .is_empty()
            } else {
                false
            };
            if use_default {
                let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
                if quoted && word.is_empty() {
                    out.push_quoted_null();
                } else {
                    let expanded = expand_default_word(&word_str, ctx, quoted)?;
                    extend_default_expansion(out, &expanded, quoted);
                }
            } else {
                let value = read_value_cached(ctx, name, quoted, &mut value)?;
                push_value_buf(
                    out,
                    value,
                    quoted,
                    ctx.ifs.join_separator(),
                    ctx.ifs.is_null,
                    ctx.assign_in_progress,
                    ctx.split_fields,
                );
            }
            Ok(())
        }
        b'+' => {
            let use_alternate = if !is_set {
                false
            } else if colon {
                !default_test_string(
                    read_value_cached(ctx, name, quoted, &mut value)?,
                    ctx.ifs.join_separator(),
                )
                .is_empty()
            } else {
                true
            };
            if !use_alternate {
                if quoted && !quoted_at_alternate_has_no_fields(name, is_set) {
                    out.push_quoted_null();
                }
                Ok(())
            } else {
                let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
                if quoted && word.is_empty() {
                    out.push_quoted_null();
                } else {
                    let expanded = expand_default_word(&word_str, ctx, quoted)?;
                    extend_default_expansion(out, &expanded, quoted);
                }
                Ok(())
            }
        }
        b'=' => {
            let assign_value = if !is_set {
                true
            } else if colon {
                default_test_string(
                    read_value_cached(ctx, name, quoted, &mut value)?,
                    ctx.ifs.join_separator(),
                )
                .is_empty()
            } else {
                false
            };
            if assign_value {
                let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
                let expanded = expand_assignment_default_word(&word_str, ctx, quoted)?;
                let s = assignment_value_from_expanded(expanded.as_bytes());
                let assigned = assign_default_parameter(ctx, name, s)?;
                if !quoted && expanded.as_bytes().contains(&CTLFIELD) && !ctx.ifs.is_null {
                    extend_assignment_default_expansion(out, &expanded, quoted, &ctx.ifs);
                } else {
                    push_value(out, assigned.as_bytes(), quoted);
                }
            } else {
                let value = read_value_cached(ctx, name, quoted, &mut value)?;
                push_value_buf(
                    out,
                    value,
                    quoted,
                    ctx.ifs.join_separator(),
                    ctx.ifs.is_null,
                    ctx.assign_in_progress,
                    ctx.split_fields,
                );
            }
            Ok(())
        }
        b'?' => {
            let fail = if !is_set {
                true
            } else if colon {
                default_test_string(
                    read_value_cached(ctx, name, quoted, &mut value)?,
                    ctx.ifs.join_separator(),
                )
                .is_empty()
            } else {
                false
            };
            if fail {
                let word_str = std::str::from_utf8(word).unwrap_or("").to_string();
                let msg = if word.is_empty() {
                    if colon {
                        String::new()
                    } else {
                        "parameter not set".to_string()
                    }
                } else {
                    let expanded = expand_default_word(&word_str, ctx, quoted)?;
                    String::from_utf8_lossy(&crate::buf::dequote_bytes(expanded.as_bytes()))
                        .into_owned()
                };
                Err(ExpandError::UnboundColonError(name.into(), msg))
            } else {
                let value = read_value_cached(ctx, name, quoted, &mut value)?;
                push_value_buf(
                    out,
                    value,
                    quoted,
                    ctx.ifs.join_separator(),
                    ctx.ifs.is_null,
                    ctx.assign_in_progress,
                    ctx.split_fields,
                );
                Ok(())
            }
        }
        _ => unreachable!(),
    }
}

fn quoted_at_alternate_has_no_fields(name: &str, is_set: bool) -> bool {
    !is_set && (name == "@" || array_all_ref(name).is_some_and(|(_, subscript)| subscript == "@"))
}

fn read_value_cached<'a>(
    ctx: &mut ExpCtx,
    name: &str,
    quoted: bool,
    value: &'a mut Option<ValueRepr>,
) -> Result<&'a ValueRepr, ExpandError> {
    if value.is_none() {
        *value = Some(read_value(ctx, name, quoted)?);
    }
    Ok(value.as_ref().unwrap())
}

fn assignment_value_from_expanded(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
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
            CTLFIELD => {
                out.push(b' ');
                i += 1;
            }
            CTLNUL => {
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn assign_default_parameter(
    ctx: &mut ExpCtx,
    name: &str,
    value: String,
) -> Result<String, ExpandError> {
    if matches!(name.parse::<usize>(), Ok(n) if n != 0) {
        return Err(ExpandError::Other(format!(
            "${name}: cannot assign in this way"
        )));
    }
    if let Some(bracket) = name.find('[') {
        let base = &name[..bracket];
        let close = name.rfind(']').unwrap_or(name.len());
        let subscript = &name[bracket + 1..close];
        let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
        if ctx.env.is_readonly(&target) {
            return Err(ExpandError::AssignToReadonly(target));
        }
        if ctx.env.kind(&target) == VarKind::Assoc {
            let key = crate::expand_string_to_string_impl(subscript, ctx)?;
            let value = apply_assignment_attrs(ctx, &target, value)?;
            ctx.env.set_array_assoc(&target, &key, value.clone());
            return Ok(value);
        }
        if matches!(subscript, "@" | "*") {
            return Err(ExpandError::InvalidArraySubscript(name.to_string()));
        }
        let index = parse_indexed_subscript(ctx, &target, subscript, name)?;
        let value = apply_assignment_attrs(ctx, &target, value)?;
        ctx.env.set_array_indexed(&target, index, value.clone());
        return Ok(value);
    }

    let unresolved_nameref = ctx.env.attrs(name).contains(VarAttrs::NAMEREF)
        && ctx
            .env
            .nameref_target(name)
            .as_deref()
            .unwrap_or_default()
            .is_empty();
    let target = nameref::resolve(ctx.env, name).unwrap_or_else(|| name.to_string());
    let value = apply_assignment_attrs(ctx, &target, value)?;
    ctx.env
        .assign(name, value.clone())
        .map_err(expand_assign_error)?;
    if unresolved_nameref {
        return Ok(value);
    }
    Ok(read_scalar(ctx, name, false).unwrap_or(value))
}

fn apply_assignment_attrs(
    ctx: &mut ExpCtx,
    target: &str,
    mut value: String,
) -> Result<String, ExpandError> {
    let attrs = ctx.env.attrs(target);
    if attrs.contains(VarAttrs::INTEGER) {
        value = arith::eval_preexpanded(&value, ctx)?.to_string();
    }
    if attrs.contains(VarAttrs::UPPERCASE) {
        value = value.to_uppercase();
    }
    if attrs.contains(VarAttrs::LOWERCASE) {
        value = value.to_lowercase();
    }
    if attrs.contains(VarAttrs::CAPCASE) {
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars();
        if let Some(first) = chars.next() {
            if first.is_alphabetic() {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
        }
        value = out;
    }
    Ok(value)
}

fn expand_assign_error(err: AssignError) -> ExpandError {
    match err {
        AssignError::ReadOnly(name) => ExpandError::AssignToReadonly(name),
        AssignError::BadArraySubscript(name) => ExpandError::InvalidArraySubscript(name),
        AssignError::InvalidName(name) => {
            ExpandError::Other(format!("`{name}': not a valid identifier"))
        }
        AssignError::InvalidInteger(value) => {
            ExpandError::Other(format!("{value}: invalid integer"))
        }
        AssignError::CircularNameReference(name) => {
            ExpandError::Other(format!("{name}: circular name reference"))
        }
    }
}

fn expand_default_word(
    word: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
) -> Result<ExpandBuf, ExpandError> {
    let previous = ctx.param_rhs_nosplit;
    ctx.param_rhs_nosplit = true;
    let result = expand_default_word_inner(word, ctx, quoted);
    ctx.param_rhs_nosplit = previous;
    result
}

fn expand_default_word_inner(
    word: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
) -> Result<ExpandBuf, ExpandError> {
    if ctx.heredoc_context {
        let mut wd = crate::Wd::from_bytes_with_flags(
            word.as_bytes().to_vec(),
            cherubsh_common::W_NOSPLIT | crate::INTERNAL_HEREDOC_CONTEXT,
            cherubsh_common::Span::dummy(),
        );
        if quoted {
            wd.flags |= cherubsh_common::W_QUOTED
                | crate::INTERNAL_QUOTED_CONTEXT
                | crate::INTERNAL_PARAM_WORD_CONTEXT;
        }
        return crate::internal::expand_word_internal(&wd, ctx, quoted).map(|exp| exp.buf);
    }
    crate::expand_word_string(word, ctx, quoted)
}

fn expand_assignment_default_word(
    word: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
) -> Result<ExpandBuf, ExpandError> {
    let previous = ctx.assign_in_progress;
    ctx.assign_in_progress = true;
    let previous_rhs = ctx.param_rhs_nosplit;
    ctx.param_rhs_nosplit = false;
    let result = expand_default_word_inner(word, ctx, quoted);
    ctx.param_rhs_nosplit = previous_rhs;
    ctx.assign_in_progress = previous;
    result
}

fn extend_default_expansion(out: &mut ExpandBuf, expanded: &ExpandBuf, quoted: bool) {
    extend_default_expansion_inner(out, expanded, quoted, false, None);
}

fn extend_assignment_default_expansion(
    out: &mut ExpandBuf,
    expanded: &ExpandBuf,
    quoted: bool,
    ifs: &crate::ifs::IfsState,
) {
    extend_default_expansion_inner(out, expanded, quoted, true, Some(ifs));
}

fn extend_default_expansion_inner(
    out: &mut ExpandBuf,
    expanded: &ExpandBuf,
    quoted: bool,
    drop_empty_fields: bool,
    ifs: Option<&crate::ifs::IfsState>,
) {
    if !quoted {
        if expanded.quoted_dollar_at {
            extend_unquoted_dollar_at_expansion(out, expanded, drop_empty_fields, ifs);
        } else {
            out.extend_from(expanded);
        }
        return;
    }

    if expanded.is_empty() && expanded.quoted_dollar_at {
        out.push_quoted_null();
        out.has_dollar_at |= expanded.has_dollar_at;
        out.quoted_dollar_at |= expanded.quoted_dollar_at;
        return;
    }

    let bytes = expanded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == CTLESC {
            out.push_raw_byte(b);
            i += 1;
            if i < bytes.len() {
                let escaped = bytes[i];
                out.push_raw_byte(escaped);
                i += 1;
                if escaped == CTLRAW && i < bytes.len() {
                    out.push_raw_byte(bytes[i]);
                    i += 1;
                }
            }
            continue;
        }
        if b == CTLNUL {
            out.push_quoted_null();
            i += 1;
            continue;
        }
        if b == CTLFIELD {
            out.push_field_sep();
            i += 1;
            continue;
        }
        out.push_quoted(b);
        i += 1;
    }
    out.has_dollar_at |= expanded.has_dollar_at;
    out.quoted_dollar_at |= expanded.quoted_dollar_at;
}

fn extend_unquoted_dollar_at_expansion(
    out: &mut ExpandBuf,
    expanded: &ExpandBuf,
    drop_empty_fields: bool,
    ifs: Option<&crate::ifs::IfsState>,
) {
    let bytes = expanded.as_bytes();
    let mut first = true;
    for (start, part) in split_ctlfield_parts(bytes) {
        let cleaned = strip_suppressible_nulls(&part, start, &expanded.param_nulls);
        if cleaned.is_empty()
            || (drop_empty_fields
                && ifs.is_some_and(|ifs| field_is_ifs_empty_after_quote_removal(&cleaned, ifs)))
        {
            continue;
        }
        if !first {
            out.push_field_sep();
        }
        out.push_raw(&cleaned);
        first = false;
    }
    out.has_dollar_at |= expanded.has_dollar_at;
    out.quoted_dollar_at |= expanded.quoted_dollar_at;
}

fn field_is_ifs_empty_after_quote_removal(bytes: &[u8], ifs: &crate::ifs::IfsState) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == CTLNUL {
            i += 1;
            continue;
        }
        if b == CTLESC && i + 2 < bytes.len() && bytes[i + 1] == CTLRAW {
            let literal = bytes[i + 2];
            if literal == CTLNUL || ifs.is_ifs_ws(literal) {
                i += 3;
                continue;
            }
            return false;
        }
        if b == CTLESC && i + 1 < bytes.len() {
            let escaped = bytes[i + 1];
            if ifs.is_ifs_ws(escaped) {
                i += 2;
                continue;
            }
            return false;
        }
        if ifs.is_ifs_ws(b) {
            i += 1;
            continue;
        }
        return false;
    }
    true
}

fn split_ctlfield_parts(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    let mut parts = Vec::new();
    let mut cur = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == CTLESC && i + 2 < bytes.len() && bytes[i + 1] == CTLRAW {
            cur.extend_from_slice(&bytes[i..i + 3]);
            i += 3;
            continue;
        }
        if bytes[i] == CTLESC && i + 1 < bytes.len() {
            cur.push(bytes[i]);
            cur.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if bytes[i] == CTLFIELD {
            parts.push((start, std::mem::take(&mut cur)));
            i += 1;
            start = i;
            continue;
        }
        cur.push(bytes[i]);
        i += 1;
    }
    parts.push((start, cur));
    parts
}

fn strip_suppressible_nulls(bytes: &[u8], start: usize, nulls: &[usize]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .filter_map(|(idx, b)| {
            if *b == CTLNUL && nulls.contains(&(start + idx)) {
                None
            } else {
                Some(*b)
            }
        })
        .collect()
}

fn parameter_is_set(ctx: &mut ExpCtx, name: &str) -> bool {
    if name == "@" || name == "*" {
        return ctx.env.positional_count() > 0;
    }
    if matches!(name, "?" | "#" | "$" | "-") {
        return true;
    }
    if name == "!" {
        return ctx.env.last_async_pid().is_some();
    }
    if let Ok(index) = name.parse::<usize>() {
        return index == 0 || ctx.env.positional(index).is_some();
    }
    if let Some((base, _)) = array_all_ref(name) {
        let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
        return match ctx.env.kind(&target) {
            VarKind::Assoc => ctx.env.assoc_len(&target) > 0,
            VarKind::Indexed => ctx.env.array_len(&target) > 0,
            VarKind::Unset => false,
            _ => ctx.env.get(&target).is_some(),
        };
    }
    if ctx.env.attrs(name).contains(VarAttrs::NAMEREF)
        && ctx
            .env
            .nameref_target(name)
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        return false;
    }
    if let Some(bracket) = name.find('[') {
        let base = &name[..bracket];
        let close = name.rfind(']').unwrap_or(name.len());
        let subscript = &name[bracket + 1..close];
        let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
        if ctx.env.kind(&target) == VarKind::Assoc {
            let key = crate::expand_string_to_string_impl(subscript, ctx).unwrap_or_default();
            return ctx.env.get_array_assoc(&target, &key).is_some();
        }
        let idx = parse_subscript(ctx, subscript).unwrap_or(0);
        return ctx.env.get_array_indexed(&target, idx).is_some();
    }
    let target = nameref::resolve(ctx.env, name).unwrap_or_else(|| name.to_string());
    if target != name && target.contains('[') {
        return matches!(
            read_element_value_if_set(ctx, &target),
            Ok(ElementLookup::Set(_))
        );
    }
    match ctx.env.kind(&target) {
        VarKind::Assoc => ctx.env.get_array_assoc(&target, "0").is_some(),
        VarKind::Indexed => ctx.env.get_array_indexed(&target, 0).is_some(),
        VarKind::Unset => false,
        _ => ctx.env.get(&target).is_some(),
    }
}

fn parameter_has_transform_value(ctx: &mut ExpCtx, name: &str) -> bool {
    if name == "@" || name == "*" {
        return ctx.env.positional_count() > 0;
    }
    if matches!(name, "?" | "#" | "$" | "-") {
        return true;
    }
    if name == "!" {
        return ctx.env.last_async_pid().is_some();
    }
    if let Ok(index) = name.parse::<usize>() {
        return index == 0 || ctx.env.positional(index).is_some();
    }
    let base = array_all_ref(name).map(|(base, _)| base).unwrap_or(name);
    let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
    if target != base && target.contains('[') {
        return matches!(
            read_element_value_if_set(ctx, &target),
            Ok(ElementLookup::Set(_))
        );
    }
    match ctx.env.kind(&target) {
        VarKind::Assoc => ctx.env.assoc_len(&target) > 0,
        VarKind::Indexed => ctx.env.array_len(&target) > 0,
        VarKind::Unset => false,
        _ => ctx.env.get(&target).is_some(),
    }
}

fn handle_substring(
    name: &str,
    rest: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    // rest = "<offset>[ :<len>]"
    if rest.is_empty() {
        return Err(ExpandError::BadSubstitution(format!("{name}:")));
    }
    if name == "#" && rest == b"%" {
        return Err(ExpandError::Other(
            "#: %: arithmetic syntax error: operand expected (error token is \"%\")".into(),
        ));
    }
    let mut depth = 0;
    let mut ternary_depth = 0;
    let mut sep: Option<usize> = None;
    for (idx, b) in rest.iter().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'?' if depth == 0 => ternary_depth += 1,
            b':' if depth == 0 && ternary_depth > 0 => ternary_depth -= 1,
            b':' if depth == 0 => {
                sep = Some(idx);
                break;
            }
            _ => {}
        }
    }
    let (off_part, len_part) = match sep {
        Some(p) => (&rest[..p], Some(&rest[p + 1..])),
        None => (rest, None),
    };
    let off_str = std::str::from_utf8(off_part).unwrap_or("");
    let off_expanded = crate::expand_arith_string_to_string_impl(off_str, ctx)?;
    let off_val = if off_expanded.trim().is_empty() {
        0
    } else {
        eval_substring_arith(name, &off_expanded, ctx)?
    };
    let len_expr = len_part.map(|p| std::str::from_utf8(p).unwrap_or("").to_string());
    let len_val: Option<i64> = match len_expr.as_deref() {
        Some(p) => {
            let expanded = crate::expand_arith_string_to_string_impl(p, ctx)?;
            if expanded.trim().is_empty() {
                Some(0)
            } else {
                Some(eval_substring_arith(name, &expanded, ctx)?)
            }
        }
        None => None,
    };
    // Special case: $@ / $* substring.
    if name == "@" || name == "*" {
        let elems = positional_substring_elems(ctx, off_val, len_val, len_expr.as_deref())?;
        push_substring_array_values(
            out,
            &elems,
            quoted,
            name == "*",
            ctx.ifs.join_separator(),
            ctx.ifs.is_null,
            ctx.assign_in_progress,
            ctx.split_fields,
        );
        return Ok(());
    }
    if let Some((base, subscript)) = array_all_ref(name) {
        let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
        let kind = ctx.env.kind(&target);
        if kind == VarKind::Indexed {
            let elems = indexed_array_substring_elems(
                ctx.env.get_array_all(&target).unwrap_or_default(),
                off_val,
                len_val,
                len_expr.as_deref(),
            )?;
            push_substring_array_values(
                out,
                &elems,
                quoted,
                subscript == "*",
                ctx.ifs.join_separator(),
                ctx.ifs.is_null,
                ctx.assign_in_progress,
                ctx.split_fields,
            );
            return Ok(());
        }
        let elems = if kind == VarKind::Assoc {
            reject_negative_array_substring_len(len_val, len_expr.as_deref())?;
            ctx.env
                .assoc_all(&target)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, v)| v)
                .collect::<Vec<_>>()
        } else {
            if let Some(value) = ctx.env.get(&target) {
                push_scalar_substring(ctx.env, out, &value, off_val, len_val, quoted);
                return Ok(());
            }
            let elems = Vec::new();
            push_substring_array_values(
                out,
                &elems,
                quoted,
                subscript == "*",
                ctx.ifs.join_separator(),
                ctx.ifs.is_null,
                ctx.assign_in_progress,
                ctx.split_fields,
            );
            return Ok(());
        };
        reject_negative_array_substring_len(len_val, len_expr.as_deref())?;
        let total = elems.len() as i64;
        let start = if off_val < 0 {
            (total + off_val).max(0)
        } else {
            off_val.min(total)
        };
        let take = match len_val {
            Some(l) if l < 0 => (total - start + l).max(0),
            Some(l) => l,
            None => total - start,
        };
        let end = (start + take).min(total).max(start);
        push_substring_array_values(
            out,
            &elems[start as usize..end as usize],
            quoted,
            subscript == "*",
            ctx.ifs.join_separator(),
            ctx.ifs.is_null,
            ctx.assign_in_progress,
            ctx.split_fields,
        );
        return Ok(());
    }
    let value = read_scalar(ctx, name, false)?;
    push_scalar_substring(ctx.env, out, &value, off_val, len_val, quoted);
    Ok(())
}

fn push_scalar_substring(
    env: &dyn Environment,
    out: &mut ExpandBuf,
    value: &str,
    off_val: i64,
    len_val: Option<i64>,
    quoted: bool,
) {
    let value_bytes = crate::quote::shell_string_to_bytes(value);
    let value_text = std::str::from_utf8(&value_bytes).ok();
    let total = if locale_is_utf8(env) {
        value_text
            .map(|value| value.chars().count())
            .unwrap_or(value_bytes.len()) as i64
    } else {
        value_bytes.len() as i64
    };
    let mut start = if off_val < 0 {
        (total + off_val).max(0)
    } else {
        off_val.min(total)
    };
    if start < 0 {
        start = 0;
    }
    let end = match len_val {
        Some(l) if l < 0 => (total + l).max(start),
        Some(l) => (start + l).min(total),
        None => total,
    };
    let (start_byte, end_byte) = if locale_is_utf8(env) {
        value_text
            .map(|value| char_range_to_byte_range(value, start as usize, end as usize))
            .unwrap_or((start as usize, end as usize))
    } else {
        (start as usize, end as usize)
    };
    let slice = &value_bytes[start_byte..end_byte];
    push_value(out, slice, quoted);
}

fn char_range_to_byte_range(value: &str, start: usize, end: usize) -> (usize, usize) {
    let mut start_byte = value.len();
    let mut end_byte = value.len();
    for (idx, (byte, _)) in value.char_indices().enumerate() {
        if idx == start {
            start_byte = byte;
        }
        if idx == end {
            end_byte = byte;
            break;
        }
    }
    if start == value.chars().count() {
        start_byte = value.len();
    }
    if end == value.chars().count() {
        end_byte = value.len();
    }
    (start_byte, end_byte)
}

fn eval_substring_arith(name: &str, expr: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    arith::eval_preexpanded(expr, ctx).map_err(|err| match err {
        ExpandError::ArithSyntax(message) if is_bash_style_arith_syntax(&message) => {
            ExpandError::ArithSyntax(format!("{name}: {message}"))
        }
        other => other,
    })
}

fn is_bash_style_arith_syntax(message: &str) -> bool {
    message.contains(": syntax error") || message.contains(": arithmetic syntax error")
}

fn indexed_array_substring_elems(
    elems: Vec<(i64, String)>,
    off_val: i64,
    len_val: Option<i64>,
    len_expr: Option<&str>,
) -> Result<Vec<String>, ExpandError> {
    reject_negative_array_substring_len(len_val, len_expr)?;
    let Some(max_index) = elems.iter().map(|(idx, _)| *idx).max() else {
        return Ok(Vec::new());
    };
    let start_index = if off_val < 0 {
        (max_index + 1 + off_val).max(0)
    } else {
        off_val
    };
    let mut values = elems
        .into_iter()
        .filter(|(idx, _)| *idx >= start_index)
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let total = values.len() as i64;
    let take = match len_val {
        Some(len) if len < 0 => (total + len).max(0),
        Some(len) => len.max(0),
        None => total,
    };
    values.truncate(take as usize);
    Ok(values)
}

fn reject_negative_array_substring_len(
    len_val: Option<i64>,
    len_expr: Option<&str>,
) -> Result<(), ExpandError> {
    if matches!(len_val, Some(len) if len < 0) {
        let display = len_expr.unwrap_or_default();
        return Err(ExpandError::Other(format!(
            "{display}: substring expression < 0"
        )));
    }
    Ok(())
}

fn handle_remove(
    name: &str,
    pat_raw: &[u8],
    prefix: bool,
    longest: bool,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let mut opts = pattern_match_opts(ctx);
    opts.nocaseglob = false;
    if !quoted {
        if let Some(pattern) = raw_literal_pattern(pat_raw, opts) {
            if let Some(value) = read_plain_scalar_cow(ctx, name)? {
                if let Some(bytes) = plain_shell_string_bytes(value.as_ref()) {
                    push_literal_remove(out, bytes, pattern, prefix);
                    return Ok(());
                }
            }
        }
    }
    let pattern = expand_pattern(pat_raw, ctx)?;
    let value = read_value(ctx, name, quoted)?;
    let result = if literal_pattern(&pattern, opts).is_some() {
        let pattern = pattern.as_slice();
        transform_value(&value, |s| literal_remove(s, pattern, prefix))
    } else {
        transform_value(&value, |s| {
            pat_remove_with_opts(s, &pattern, prefix, longest, opts)
        })
    };
    push_value_buf(
        out,
        &result,
        quoted,
        ctx.ifs.join_separator(),
        ctx.ifs.is_null,
        ctx.assign_in_progress,
        ctx.split_fields,
    );
    Ok(())
}

fn handle_patsub(
    name: &str,
    rest: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    // rest format: [/|#|%]<pat>/<rep>  - or just <pat>/<rep> or <pat>
    let mut idx = 0;
    let mode_byte = if idx < rest.len() {
        let b = rest[idx];
        match b {
            b'/' => {
                idx += 1;
                Some(b'/')
            }
            b'#' => {
                idx += 1;
                Some(b'#')
            }
            b'%' => {
                idx += 1;
                Some(b'%')
            }
            _ => None,
        }
    } else {
        None
    };
    // Find unescaped / separator at top level.
    let mut sep_at: Option<usize> = None;
    let mut p = if mode_byte == Some(b'/') && rest.get(idx) == Some(&b'/') {
        idx + 1
    } else {
        idx
    };
    while p < rest.len() {
        match rest[p] {
            b'\\' if p + 1 < rest.len() => p += 2,
            b'[' => {
                if let Some(end) = pattern_bracket_expr_end(rest, p) {
                    p = end;
                } else {
                    p += 1;
                }
            }
            b'/' => {
                sep_at = Some(p);
                break;
            }
            _ => p += 1,
        }
    }
    let (pat_raw, rep_raw) = match sep_at {
        Some(s) => (&rest[idx..s], &rest[s + 1..]),
        None => (&rest[idx..], &b""[..]),
    };
    let mode = match mode_byte {
        Some(b'/') => PatSubMode::All,
        Some(b'#') => PatSubMode::PrefixAnchored,
        Some(b'%') => PatSubMode::SuffixAnchored,
        _ => PatSubMode::First,
    };
    let opts = pattern_match_opts(ctx);
    let patsub_replacement = ctx.env.option("patsub_replacement");
    if !quoted && !patsub_replacement {
        if let (Some(pattern), Some(replacement)) = (
            raw_literal_pattern(pat_raw, opts),
            raw_literal_replacement(rep_raw),
        ) {
            if let Some(value) = read_plain_scalar_cow(ctx, name)? {
                if let Some(bytes) = plain_shell_string_bytes(value.as_ref()) {
                    push_literal_substitute(out, bytes, pattern, replacement, mode);
                    return Ok(());
                }
            }
        }
    }
    let pattern = expand_pattern(pat_raw, ctx)?;
    let rep_str = std::str::from_utf8(rep_raw).unwrap_or("");
    let rep_expanded = crate::expand_word_string(rep_str, ctx, false)?;
    let value = read_value(ctx, name, quoted)?;
    let rep_template = rep_expanded.as_bytes().to_vec();
    let rep_bytes = if patsub_replacement {
        Vec::new()
    } else {
        crate::buf::dequote_bytes(&rep_template)
    };
    let result = transform_value(&value, |s| {
        if !patsub_replacement {
            if let Some(literal) = literal_pattern(&pattern, opts) {
                return literal_substitute(s, literal, &rep_bytes, mode);
            }
        }
        if patsub_replacement {
            pat_subst_with_replacer(s, &pattern, mode, opts, |matched| {
                expand_patsub_replacement(&rep_template, matched)
            })
        } else {
            pat_subst_with_opts(s, &pattern, &rep_bytes, mode, opts)
        }
    });
    push_value_buf(
        out,
        &result,
        quoted,
        ctx.ifs.join_separator(),
        ctx.ifs.is_null,
        ctx.assign_in_progress,
        ctx.split_fields,
    );
    Ok(())
}

fn literal_pattern(pattern: &[u8], opts: GlobOpts) -> Option<&[u8]> {
    if opts.nocaseglob || opts.extglob || pattern.is_empty() {
        return None;
    }
    let has_meta = pattern
        .iter()
        .any(|b| is_ctl(*b) || matches!(*b, b'*' | b'?' | b'[' | b'\\'));
    (!has_meta).then_some(pattern)
}

fn raw_literal_pattern(pattern: &[u8], opts: GlobOpts) -> Option<&[u8]> {
    if opts.nocaseglob || opts.extglob || pattern.is_empty() {
        return None;
    }
    raw_plain_word(pattern)
        .filter(|pat| !pat.iter().any(|b| matches!(*b, b'*' | b'?' | b'[' | b'\\')))
}

fn raw_literal_replacement(replacement: &[u8]) -> Option<&[u8]> {
    raw_plain_word(replacement)
}

fn raw_plain_word(word: &[u8]) -> Option<&[u8]> {
    let has_expansion_or_quote = word
        .iter()
        .any(|b| is_ctl(*b) || matches!(*b, b'$' | b'`' | b'\'' | b'"' | b'\\' | b'~'));
    (!has_expansion_or_quote).then_some(word)
}

fn literal_remove(value: &[u8], pattern: &[u8], prefix: bool) -> Vec<u8> {
    if prefix {
        value
            .strip_prefix(pattern)
            .map_or_else(|| value.to_vec(), |rest| rest.to_vec())
    } else {
        value
            .strip_suffix(pattern)
            .map_or_else(|| value.to_vec(), |rest| rest.to_vec())
    }
}

fn push_literal_remove(out: &mut ExpandBuf, value: &[u8], pattern: &[u8], prefix: bool) {
    out.push_raw(literal_remove_slice(value, pattern, prefix));
}

fn literal_remove_slice<'a>(value: &'a [u8], pattern: &[u8], prefix: bool) -> &'a [u8] {
    if prefix {
        value.strip_prefix(pattern).unwrap_or(value)
    } else {
        value.strip_suffix(pattern).unwrap_or(value)
    }
}

fn literal_substitute(
    value: &[u8],
    pattern: &[u8],
    replacement: &[u8],
    mode: PatSubMode,
) -> Vec<u8> {
    if pattern.is_empty() {
        return value.to_vec();
    }
    match mode {
        PatSubMode::PrefixAnchored => value.strip_prefix(pattern).map_or_else(
            || value.to_vec(),
            |rest| {
                let mut out = Vec::with_capacity(replacement.len() + rest.len());
                out.extend_from_slice(replacement);
                out.extend_from_slice(rest);
                out
            },
        ),
        PatSubMode::SuffixAnchored => value.strip_suffix(pattern).map_or_else(
            || value.to_vec(),
            |rest| {
                let mut out = Vec::with_capacity(rest.len() + replacement.len());
                out.extend_from_slice(rest);
                out.extend_from_slice(replacement);
                out
            },
        ),
        PatSubMode::First => {
            if let Some(pos) = find_bytes(value, pattern) {
                let mut out = Vec::with_capacity(value.len() - pattern.len() + replacement.len());
                out.extend_from_slice(&value[..pos]);
                out.extend_from_slice(replacement);
                out.extend_from_slice(&value[pos + pattern.len()..]);
                out
            } else {
                value.to_vec()
            }
        }
        PatSubMode::All => {
            let mut out = Vec::with_capacity(value.len());
            let mut rest = value;
            while let Some(pos) = find_bytes(rest, pattern) {
                out.extend_from_slice(&rest[..pos]);
                out.extend_from_slice(replacement);
                rest = &rest[pos + pattern.len()..];
            }
            out.extend_from_slice(rest);
            out
        }
    }
}

fn push_literal_substitute(
    out: &mut ExpandBuf,
    value: &[u8],
    pattern: &[u8],
    replacement: &[u8],
    mode: PatSubMode,
) {
    if pattern.is_empty() {
        out.push_raw(value);
        return;
    }
    match mode {
        PatSubMode::PrefixAnchored => {
            if let Some(rest) = value.strip_prefix(pattern) {
                out.push_raw(replacement);
                out.push_raw(rest);
            } else {
                out.push_raw(value);
            }
        }
        PatSubMode::SuffixAnchored => {
            if let Some(rest) = value.strip_suffix(pattern) {
                out.push_raw(rest);
                out.push_raw(replacement);
            } else {
                out.push_raw(value);
            }
        }
        PatSubMode::First => {
            if let Some(pos) = find_bytes(value, pattern) {
                out.push_raw(&value[..pos]);
                out.push_raw(replacement);
                out.push_raw(&value[pos + pattern.len()..]);
            } else {
                out.push_raw(value);
            }
        }
        PatSubMode::All => {
            let mut rest = value;
            while let Some(pos) = find_bytes(rest, pattern) {
                out.push_raw(&rest[..pos]);
                out.push_raw(replacement);
                rest = &rest[pos + pattern.len()..];
            }
            out.push_raw(rest);
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let (&first, rest) = needle.split_first()?;
    let last_start = haystack.len().checked_sub(needle.len())?;
    let mut pos = 0;
    while pos <= last_start {
        let rel = haystack[pos..=last_start]
            .iter()
            .position(|b| *b == first)?;
        pos += rel;
        if haystack[pos + 1..].starts_with(rest) {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

fn pattern_bracket_expr_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut j = start + 1;
    if j >= bytes.len() {
        return None;
    }
    if matches!(bytes[j], b'!' | b'^') {
        j += 1;
    }
    if j < bytes.len() && bytes[j] == b']' {
        j += 1;
    }
    'outer: while j < bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            j += 2;
            continue;
        }
        if bytes[j] == b'[' && j + 1 < bytes.len() && matches!(bytes[j + 1], b':' | b'.' | b'=') {
            let term = bytes[j + 1];
            let mut k = j + 2;
            while k + 1 < bytes.len() {
                if bytes[k] == term && bytes[k + 1] == b']' {
                    j = k + 2;
                    continue 'outer;
                }
                k += 1;
            }
        }
        if bytes[j] == b']' && j > start + 1 {
            return Some(j + 1);
        }
        j += 1;
    }
    None
}

fn expand_patsub_replacement(template: &[u8], matched: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(template.len() + matched.len());
    let mut i = 0;
    while i < template.len() {
        match template[i] {
            CTLESC if i + 2 < template.len() && template[i + 1] == CTLRAW => {
                out.push(template[i + 2]);
                i += 3;
            }
            CTLESC if i + 1 < template.len() => {
                out.push(template[i + 1]);
                i += 2;
            }
            CTLNUL | CTLFIELD => {
                i += 1;
            }
            b'\\' => {
                let start = i;
                while i < template.len() && template[i] == b'\\' {
                    i += 1;
                }
                if i < template.len() && template[i] == b'&' {
                    let count = i - start;
                    out.extend(std::iter::repeat(b'\\').take(count / 2));
                    if count % 2 == 0 {
                        out.extend_from_slice(matched);
                    } else {
                        out.push(b'&');
                    }
                    i += 1;
                } else {
                    out.extend_from_slice(&template[start..i]);
                }
            }
            b'&' => {
                out.extend_from_slice(matched);
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

fn handle_casemod(
    name: &str,
    pat_raw: &[u8],
    mode: CaseModMode,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let pat_opt = if pat_raw.is_empty() {
        None
    } else {
        Some(expand_pattern(pat_raw, ctx)?)
    };
    let value = read_value(ctx, name, quoted)?;
    let mut opts = pattern_match_opts(ctx);
    opts.nocaseglob = false;
    let result = transform_value(&value, |s| {
        pat_casemod_with_opts(s, pat_opt.as_deref(), mode, opts)
    });
    push_value_buf(
        out,
        &result,
        quoted,
        ctx.ifs.join_separator(),
        ctx.ifs.is_null,
        ctx.assign_in_progress,
        ctx.split_fields,
    );
    Ok(())
}

fn pattern_match_opts(ctx: &ExpCtx) -> GlobOpts {
    pattern_match_opts_env(ctx.env)
}

fn pattern_match_opts_env(env: &dyn Environment) -> GlobOpts {
    GlobOpts {
        nocaseglob: env.option("nocasematch"),
        extglob: env.option("extglob"),
        ..Default::default()
    }
}

fn handle_transform(
    name: &str,
    rest: &[u8],
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    if rest.is_empty() {
        return Err(ExpandError::BadSubstitution(format!("${{{}@}}", name)));
    }
    let kind = rest[0];
    if !matches!(
        kind,
        b'Q' | b'E' | b'P' | b'A' | b'a' | b'K' | b'k' | b'U' | b'L' | b'u'
    ) {
        return Err(ExpandError::BadSubstitution(format!(
            "${{{}@{}}}",
            name, kind as char
        )));
    }
    if matches!(kind, b'Q' | b'E' | b'P') {
        match read_element_value_if_set(ctx, name)? {
            ElementLookup::Set(value) => {
                let result = match kind {
                    b'Q' => transform_string_value(&value, shell_quote_shell_string),
                    b'E' => transform_value(&value, quote::ansi_c_decode),
                    b'P' => transform_value_result(&value, |s| {
                        prompt_expand(&String::from_utf8_lossy(s), ctx).map(str_to_bytes)
                    })?,
                    _ => unreachable!(),
                };
                push_value_buf(
                    out,
                    &result,
                    quoted,
                    ctx.ifs.join_separator(),
                    ctx.ifs.is_null,
                    ctx.assign_in_progress,
                    ctx.split_fields,
                );
                return Ok(());
            }
            ElementLookup::Unset => return Ok(()),
            ElementLookup::NotElement => {
                if !parameter_is_set(ctx, name) {
                    return Ok(());
                }
            }
        }
    }
    if matches!(kind, b'a' | b'A')
        && ctx.eval_unbound_error
        && !parameter_has_transform_value(ctx, name)
    {
        return Err(ExpandError::UnboundVariable(name.to_string()));
    }
    if kind == b'a' {
        let attr_name = array_all_ref(name).map(|(base, _)| base).unwrap_or(name);
        let attr_name =
            nameref::resolve(ctx.env, attr_name).unwrap_or_else(|| attr_name.to_string());
        let bytes = attribute_letters(ctx.env.attrs(&attr_name)).into_bytes();
        push_value(out, &bytes, quoted);
        return Ok(());
    }
    if kind == b'A' {
        return handle_transform_declare(name, ctx, quoted, out);
    }
    if matches!(kind, b'K' | b'k') {
        if let Some((base, subscript)) = array_all_ref(name) {
            let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
            let pairs = match ctx.env.kind(&target) {
                VarKind::Assoc => ctx.env.assoc_all(&target).unwrap_or_default(),
                VarKind::Indexed => ctx
                    .env
                    .get_array_all(&target)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(idx, value)| (idx.to_string(), value))
                    .collect(),
                _ => Vec::new(),
            };
            if kind == b'k' {
                let elems = pairs
                    .into_iter()
                    .flat_map(|(key, value)| [key, value])
                    .collect::<Vec<_>>();
                push_array_values(
                    out,
                    &elems,
                    quoted,
                    subscript == "*",
                    ctx.ifs.join_separator(),
                    ctx.ifs.is_null,
                    ctx.assign_in_progress,
                    ctx.split_fields,
                );
            } else {
                let trailing_space = ctx.env.kind(&target) == VarKind::Assoc;
                let mut chunks = Vec::new();
                for (key, value) in pairs {
                    chunks.push(format!(
                        "{} {}",
                        key_value_transform_key_quote(&key),
                        double_quote(&value)
                    ));
                }
                let mut rendered = chunks.join(" ");
                if trailing_space && !rendered.is_empty() {
                    rendered.push(' ');
                }
                push_value(out, rendered.as_bytes(), quoted);
            }
            return Ok(());
        }
    }
    if matches!(kind, b'K' | b'k') {
        let target = nameref::resolve(ctx.env, name).unwrap_or_else(|| name.to_string());
        if ctx.env.kind(&target) == VarKind::Assoc {
            return Ok(());
        }
        let value = read_value(ctx, name, quoted)?;
        let result = transform_value(&value, quote::shell_quote);
        push_value_buf(
            out,
            &result,
            quoted,
            ctx.ifs.join_separator(),
            ctx.ifs.is_null,
            ctx.assign_in_progress,
            ctx.split_fields,
        );
        return Ok(());
    }
    if matches!(kind, b'Q' | b'E' | b'P') {
        let value = read_value(ctx, name, quoted)?;
        let result = match kind {
            b'Q' => transform_string_value(&value, shell_quote_shell_string),
            b'E' => transform_value(&value, quote::ansi_c_decode),
            b'P' => transform_value_result(&value, |s| {
                prompt_expand(&String::from_utf8_lossy(s), ctx).map(str_to_bytes)
            })?,
            _ => unreachable!(),
        };
        push_value_buf(
            out,
            &result,
            quoted,
            ctx.ifs.join_separator(),
            ctx.ifs.is_null,
            ctx.assign_in_progress,
            ctx.split_fields,
        );
        return Ok(());
    }
    let bytes = match kind {
        b'U' => read_scalar(ctx, name, true)?.to_uppercase().into_bytes(),
        b'L' => read_scalar(ctx, name, true)?.to_lowercase().into_bytes(),
        b'u' => {
            let mut s = read_scalar(ctx, name, true)?;
            if let Some(c) = s.chars().next() {
                let upper = c.to_ascii_uppercase();
                s.replace_range(0..c.len_utf8(), &upper.to_string());
            }
            s.into_bytes()
        }
        _ => return Err(ExpandError::BadSubstitution(format!("@{}", kind as char))),
    };
    push_value(out, &bytes, quoted);
    Ok(())
}

fn handle_transform_declare(
    name: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let rendered = if name == "@" || name == "*" {
        let elems = positionals(ctx);
        let values = elems
            .iter()
            .map(|s| shell_quote_shell_string(s))
            .collect::<Vec<_>>()
            .join(" ");
        if values.is_empty() {
            "set --".to_string()
        } else {
            format!("set -- {values}")
        }
    } else if let Some((base, _)) = array_all_ref(name) {
        format_array_declaration(ctx, base)
    } else {
        format_scalar_declaration(ctx, name)
    };
    push_value(out, rendered.as_bytes(), quoted);
    Ok(())
}

fn format_scalar_declaration(ctx: &mut ExpCtx, name: &str) -> String {
    if name.contains('[') {
        if let Ok(ElementLookup::Set(ValueRepr::Scalar(value))) =
            read_element_value_if_set(ctx, name)
        {
            let base = name.split_once('[').map(|(base, _)| base).unwrap_or(name);
            let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
            let flags = attribute_letters(ctx.env.attrs(&target));
            let quoted = shell_quote_shell_string(&value);
            return if flags.is_empty() {
                format!("{target}={quoted}")
            } else {
                format!("declare -{flags} {target}={quoted}")
            };
        }
        return String::new();
    }
    let target = nameref::resolve(ctx.env, name).unwrap_or_else(|| name.to_string());
    let attrs = ctx.env.attrs(&target);
    let value = ctx.env.get(&target);
    if value.is_none() && attrs.is_empty() {
        return String::new();
    }
    let flags = attribute_letters(attrs);
    if value.is_none() {
        return if flags.is_empty() {
            String::new()
        } else {
            format!("declare -{flags} {target}")
        };
    }
    let quoted = shell_quote_shell_string(&value.unwrap_or_default());
    if flags.is_empty() {
        format!("{target}={quoted}")
    } else {
        format!("declare -{flags} {target}={quoted}")
    }
}

fn format_array_declaration(ctx: &mut ExpCtx, base: &str) -> String {
    let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
    match ctx.env.kind(&target) {
        VarKind::Indexed => {
            let flags = attribute_letters(ctx.env.attrs(&target));
            let flags = if flags.is_empty() {
                "a".to_string()
            } else {
                flags
            };
            let Some(items) = ctx.env.get_array_all(&target) else {
                return format!("declare -{flags} {target}");
            };
            let body = items
                .iter()
                .map(|(idx, value)| format!("[{idx}]={}", array_element_quote(value)))
                .collect::<Vec<_>>()
                .join(" ");
            if body.is_empty() {
                format!("declare -{flags} {target}=()")
            } else {
                format!("declare -{flags} {target}=({body})")
            }
        }
        VarKind::Assoc => {
            let flags = attribute_letters(ctx.env.attrs(&target));
            let flags = if flags.is_empty() {
                "A".to_string()
            } else {
                flags
            };
            let Some(items) = ctx.env.assoc_all(&target) else {
                return format!("declare -{flags} {target}");
            };
            let body = items
                .iter()
                .map(|(key, value)| {
                    format!("[{}]={}", assoc_key_quote(key), array_element_quote(value))
                })
                .collect::<Vec<_>>()
                .join(" ");
            if body.is_empty() {
                format!("declare -{flags} {target}=()")
            } else {
                format!("declare -{flags} {target}=({body} )")
            }
        }
        _ => format_scalar_declaration(ctx, &target),
    }
}

fn shell_quote_shell_string(value: &str) -> String {
    let bytes = quote::shell_string_to_bytes(value);
    if shell_string_needs_ansi_quote(value, &bytes) {
        return ansi_quote_bytes(&bytes);
    }
    String::from_utf8_lossy(&quote::shell_quote(value.as_bytes())).into_owned()
}

fn array_element_quote(value: &str) -> String {
    let bytes = quote::shell_string_to_bytes(value);
    if shell_string_needs_ansi_quote(value, &bytes) {
        ansi_quote_bytes(&bytes)
    } else {
        double_quote(value)
    }
}

fn shell_string_needs_ansi_quote(value: &str, bytes: &[u8]) -> bool {
    bytes != value.as_bytes() || bytes.iter().any(|b| *b < 0x20 || *b == 0x7f)
}

fn ansi_quote_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("$'");
    for &b in bytes {
        match b {
            b'\'' => out.push_str("\\'"),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x0b => out.push_str("\\v"),
            0x0c => out.push_str("\\f"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('\'');
    out
}

fn double_quote(value: &str) -> String {
    format!("\"{}\"", escape_double(value))
}

fn assoc_key_quote(value: &str) -> String {
    let safe = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | '-' | '+' | ':' | ','));
    if safe {
        value.to_string()
    } else if {
        let bytes = quote::shell_string_to_bytes(value);
        shell_string_needs_ansi_quote(value, &bytes)
    } {
        shell_quote_shell_string(value)
    } else {
        double_quote(value)
    }
}

fn key_value_transform_key_quote(value: &str) -> String {
    let safe = value.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | '-' | '+' | ':' | ',' | '=')
    });
    if safe {
        value.to_string()
    } else {
        array_element_quote(value)
    }
}

const PROMPT_ESCAPED_DOLLAR: char = '\u{e000}';
const PROMPT_LITERAL_BACKSLASH: char = '\u{e001}';
const RL_PROMPT_START_IGNORE: char = '\x01';
const RL_PROMPT_END_IGNORE: char = '\x02';

fn prompt_expand(value: &str, ctx: &mut ExpCtx) -> Result<String, ExpandError> {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            if ctx.env.option("posix") && bytes[i] == b'!' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'!' {
                    out.push('!');
                    i += 2;
                } else {
                    out.push_str(&ctx.env.prompt_history_number().to_string());
                    i += 1;
                }
                continue;
            }
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            out.push(PROMPT_LITERAL_BACKSLASH);
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            b'0'..=b'7' => {
                let mut n = 0u32;
                let mut digits = 0;
                let mut j = i + 1;
                while digits < 3 && j < bytes.len() && (b'0'..=b'7').contains(&bytes[j]) {
                    n = n * 8 + (bytes[j] - b'0') as u32;
                    j += 1;
                    digits += 1;
                }
                if n <= 0xff {
                    out.push(n as u8 as char);
                }
                i = j;
            }
            b'[' => {
                if ctx.env.prompt_nonprinting_markers() {
                    out.push(RL_PROMPT_START_IGNORE);
                }
                i += 2;
            }
            b']' => {
                if ctx.env.prompt_nonprinting_markers() {
                    out.push(RL_PROMPT_END_IGNORE);
                }
                i += 2;
            }
            b'a' => {
                out.push('\x07');
                i += 2;
            }
            b'e' | b'E' => {
                out.push('\x1b');
                i += 2;
            }
            b'r' => {
                out.push('\r');
                i += 2;
            }
            b't' => {
                out.push_str(&strftime_now("%H:%M:%S"));
                i += 2;
            }
            b'T' => {
                out.push_str(&strftime_now("%I:%M:%S"));
                i += 2;
            }
            b'@' => {
                out.push_str(&strftime_now("%I:%M %p"));
                i += 2;
            }
            b'A' => {
                out.push_str(&strftime_now("%H:%M"));
                i += 2;
            }
            b'd' => {
                out.push_str(&strftime_now("%a %b %d"));
                i += 2;
            }
            b'D' => {
                if i + 2 < bytes.len() && bytes[i + 2] == b'{' {
                    let start = i + 3;
                    let end = (start..bytes.len())
                        .find(|&j| bytes[j] == b'}')
                        .unwrap_or(bytes.len());
                    let fmt = std::str::from_utf8(&bytes[start..end]).unwrap_or("%X");
                    out.push_str(&strftime_now(if fmt.is_empty() { "%X" } else { fmt }));
                    i = if end < bytes.len() { end + 1 } else { end };
                } else {
                    out.push(PROMPT_LITERAL_BACKSLASH);
                    out.push('D');
                    i += 2;
                }
            }
            b'h' => {
                let host = current_host_name(ctx.env);
                out.push_str(host.split('.').next().unwrap_or(&host));
                i += 2;
            }
            b'H' => {
                out.push_str(&current_host_name(ctx.env));
                i += 2;
            }
            b's' => {
                let shell = ctx
                    .env
                    .prompt_shell_name()
                    .or_else(|| ctx.env.positional(0))
                    .unwrap_or_else(|| "cherubsh".to_string());
                out.push_str(&base_name(&shell));
                i += 2;
            }
            b'u' => {
                out.push_str(&current_user_name());
                i += 2;
            }
            b'w' => {
                out.push_str(&render_pwd(ctx.env, false));
                i += 2;
            }
            b'W' => {
                out.push_str(&render_pwd(ctx.env, true));
                i += 2;
            }
            b'v' => {
                out.push_str(&bash_version(ctx.env, false));
                i += 2;
            }
            b'V' => {
                out.push_str(&bash_version(ctx.env, true));
                i += 2;
            }
            b'$' => {
                out.push(PROMPT_ESCAPED_DOLLAR);
                i += 2;
            }
            b'\\' => {
                out.push(PROMPT_LITERAL_BACKSLASH);
                i += 2;
            }
            b'n' => {
                out.push('\n');
                i += 2;
            }
            b'#' => {
                out.push_str(&ctx.env.prompt_command_number().to_string());
                i += 2;
            }
            b'!' => {
                out.push_str(&ctx.env.prompt_history_number().to_string());
                i += 2;
            }
            b'j' => {
                out.push_str(&ctx.env.prompt_job_count().to_string());
                i += 2;
            }
            other => {
                out.push(PROMPT_LITERAL_BACKSLASH);
                out.push(other as char);
                i += 2;
            }
        }
    }
    if ctx.env.option("promptvars") {
        let expanded = crate::expand_string_to_string_impl(&out, ctx)?;
        Ok(restore_escaped_prompt_dollars(&expanded))
    } else {
        Ok(restore_escaped_prompt_dollars(&out))
    }
}

fn str_to_bytes(value: String) -> Vec<u8> {
    value.into_bytes()
}

fn restore_escaped_prompt_dollars(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            PROMPT_ESCAPED_DOLLAR => '$',
            PROMPT_LITERAL_BACKSLASH => '\\',
            other => other,
        })
        .collect()
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn current_user_name() -> String {
    if let Some(name) = std::env::var_os("USER") {
        if let Some(s) = name.to_str() {
            return s.to_string();
        }
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() {
            let cstr = CStr::from_ptr((*pw).pw_name);
            if let Ok(s) = cstr.to_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn current_host_name(env: &dyn Environment) -> String {
    if let Some(host) = env.get("HOSTNAME").filter(|s| !s.is_empty()) {
        return host;
    }
    let mut buf = [0u8; 256];
    let result = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if result != 0 {
        return String::new();
    }
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

fn render_pwd(env: &dyn Environment, basename_only: bool) -> String {
    let pwd = env
        .get("PWD")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| ".".to_string());
    if basename_only {
        if pwd == "/" {
            return "/".to_string();
        }
        if env.get("HOME").as_deref() == Some(pwd.as_str()) {
            return "~".to_string();
        }
        return base_name(&pwd);
    }
    let home = env.get("HOME").unwrap_or_default();
    if !home.is_empty() && pwd.starts_with(&home) {
        let mut tilde = String::from("~");
        tilde.push_str(&pwd[home.len()..]);
        return tilde;
    }
    pwd
}

fn bash_version(env: &dyn Environment, release: bool) -> String {
    let raw = env
        .get("BASH_VERSION")
        .or_else(|| std::env::var("CHERUBSH_BASH_COMPAT_VERSION").ok())
        .unwrap_or_else(|| "5.3.0".to_string());
    let numeric = raw
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|s| !s.is_empty())
        .unwrap_or("5.3.0");
    let mut parts = numeric.split('.');
    let major = parts.next().unwrap_or("5");
    let minor = parts.next().unwrap_or("3");
    let patch = parts.next().unwrap_or("0");
    if release {
        format!("{major}.{minor}.{patch}")
    } else {
        format!("{major}.{minor}")
    }
}

fn strftime_now(fmt: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    unsafe {
        let tm = libc::localtime(&secs as *const libc::time_t);
        if tm.is_null() {
            return String::new();
        }
        let c_fmt = match std::ffi::CString::new(fmt) {
            Ok(value) => value,
            Err(_) => return String::new(),
        };
        let mut buf = [0u8; 256];
        let written = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c_fmt.as_ptr(),
            tm,
        );
        if written == 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..written]).into_owned()
    }
}

fn attribute_letters(attrs: VarAttrs) -> String {
    let mut s = String::new();
    if attrs.contains(VarAttrs::ARRAY) {
        s.push('a');
    }
    if attrs.contains(VarAttrs::ASSOC) {
        s.push('A');
    }
    if attrs.contains(VarAttrs::INTEGER) {
        s.push('i');
    }
    if attrs.contains(VarAttrs::NAMEREF) {
        s.push('n');
    }
    if attrs.contains(VarAttrs::READONLY) {
        s.push('r');
    }
    if attrs.contains(VarAttrs::EXPORT) {
        s.push('x');
    }
    if attrs.contains(VarAttrs::UPPERCASE) {
        s.push('u');
    }
    if attrs.contains(VarAttrs::LOWERCASE) {
        s.push('l');
    }
    if attrs.contains(VarAttrs::TRACE) {
        s.push('t');
    }
    s
}

fn report_circular_name_reference(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: warning: {name}: circular name reference");
    } else {
        eprintln!("cherubsh: warning: {name}: circular name reference");
    }
}

fn escape_double(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Read a variable as a scalar (joins arrays via IFS[0]). `length_mode` is
/// true when `${#var}` semantics apply.
fn read_scalar(ctx: &mut ExpCtx, name: &str, length_mode: bool) -> Result<String, ExpandError> {
    // Special parameters
    if name == "!" && ctx.eval_unbound_error && ctx.env.last_async_pid().is_none() {
        return Err(ExpandError::UnboundVariable("!".into()));
    }
    if let Some(s) = special_param(name, ctx) {
        return Ok(s);
    }
    if let Ok(n) = name.parse::<usize>() {
        if n != 0 && ctx.eval_unbound_error && ctx.env.positional(n).is_none() {
            return Err(ExpandError::UnboundVariable(name.into()));
        }
        return Ok(ctx.env.positional(n).unwrap_or_default());
    }
    // Subscripted access
    if let Some(bracket) = name.find('[') {
        let base = &name[..bracket];
        let close = name.rfind(']').unwrap_or(name.len());
        let subscript = &name[bracket + 1..close];
        if subscript == "@" || subscript == "*" {
            let sep = ctx.ifs.join_separator().unwrap_or(b' ');
            return Ok(join_array(ctx, base, sep));
        }
        let target = match nameref::resolve(ctx.env, base) {
            Some(target) => target,
            None if ctx.env.attrs(base).contains(VarAttrs::NAMEREF) => {
                report_circular_name_reference(ctx.env, base);
                return Ok(String::new());
            }
            None => base.to_string(),
        };
        if ctx.env.kind(&target) == VarKind::Assoc {
            let key = crate::expand_string_to_string_impl(subscript, ctx)?;
            let value = ctx.env.get_array_assoc(&target, &key);
            if ctx.eval_unbound_error && value.is_none() {
                return Err(ExpandError::UnboundVariable(name.to_string()));
            }
            return Ok(value.unwrap_or_default());
        }
        let idx = match parse_indexed_subscript(ctx, &target, subscript, &target) {
            Ok(idx) => idx,
            Err(ExpandError::InvalidArraySubscript(label)) => {
                report_array_subscript_error(ctx.env, &label);
                return Ok(String::new());
            }
            Err(err) => return Err(err),
        };
        if ctx.env.kind(&target) != VarKind::Indexed {
            return Ok(if idx == 0 {
                ctx.env.get(&target).unwrap_or_default()
            } else {
                if ctx.eval_unbound_error {
                    return Err(ExpandError::UnboundVariable(name.to_string()));
                }
                String::new()
            });
        }
        let value = ctx.env.get_array_indexed(&target, idx);
        if ctx.eval_unbound_error && value.is_none() {
            return Err(ExpandError::UnboundVariable(name.to_string()));
        }
        return Ok(value.unwrap_or_default());
    }
    let target = match nameref::resolve(ctx.env, name) {
        Some(target) => target,
        None if ctx.env.attrs(name).contains(VarAttrs::NAMEREF) => {
            report_circular_name_reference(ctx.env, name);
            return Ok(String::new());
        }
        None => name.to_string(),
    };
    if target != name && target.contains('[') {
        let saved = ctx.eval_unbound_error;
        if !length_mode && !saved {
            ctx.eval_unbound_error = false;
        }
        let result = read_scalar(ctx, &target, length_mode);
        ctx.eval_unbound_error = saved;
        return result.map_err(|err| map_nameref_unbound(err, &target, name));
    }
    let kind = ctx.env.kind(&target);
    match kind {
        VarKind::Indexed => {
            let value = ctx
                .env
                .get_array_indexed(&target, 0)
                .or_else(|| ctx.env.get(&target));
            if ctx.eval_unbound_error && value.is_none() {
                return Err(ExpandError::UnboundVariable(
                    nameref_unbound_name(&target, name).to_string(),
                ));
            }
            Ok(value.unwrap_or_default())
        }
        VarKind::Assoc => {
            let value = ctx.env.get_array_assoc(&target, "0");
            if ctx.eval_unbound_error && value.is_none() {
                return Err(ExpandError::UnboundVariable(
                    nameref_unbound_name(&target, name).to_string(),
                ));
            }
            Ok(value.unwrap_or_default())
        }
        _ => {
            if ctx.eval_unbound_error && ctx.env.get(&target).is_none() {
                return Err(ExpandError::UnboundVariable(
                    nameref_unbound_name(&target, name).to_string(),
                ));
            }
            Ok(ctx.env.get(&target).unwrap_or_default())
        }
    }
}

fn nameref_unbound_name<'a>(target: &'a str, source: &'a str) -> &'a str {
    if target != source {
        source
    } else {
        target
    }
}

fn map_nameref_unbound(err: ExpandError, target: &str, source: &str) -> ExpandError {
    match err {
        ExpandError::UnboundVariable(name) if name == target => {
            ExpandError::UnboundVariable(source.to_string())
        }
        other => other,
    }
}

fn read_plain_scalar_cow<'a>(
    ctx: &'a mut ExpCtx,
    name: &str,
) -> Result<Option<Cow<'a, str>>, ExpandError> {
    read_plain_scalar_cow_env(ctx.env, ctx.eval_unbound_error, name)
}

fn read_plain_scalar_cow_env<'a>(
    env: &'a dyn Environment,
    eval_unbound_error: bool,
    name: &str,
) -> Result<Option<Cow<'a, str>>, ExpandError> {
    let Some(first) = name.as_bytes().first().copied() else {
        return Ok(None);
    };
    if !(first == b'_' || first.is_ascii_alphabetic())
        || !name
            .as_bytes()
            .iter()
            .all(|b| *b == b'_' || b.is_ascii_alphanumeric())
    {
        return Ok(None);
    }
    if env.attrs(name).contains(VarAttrs::NAMEREF) {
        return Ok(None);
    }
    let value = env.get_cow(name);
    if eval_unbound_error && value.is_none() {
        return Err(ExpandError::UnboundVariable(name.to_string()));
    }
    Ok(Some(value.unwrap_or(Cow::Borrowed(""))))
}

fn plain_shell_string_bytes(value: &str) -> Option<&[u8]> {
    if !value.is_ascii() || value.as_bytes().iter().any(|b| is_ctl(*b)) {
        return None;
    }
    Some(value.as_bytes())
}

fn report_array_subscript_error(env: &dyn cherubsh_common::Environment, label: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {label}: bad array subscript");
    } else {
        eprintln!("cherubsh: {label}: bad array subscript");
    }
}

/// Read a variable preserving array-vs-scalar identity. Used by operator
/// handlers that need to iterate elements (${arr[@]/p/r}).
fn read_value(ctx: &mut ExpCtx, name: &str, _quoted: bool) -> Result<ValueRepr, ExpandError> {
    if name == "@" {
        return Ok(ValueRepr::Array {
            elems: positionals(ctx),
            star: false,
        });
    }
    if name == "*" {
        return Ok(ValueRepr::Array {
            elems: positionals(ctx),
            star: true,
        });
    }
    if name == "!" && ctx.eval_unbound_error && ctx.env.last_async_pid().is_none() {
        return Err(ExpandError::UnboundVariable("!".into()));
    }
    if let Some(s) = special_param(name, ctx) {
        return Ok(ValueRepr::Scalar(s));
    }
    if let Ok(n) = name.parse::<usize>() {
        if n != 0 && ctx.eval_unbound_error && ctx.env.positional(n).is_none() {
            return Err(ExpandError::UnboundVariable(name.into()));
        }
        return Ok(ValueRepr::Scalar(ctx.env.positional(n).unwrap_or_default()));
    }
    if let Some(bracket) = name.find('[') {
        let base = &name[..bracket];
        let close = name.rfind(']').unwrap_or(name.len());
        let subscript = &name[bracket + 1..close];
        if subscript == "@" || subscript == "*" {
            let target = match nameref::resolve(ctx.env, base) {
                Some(target) => target,
                None if ctx.env.attrs(base).contains(VarAttrs::NAMEREF) => {
                    report_circular_name_reference(ctx.env, base);
                    return Ok(ValueRepr::Array {
                        elems: Vec::new(),
                        star: subscript == "*",
                    });
                }
                None => base.to_string(),
            };
            match ctx.env.kind(&target) {
                VarKind::Indexed => {
                    let elems = ctx
                        .env
                        .get_array_all(&target)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, v)| v)
                        .collect();
                    return Ok(ValueRepr::Array {
                        elems,
                        star: subscript == "*",
                    });
                }
                VarKind::Assoc => {
                    let elems = ctx
                        .env
                        .assoc_all(&target)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(_, v)| v)
                        .collect();
                    return Ok(ValueRepr::Array {
                        elems,
                        star: subscript == "*",
                    });
                }
                _ => {
                    if let Some(value) = ctx.env.get(&target) {
                        return Ok(ValueRepr::Scalar(value));
                    }
                    return Ok(ValueRepr::Array {
                        elems: Vec::new(),
                        star: subscript == "*",
                    });
                }
            }
        }
        let target = match nameref::resolve(ctx.env, base) {
            Some(target) => target,
            None if ctx.env.attrs(base).contains(VarAttrs::NAMEREF) => {
                report_circular_name_reference(ctx.env, base);
                return Ok(ValueRepr::Scalar(String::new()));
            }
            None => base.to_string(),
        };
        if ctx.env.kind(&target) == VarKind::Assoc {
            let key = crate::expand_string_to_string_impl(subscript, ctx)?;
            let value = ctx.env.get_array_assoc(&target, &key);
            if ctx.eval_unbound_error && value.is_none() {
                return Err(ExpandError::UnboundVariable(name.to_string()));
            }
            return Ok(ValueRepr::Scalar(value.unwrap_or_default()));
        }
        let idx = match parse_indexed_subscript(ctx, &target, subscript, &target) {
            Ok(idx) => idx,
            Err(ExpandError::InvalidArraySubscript(label)) => {
                report_array_subscript_error(ctx.env, &label);
                return Ok(ValueRepr::Scalar(String::new()));
            }
            Err(err) => return Err(err),
        };
        if ctx.env.kind(&target) != VarKind::Indexed {
            return Ok(ValueRepr::Scalar(if idx == 0 {
                ctx.env.get(&target).unwrap_or_default()
            } else {
                if ctx.eval_unbound_error {
                    return Err(ExpandError::UnboundVariable(name.to_string()));
                }
                String::new()
            }));
        }
        let value = ctx.env.get_array_indexed(&target, idx);
        if ctx.eval_unbound_error && value.is_none() {
            return Err(ExpandError::UnboundVariable(name.to_string()));
        }
        return Ok(ValueRepr::Scalar(value.unwrap_or_default()));
    }
    let target = match nameref::resolve(ctx.env, name) {
        Some(target) => target,
        None if ctx.env.attrs(name).contains(VarAttrs::NAMEREF) => {
            report_circular_name_reference(ctx.env, name);
            return Ok(ValueRepr::Scalar(String::new()));
        }
        None => name.to_string(),
    };
    if target != name && target.contains('[') {
        return read_value(ctx, &target, false);
    }
    match ctx.env.kind(&target) {
        VarKind::Indexed => {
            let value = ctx
                .env
                .get_array_indexed(&target, 0)
                .or_else(|| ctx.env.get(&target));
            if ctx.eval_unbound_error && value.is_none() {
                return Err(ExpandError::UnboundVariable(target));
            }
            Ok(ValueRepr::Scalar(value.unwrap_or_default()))
        }
        VarKind::Assoc => {
            let value = ctx.env.get_array_assoc(&target, "0");
            if ctx.eval_unbound_error && value.is_none() {
                return Err(ExpandError::UnboundVariable(target));
            }
            Ok(ValueRepr::Scalar(value.unwrap_or_default()))
        }
        _ => {
            if ctx.eval_unbound_error && ctx.env.get(&target).is_none() {
                return Err(ExpandError::UnboundVariable(target));
            }
            Ok(ValueRepr::Scalar(ctx.env.get(&target).unwrap_or_default()))
        }
    }
}

#[derive(Debug, Clone)]
pub enum ValueRepr {
    Scalar(String),
    Array { elems: Vec<String>, star: bool },
}

enum ElementLookup {
    NotElement,
    Unset,
    Set(ValueRepr),
}

fn read_element_value_if_set(ctx: &mut ExpCtx, name: &str) -> Result<ElementLookup, ExpandError> {
    let Some(bracket) = name.find('[') else {
        return Ok(ElementLookup::NotElement);
    };
    let base = &name[..bracket];
    let close = name.rfind(']').unwrap_or(name.len());
    let subscript = &name[bracket + 1..close];
    if subscript == "@" || subscript == "*" {
        return Ok(ElementLookup::NotElement);
    }
    let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
    if ctx.env.kind(&target) == VarKind::Assoc {
        let key = crate::expand_string_to_string_impl(subscript, ctx)?;
        return Ok(ctx
            .env
            .get_array_assoc(&target, &key)
            .map(|value| ElementLookup::Set(ValueRepr::Scalar(value)))
            .unwrap_or(ElementLookup::Unset));
    }
    let idx = match parse_indexed_subscript(ctx, &target, subscript, &target) {
        Ok(idx) => idx,
        Err(ExpandError::InvalidArraySubscript(label)) => {
            report_array_subscript_error(ctx.env, &label);
            return Ok(ElementLookup::Unset);
        }
        Err(err) => return Err(err),
    };
    if ctx.env.kind(&target) != VarKind::Indexed {
        return Ok(if idx == 0 {
            ctx.env
                .get(&target)
                .map(|value| ElementLookup::Set(ValueRepr::Scalar(value)))
                .unwrap_or(ElementLookup::Unset)
        } else {
            ElementLookup::Unset
        });
    }
    Ok(ctx
        .env
        .get_array_indexed(&target, idx)
        .map(|value| ElementLookup::Set(ValueRepr::Scalar(value)))
        .unwrap_or(ElementLookup::Unset))
}

fn default_test_string(v: &ValueRepr, sep: Option<u8>) -> String {
    match v {
        ValueRepr::Scalar(s) => s.clone(),
        ValueRepr::Array { elems, star: true } => {
            let sep = sep.map(|b| (b as char).to_string()).unwrap_or_default();
            elems.join(&sep)
        }
        ValueRepr::Array { elems, star: false } => elems.join(" "),
    }
}

fn transform_value(v: &ValueRepr, mut f: impl FnMut(&[u8]) -> Vec<u8>) -> ValueRepr {
    match v {
        ValueRepr::Scalar(s) => {
            let bytes = f(&crate::quote::shell_string_to_bytes(s));
            ValueRepr::Scalar(crate::quote::bytes_to_shell_string(&bytes))
        }
        ValueRepr::Array { elems, star } => {
            let new: Vec<String> = elems
                .iter()
                .map(|s| {
                    let bytes = f(&crate::quote::shell_string_to_bytes(s));
                    crate::quote::bytes_to_shell_string(&bytes)
                })
                .collect();
            ValueRepr::Array {
                elems: new,
                star: *star,
            }
        }
    }
}

fn transform_value_result(
    v: &ValueRepr,
    mut f: impl FnMut(&[u8]) -> Result<Vec<u8>, ExpandError>,
) -> Result<ValueRepr, ExpandError> {
    match v {
        ValueRepr::Scalar(s) => {
            let bytes = f(&crate::quote::shell_string_to_bytes(s))?;
            Ok(ValueRepr::Scalar(crate::quote::bytes_to_shell_string(
                &bytes,
            )))
        }
        ValueRepr::Array { elems, star } => {
            let mut new = Vec::with_capacity(elems.len());
            for s in elems {
                let bytes = f(&crate::quote::shell_string_to_bytes(s))?;
                new.push(crate::quote::bytes_to_shell_string(&bytes));
            }
            Ok(ValueRepr::Array {
                elems: new,
                star: *star,
            })
        }
    }
}

fn transform_string_value(v: &ValueRepr, mut f: impl FnMut(&str) -> String) -> ValueRepr {
    match v {
        ValueRepr::Scalar(s) => ValueRepr::Scalar(f(s)),
        ValueRepr::Array { elems, star } => ValueRepr::Array {
            elems: elems.iter().map(|s| f(s)).collect(),
            star: *star,
        },
    }
}

fn push_value(out: &mut ExpandBuf, bytes: &[u8], quoted: bool) {
    if quoted {
        if bytes.is_empty() {
            out.push_quoted_param_null();
        } else {
            for b in bytes {
                out.push_quoted(*b);
            }
        }
    } else {
        if !bytes.iter().any(|b| is_ctl(*b)) {
            out.push_raw(bytes);
            return;
        }
        for b in bytes {
            out.push_literal(*b);
        }
    }
}

fn push_value_buf(
    out: &mut ExpandBuf,
    v: &ValueRepr,
    quoted: bool,
    sep: Option<u8>,
    ifs_null: bool,
    assignment_rhs: bool,
    split_fields: bool,
) {
    match v {
        ValueRepr::Scalar(s) => push_value(out, s.as_bytes(), quoted),
        ValueRepr::Array { elems, star } => push_array_values(
            out,
            elems,
            quoted,
            *star,
            sep,
            ifs_null,
            assignment_rhs,
            split_fields,
        ),
    }
}

fn push_parameter_value(
    ctx: &mut ExpCtx,
    name: &str,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    if let Some((base, subscript)) = array_all_ref(name) {
        let value = read_value(ctx, name, quoted)?;
        if let ValueRepr::Array { elems, .. } = value {
            let star = subscript == "*";
            push_array_values(
                out,
                &elems,
                quoted,
                star,
                value_separator(ctx, quoted, star),
                ctx.ifs.is_null,
                ctx.assign_in_progress,
                ctx.split_fields,
            );
            return Ok(());
        }
        push_value_buf(
            out,
            &value,
            quoted,
            value_separator(ctx, quoted, false),
            ctx.ifs.is_null,
            ctx.assign_in_progress,
            ctx.split_fields,
        );
        let _ = base;
        return Ok(());
    }
    let value = read_value(ctx, name, quoted)?;
    if let ValueRepr::Array { elems, star } = &value {
        let live_ifs = crate::ifs::IfsState::from_env(ctx.env);
        let protect_live_null_fields =
            !quoted && ctx.split_fields && *star && live_ifs.is_null && !ctx.ifs.is_null;
        push_array_values_impl(
            out,
            elems,
            quoted,
            *star,
            value_separator(ctx, quoted, *star),
            if protect_live_null_fields {
                true
            } else {
                ctx.ifs.is_null
            },
            ctx.assign_in_progress,
            ctx.split_fields,
            false,
            protect_live_null_fields,
        );
        return Ok(());
    }
    let star = value_is_star_array(&value);
    push_value_buf(
        out,
        &value,
        quoted,
        value_separator(ctx, quoted, star),
        ctx.ifs.is_null,
        ctx.assign_in_progress,
        ctx.split_fields,
    );
    Ok(())
}

fn value_is_star_array(value: &ValueRepr) -> bool {
    matches!(value, ValueRepr::Array { star: true, .. })
}

fn value_separator(ctx: &mut ExpCtx, quoted: bool, star: bool) -> Option<u8> {
    if quoted && star {
        crate::ifs::IfsState::from_env(ctx.env).join_separator()
    } else {
        ctx.ifs.join_separator()
    }
}

fn push_array_values(
    out: &mut ExpandBuf,
    arr: &[String],
    quoted: bool,
    star: bool,
    sep: Option<u8>,
    ifs_null: bool,
    assignment_rhs: bool,
    split_fields: bool,
) {
    push_array_values_impl(
        out,
        arr,
        quoted,
        star,
        sep,
        ifs_null,
        assignment_rhs,
        split_fields,
        false,
        false,
    )
}

fn push_substring_array_values(
    out: &mut ExpandBuf,
    arr: &[String],
    quoted: bool,
    star: bool,
    sep: Option<u8>,
    ifs_null: bool,
    assignment_rhs: bool,
    split_fields: bool,
) {
    push_array_values_impl(
        out,
        arr,
        quoted,
        star,
        sep,
        ifs_null,
        assignment_rhs,
        split_fields,
        assignment_rhs && !quoted && star,
        false,
    )
}

fn push_array_values_impl(
    out: &mut ExpandBuf,
    arr: &[String],
    quoted: bool,
    star: bool,
    sep: Option<u8>,
    ifs_null: bool,
    assignment_rhs: bool,
    split_fields: bool,
    raw_ctlnul: bool,
    protect_field_values: bool,
) {
    if quoted && star && arr.is_empty() {
        out.push_quoted_null();
        return;
    }
    if !assignment_rhs && !split_fields && !star {
        for (i, s) in arr.iter().enumerate() {
            if i > 0 {
                if quoted {
                    out.push_quoted(b' ');
                } else {
                    out.push_literal(b' ');
                }
            }
            push_array_element_value(out, s.as_bytes(), quoted, raw_ctlnul);
        }
        return;
    }
    if (quoted || assignment_rhs) && !star {
        out.buf_record_dollar_at();
        for (i, s) in arr.iter().enumerate() {
            if i > 0 {
                out.push_field_sep();
            }
            push_array_element_value(out, s.as_bytes(), quoted, raw_ctlnul);
        }
        return;
    }
    if !quoted && !assignment_rhs && split_fields && ifs_null {
        for (i, s) in arr.iter().enumerate() {
            if i > 0 {
                out.push_field_sep();
            }
            push_array_element_value(out, s.as_bytes(), protect_field_values, raw_ctlnul);
        }
        return;
    }
    for (i, s) in arr.iter().enumerate() {
        if i > 0 {
            if let Some(sep) = sep {
                if quoted {
                    out.push_quoted(sep);
                } else {
                    out.push_literal(sep);
                }
            }
        }
        push_array_element_value(out, s.as_bytes(), quoted, raw_ctlnul);
    }
}

fn push_array_element_value(out: &mut ExpandBuf, bytes: &[u8], quoted: bool, raw_ctlnul: bool) {
    if quoted && bytes.is_empty() {
        out.push_quoted_null();
        return;
    }
    push_value_maybe_raw_ctlnul(out, bytes, quoted, raw_ctlnul);
}

fn push_value_maybe_raw_ctlnul(out: &mut ExpandBuf, bytes: &[u8], quoted: bool, raw_ctlnul: bool) {
    if !raw_ctlnul || quoted {
        push_value(out, bytes, quoted);
        return;
    }
    for b in bytes {
        if *b == CTLNUL {
            out.push_raw_byte(CTLNUL);
        } else {
            out.push_literal(*b);
        }
    }
}

fn array_all_ref(name: &str) -> Option<(&str, &str)> {
    let bracket = name.find('[')?;
    let close = name.rfind(']')?;
    if close + 1 != name.len() {
        return None;
    }
    let subscript = &name[bracket + 1..close];
    if subscript == "@" || subscript == "*" {
        Some((&name[..bracket], subscript))
    } else {
        None
    }
}

fn special_param(name: &str, ctx: &mut ExpCtx) -> Option<String> {
    match name {
        "?" => Some(ctx.env.last_status().to_string()),
        "#" => Some(ctx.env.positional_count().to_string()),
        "$" => Some(ctx.env.shell_pid().to_string()),
        "!" => Some(
            ctx.env
                .last_async_pid()
                .map(|p| p.to_string())
                .unwrap_or_default(),
        ),
        "-" => Some(ctx.env.get("-").unwrap_or_else(|| option_letters(ctx))),
        "0" => ctx.env.positional(0),
        _ => None,
    }
}

fn option_letters(ctx: &mut ExpCtx) -> String {
    let mut s = String::new();
    for (k, c) in [
        ("errexit", 'e'),
        ("nounset", 'u'),
        ("xtrace", 'x'),
        ("noclobber", 'C'),
        ("monitor", 'm'),
        ("posix", 'p'),
    ] {
        if ctx.env.option(k) {
            s.push(c);
        }
    }
    s
}

fn parse_subscript(ctx: &mut ExpCtx, s: &str) -> Result<i64, ExpandError> {
    let expanded = crate::expand_arith_string_to_string_impl(s, ctx)?;
    if expanded.trim().is_empty() {
        return Ok(0);
    }
    arith::eval_preexpanded(&expanded, ctx)
}

fn parse_indexed_subscript(
    ctx: &mut ExpCtx,
    target: &str,
    subscript: &str,
    label: &str,
) -> Result<i64, ExpandError> {
    let index = parse_subscript(ctx, subscript)?;
    if index >= 0 {
        return Ok(index);
    }
    let Some(max) = ctx.env.array_max_index(target) else {
        return Err(ExpandError::InvalidArraySubscript(label.to_string()));
    };
    let resolved = max + 1 + index;
    if resolved < 0 {
        Err(ExpandError::InvalidArraySubscript(label.to_string()))
    } else {
        Ok(resolved)
    }
}

fn positionals(ctx: &mut ExpCtx) -> Vec<String> {
    (1..=ctx.env.positional_count())
        .filter_map(|i| ctx.env.positional(i))
        .collect()
}

fn positional_substring_elems(
    ctx: &mut ExpCtx,
    off_val: i64,
    len_val: Option<i64>,
    len_expr: Option<&str>,
) -> Result<Vec<String>, ExpandError> {
    if let Some(len) = len_val {
        if len < 0 {
            let display = len_expr.unwrap_or_default();
            return Err(ExpandError::Other(format!(
                "{display}: substring expression < 0"
            )));
        }
    }

    let count = ctx.env.positional_count() as i64;
    let mut elems = Vec::new();
    if off_val < 0 {
        let first = count + 1 + off_val;
        if first < 0 {
            return Ok(elems);
        }
        if first == 0 {
            if let Some(v) = ctx.env.positional(0) {
                elems.push(v);
            }
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i as usize) {
                    elems.push(v);
                }
            }
        } else {
            for i in first..=count {
                if let Some(v) = ctx.env.positional(i as usize) {
                    elems.push(v);
                }
            }
        }
    } else {
        if off_val == 0 {
            if let Some(v) = ctx.env.positional(0) {
                elems.push(v);
            }
        }
        let first = off_val.max(1);
        for i in first..=count {
            if let Some(v) = ctx.env.positional(i as usize) {
                elems.push(v);
            }
        }
    }

    if let Some(len) = len_val {
        elems.truncate(len as usize);
    }
    Ok(elems)
}

fn join_array(ctx: &mut ExpCtx, base: &str, sep: u8) -> String {
    let target = nameref::resolve(ctx.env, base).unwrap_or_else(|| base.to_string());
    match ctx.env.kind(&target) {
        VarKind::Indexed => {
            let all = ctx.env.get_array_all(&target).unwrap_or_default();
            all.iter()
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
                .join(&(sep as char).to_string())
        }
        VarKind::Assoc => {
            let all = ctx.env.assoc_all(&target).unwrap_or_default();
            all.iter()
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
                .join(&(sep as char).to_string())
        }
        _ => ctx.env.get(&target).unwrap_or_default(),
    }
}

fn expand_special(
    name: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    if name == "@" || name == "*" {
        let count = ctx.env.positional_count();
        let star = name == "*";
        let sep = if quoted && star {
            crate::ifs::IfsState::from_env(ctx.env).join_separator_bytes()
        } else {
            ctx.ifs.join_separator().map(|b| vec![b])
        };
        if quoted && star && count == 0 {
            out.push_quoted_null();
            return Ok(());
        }
        // When unquoted, both $@ and $* expand the same way (separators are IFS).
        // When quoted, "$@" inserts CTLESC-protected separators that survive
        // split into independent fields; "$*" joins by IFS[0].
        if ctx.assign_in_progress && !star {
            out.buf_record_dollar_at();
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i) {
                    if i > 1 {
                        out.push_field_sep();
                    }
                    push_array_element_value(out, v.as_bytes(), quoted, false);
                }
            }
            return Ok(());
        }
        if ctx.assign_in_progress && star {
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i) {
                    if i > 1 {
                        if let Some(sep) = sep.as_deref() {
                            if quoted {
                                out.push_quoted_slice(sep);
                            } else {
                                for b in sep {
                                    out.push_literal(*b);
                                }
                            }
                        }
                    }
                    push_array_element_value(out, v.as_bytes(), quoted, false);
                }
            }
            return Ok(());
        }
        if !ctx.split_fields && !star {
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i) {
                    if i > 1 {
                        if quoted {
                            out.push_quoted(b' ');
                        } else {
                            out.push_literal(b' ');
                        }
                    }
                    push_array_element_value(out, v.as_bytes(), quoted, false);
                }
            }
            return Ok(());
        }
        if ctx.param_rhs_nosplit
            && !star
            && !quoted
            && !ctx.ifs.is_unset
            && !ctx.ifs.is_null
            && ctx.ifs.first_char != Some(b' ')
        {
            out.buf_record_dollar_at();
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i) {
                    if i > 1 {
                        out.push_literal(b' ');
                    }
                    out.push_quoted_slice(v.as_bytes());
                }
            }
            return Ok(());
        }
        if quoted && !star {
            out.buf_record_dollar_at();
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i) {
                    if i > 1 {
                        out.push_field_sep();
                    }
                    push_array_element_value(out, v.as_bytes(), true, false);
                }
            }
            return Ok(());
        }
        if !quoted && ctx.split_fields && ctx.ifs.is_null {
            for i in 1..=count {
                if let Some(v) = ctx.env.positional(i) {
                    if i > 1 {
                        out.push_field_sep();
                    }
                    push_array_element_value(out, v.as_bytes(), false, false);
                }
            }
            return Ok(());
        }
        for i in 1..=count {
            if let Some(v) = ctx.env.positional(i) {
                if i > 1 {
                    if let Some(sep) = sep.as_deref() {
                        if quoted {
                            out.push_quoted_slice(sep);
                        } else {
                            for b in sep {
                                out.push_literal(*b);
                            }
                        }
                    }
                }
                push_array_element_value(out, v.as_bytes(), quoted, false);
            }
        }
        return Ok(());
    }
    if name == "!" && ctx.eval_unbound_error && ctx.env.last_async_pid().is_none() {
        return Err(ExpandError::UnboundVariable("$!".into()));
    }
    if let Ok(n) = name.parse::<usize>() {
        if n != 0 && ctx.eval_unbound_error && ctx.env.positional(n).is_none() {
            return Err(ExpandError::UnboundVariable(name.into()));
        }
    }
    let v = special_param(name, ctx).unwrap_or_else(|| {
        // Numeric positional
        if let Ok(n) = name.parse::<usize>() {
            ctx.env.positional(n).unwrap_or_default()
        } else {
            String::new()
        }
    });
    push_value(out, v.as_bytes(), quoted);
    Ok(())
}

fn expand_pattern(raw: &[u8], ctx: &mut ExpCtx) -> Result<Vec<u8>, ExpandError> {
    let s = std::str::from_utf8(raw).unwrap_or("");
    let prev_split_fields = ctx.split_fields;
    ctx.split_fields = false;
    let result = crate::expand_word_string(s, ctx, false);
    ctx.split_fields = prev_split_fields;
    let expanded = result?;
    // Patterns preserve their internal CTLESC markers (quoting denotes literal).
    Ok(expanded.into_bytes())
}

/// Locate the closing `}` for Bash 5.3's current-shell command substitution
/// forms (`${ command; }` and `${| command; }`). Unlike parameter expansion,
/// the body is shell code, so literal function/group braces are balanced too.
fn extract_current_brace_body(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut i = start;
    let mut brace_depth = 0usize;
    let mut body = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() {
            body.push(b);
            body.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if append_ansi_c_literal(bytes, &mut i, &mut body) {
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            append_quoted_bytes(bytes, &mut i, &mut body, b)?;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n == b'{' {
                body.extend_from_slice(b"${");
                i += 2;
                if current_subst_mode(bytes, i).is_some() {
                    let body_start = if bytes[i] == b'|' { i + 1 } else { i };
                    if bytes[i] == b'|' {
                        body.push(b'|');
                    }
                    let (inner, end) = extract_current_brace_body(bytes, body_start)?;
                    body.extend_from_slice(&inner);
                    body.push(b'}');
                    i = end;
                } else {
                    let (inner, end) = extract_brace_body(bytes, i, false)?;
                    body.extend_from_slice(&inner);
                    body.push(b'}');
                    i = end;
                }
                continue;
            }
            if n == b'(' {
                body.extend_from_slice(b"$(");
                i += 2;
                if i < bytes.len() && bytes[i] == b'(' {
                    body.push(b'(');
                    i += 1;
                    let (inner, end) = extract_double_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.extend_from_slice(b"))");
                    i = end;
                } else {
                    let (inner, end) = extract_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.push(b')');
                    i = end;
                }
                continue;
            }
        }
        if b == b'{' {
            brace_depth = brace_depth.saturating_add(1);
            body.push(b);
            i += 1;
            continue;
        }
        if b == b'}' {
            if brace_depth == 0 {
                return Ok((body, i + 1));
            }
            brace_depth -= 1;
            body.push(b);
            i += 1;
            continue;
        }
        body.push(b);
        i += 1;
    }
    Err(ExpandError::BadSubstitution("missing '}'".into()))
}

/// Locate matching `}` accounting for nested `{}`, `$()`, `$(())`, `${}`, and
/// quoted runs.
fn extract_brace_body(
    bytes: &[u8],
    start: usize,
    posix_single_quote: bool,
) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut i = start;
    let mut depth: i32 = 1;
    let mut body = Vec::new();
    let mut swallowed_closing_brace_in_quote = false;
    while i < bytes.len() {
        let b = bytes[i];
        if append_ansi_c_literal(bytes, &mut i, &mut body) {
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'}' {
            body.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            body.push(bytes[i]);
            body.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if b == b'\'' && !posix_single_quote {
            let mut j = i + 1;
            let mut saw_escaped_quote = false;
            let mut closes_before_quote = false;
            while j < bytes.len() && bytes[j] != b'\'' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    if bytes[j + 1] == b'\'' {
                        saw_escaped_quote = true;
                    }
                    j += 2;
                    continue;
                }
                if bytes[j] == b'}' && saw_escaped_quote {
                    closes_before_quote = true;
                    break;
                }
                j += 1;
            }
            if closes_before_quote {
                body.push(b);
                i += 1;
                continue;
            }
            body.push(b);
            i += 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                body.push(bytes[i]);
                i += 1;
            }
            if i < bytes.len() {
                body.push(bytes[i]);
                i += 1;
            }
            continue;
        }
        if b == b'"' {
            let quote_start = i;
            let body_len = body.len();
            let mut swallowed_in_this_quote = false;
            body.push(b);
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'}' {
                    swallowed_in_this_quote = true;
                }
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    body.push(bytes[i]);
                    body.push(bytes[i + 1]);
                    i += 2;
                } else {
                    body.push(bytes[i]);
                    i += 1;
                }
            }
            if i >= bytes.len() {
                if !posix_single_quote {
                    body.truncate(body_len);
                    body.push(b);
                    i = quote_start + 1;
                    continue;
                }
                return Err(ExpandError::Other(
                    "unexpected EOF while looking for matching `}'".into(),
                ));
            }
            if swallowed_in_this_quote {
                swallowed_closing_brace_in_quote = true;
            }
            body.push(bytes[i]);
            i += 1;
            continue;
        }
        if b == b'`' {
            append_quoted_bytes(bytes, &mut i, &mut body, b)?;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n == b'{' {
                body.push(b);
                body.push(n);
                i += 2;
                let (inner, end) = extract_brace_body(bytes, i, posix_single_quote)?;
                body.extend_from_slice(&inner);
                body.push(b'}');
                i = end;
                continue;
            }
            if n == b'(' {
                body.push(b);
                body.push(n);
                i += 2;
                let is_arith = i < bytes.len() && bytes[i] == b'(';
                if is_arith {
                    body.push(b'(');
                    i += 1;
                    let (inner, end) = extract_double_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.extend_from_slice(b"))");
                    i = end;
                } else {
                    let (inner, end) = extract_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.push(b')');
                    i = end;
                }
                continue;
            }
        }
        if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Ok((body, i + 1));
            }
        }
        body.push(b);
        i += 1;
    }
    if swallowed_closing_brace_in_quote {
        return Err(ExpandError::Other(
            "unexpected EOF while looking for matching `}'".into(),
        ));
    }
    Err(ExpandError::BadSubstitution("missing '}'".into()))
}

/// Find matching `)` starting just after `$(`.
fn extract_paren(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut i = start;
    let mut depth = 1i32;
    let mut case_depth = 0usize;
    let mut comment_ok = true;
    let mut heredocs: std::collections::VecDeque<(Vec<u8>, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut body = Vec::new();
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let line_end = i;
                let mut candidate_start = line_start;
                let mut candidate = &bytes[candidate_start..line_end];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate_start += 1;
                        candidate = &candidate[1..];
                    }
                }
                if depth == 1 {
                    if let Some(close_offset) =
                        heredoc_delimiter_closing_paren(candidate, delimiter.as_slice())
                    {
                        body.extend_from_slice(&bytes[line_start..candidate_start]);
                        body.extend_from_slice(delimiter.as_slice());
                        heredocs.pop_front();
                        i = candidate_start + close_offset + 1;
                        return Ok((body, i));
                    }
                }
                body.extend_from_slice(&bytes[line_start..line_end]);
                if candidate == delimiter.as_slice() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    body.push(bytes[i]);
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            body.push(b);
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                body.push(bytes[i]);
                i += 1;
            }
            continue;
        }

        if b == b'\\' && i + 1 < bytes.len() {
            body.push(b);
            body.push(bytes[i + 1]);
            i += 2;
            comment_ok = false;
            continue;
        }

        if append_ansi_c_literal(bytes, &mut i, &mut body) {
            comment_ok = false;
            continue;
        }

        if matches!(b, b'\'' | b'"' | b'`') {
            append_quoted_bytes(bytes, &mut i, &mut body, b)?;
            comment_ok = false;
            continue;
        }

        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            append_heredoc_redirect(bytes, &mut i, &mut body, &mut heredocs);
            comment_ok = false;
            continue;
        }

        if is_name_start(b) {
            let start_word = i;
            body.push(bytes[i]);
            i += 1;
            while i < bytes.len() && is_name_byte(bytes[i]) {
                body.push(bytes[i]);
                i += 1;
            }
            let previous = previous_significant_byte(bytes, start_word);
            match &bytes[start_word..i] {
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" if !matches!(previous, Some(b'|' | b'(')) => {
                    case_depth = case_depth.saturating_sub(1)
                }
                _ => {}
            }
            comment_ok = false;
            continue;
        }

        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            if case_depth > 0 && depth == 1 {
                body.push(b);
                i += 1;
                comment_ok = true;
                continue;
            }
            depth -= 1;
            if depth == 0 {
                return Ok((body, i + 1));
            }
        }
        body.push(b);
        i += 1;
        comment_ok = b.is_ascii_whitespace() || is_shell_metacharacter(b);
    }
    Err(ExpandError::BadSubstitution("missing ')'".into()))
}

fn heredoc_delimiter_closing_paren(candidate: &[u8], delimiter: &[u8]) -> Option<usize> {
    if !candidate.starts_with(delimiter) {
        return None;
    }
    let mut offset = delimiter.len();
    while offset < candidate.len() && matches!(candidate[offset], b' ' | b'\t') {
        offset += 1;
    }
    if candidate.get(offset) == Some(&b')') {
        Some(offset)
    } else {
        None
    }
}

/// Find matching `))` of `$((...))`. `start` is the position just after the
/// inner `(`.
fn extract_double_paren(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    if let DParenScan::Arithmetic(end) = arith_dparen_scan(bytes, start) {
        Ok((bytes[start..end - 2].to_vec(), end))
    } else {
        Err(ExpandError::ArithSyntax("missing '))'".into()))
    }
}

fn extract_legacy_arith(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            skip_quoted_arith(bytes, &mut i, b);
            continue;
        }
        if b == b'[' {
            depth += 1;
            i += 1;
            continue;
        }
        if b == b']' {
            depth -= 1;
            if depth == 0 {
                return Ok((bytes[start..i].to_vec(), i + 1));
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    Err(ExpandError::ArithSyntax("missing ']'".into()))
}

fn skip_quoted_arith(bytes: &[u8], i: &mut usize, quote: u8) {
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

fn append_ansi_c_literal(bytes: &[u8], i: &mut usize, body: &mut Vec<u8>) -> bool {
    if *i + 1 >= bytes.len() || bytes[*i] != b'$' || bytes[*i + 1] != b'\'' {
        return false;
    }
    let (_, end) = quote::scan_ansi_c_quoted(bytes, *i + 2);
    body.extend_from_slice(&bytes[*i..end]);
    *i = end;
    true
}

fn append_quoted_bytes(
    bytes: &[u8],
    i: &mut usize,
    body: &mut Vec<u8>,
    quote: u8,
) -> Result<(), ExpandError> {
    if quote == b'"' {
        return append_double_quoted_bytes(bytes, i, body);
    }
    body.push(quote);
    *i += 1;
    while *i < bytes.len() && bytes[*i] != quote {
        if bytes[*i] == b'\\' && *i + 1 < bytes.len() {
            body.push(bytes[*i]);
            body.push(bytes[*i + 1]);
            *i += 2;
        } else {
            body.push(bytes[*i]);
            *i += 1;
        }
    }
    if *i < bytes.len() {
        body.push(bytes[*i]);
        *i += 1;
    }
    Ok(())
}

fn append_double_quoted_bytes(
    bytes: &[u8],
    i: &mut usize,
    body: &mut Vec<u8>,
) -> Result<(), ExpandError> {
    body.push(b'"');
    *i += 1;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'"' {
            body.push(b'"');
            *i += 1;
            return Ok(());
        }
        if b == b'\\' && *i + 1 < bytes.len() {
            body.push(bytes[*i]);
            body.push(bytes[*i + 1]);
            *i += 2;
            continue;
        }
        if b == b'$' && *i + 1 < bytes.len() {
            match bytes[*i + 1] {
                b'{' => {
                    body.extend_from_slice(b"${");
                    *i += 2;
                    let (inner, end) = extract_brace_body(bytes, *i, false)?;
                    body.extend_from_slice(&inner);
                    body.push(b'}');
                    *i = end;
                    continue;
                }
                b'(' => {
                    body.extend_from_slice(b"$(");
                    *i += 2;
                    if *i < bytes.len() && bytes[*i] == b'(' {
                        body.push(b'(');
                        *i += 1;
                        let (inner, end) = extract_double_paren(bytes, *i)?;
                        body.extend_from_slice(&inner);
                        body.extend_from_slice(b"))");
                        *i = end;
                    } else {
                        let (inner, end) = extract_paren(bytes, *i)?;
                        body.extend_from_slice(&inner);
                        body.push(b')');
                        *i = end;
                    }
                    continue;
                }
                _ => {}
            }
        }
        if b == b'`' {
            append_quoted_bytes(bytes, i, body, b'`')?;
            continue;
        }
        body.push(b);
        *i += 1;
    }
    Ok(())
}

fn append_heredoc_redirect(
    bytes: &[u8],
    i: &mut usize,
    body: &mut Vec<u8>,
    heredocs: &mut std::collections::VecDeque<(Vec<u8>, bool)>,
) {
    body.push(b'<');
    *i += 1;
    body.push(b'<');
    *i += 1;
    let strip_tabs = if *i < bytes.len() && bytes[*i] == b'-' {
        body.push(b'-');
        *i += 1;
        true
    } else {
        false
    };
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t') {
        body.push(bytes[*i]);
        *i += 1;
    }
    let mut delimiter = Vec::new();
    let mut quote: Option<u8> = None;
    while *i < bytes.len() {
        let b = bytes[*i];
        if let Some(q) = quote {
            body.push(b);
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
            body.push(b);
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
                body.push(b);
                delimiter.push(bytes[*i]);
                body.push(bytes[*i]);
                *i += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() || is_shell_metacharacter(b) {
            break;
        }
        delimiter.push(b);
        body.push(b);
        *i += 1;
    }
    if strip_tabs {
        while delimiter.first() == Some(&b'\t') {
            delimiter.remove(0);
        }
    }
    if !delimiter.is_empty() {
        heredocs.push_back((delimiter, strip_tabs));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DParenScan {
    Arithmetic(usize),
    CommandSubstitution,
    Missing,
}

fn arith_dparen_scan(bytes: &[u8], start: usize) -> DParenScan {
    let mut i = start;
    let mut depth = 1i32;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                if depth == 1 {
                    if i + 1 < bytes.len() && bytes[i + 1] == b')' {
                        return DParenScan::Arithmetic(i + 2);
                    }
                    let mut j = i + 1;
                    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\r') {
                        j += 1;
                    }
                    if j >= bytes.len() || matches!(bytes[j], b')' | b'\n' | b';' | b'&' | b'|') {
                        return DParenScan::CommandSubstitution;
                    }
                    return DParenScan::Missing;
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    DParenScan::Missing
}

fn is_name_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_name_byte(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit()
}

fn is_valid_name_str(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty() && is_name_start(bytes[0]) && bytes[1..].iter().all(|b| is_name_byte(*b))
}

fn previous_significant_byte(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i > 0 {
        i -= 1;
        if !bytes[i].is_ascii_whitespace() {
            return Some(bytes[i]);
        }
    }
    None
}

fn is_shell_metacharacter(b: u8) -> bool {
    matches!(b, b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')')
}

/// Bridge into ExpandBuf for $@ semantic notes.
trait BufExt {
    fn buf_record_dollar_at(&mut self);
}
impl BufExt for ExpandBuf {
    fn buf_record_dollar_at(&mut self) {
        self.quoted_dollar_at = true;
        self.has_dollar_at = true;
    }
}

/// `<( ... )` / `>( ... )` - invoked by internal.rs which sees the leading
/// `<(` or `>(`.
pub fn process_subst_expand(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    quoted: bool,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    let dir = if bytes[*i] == b'<' {
        ProcSubstDir::Input
    } else {
        ProcSubstDir::Output
    };
    *i += 2; // skip `<(` or `>(`
    let (body, end) = extract_paren(bytes, *i)?;
    *i = end;
    let src = String::from_utf8_lossy(&body).into_owned();
    let buf = procsub::process_substitute(&src, dir, ctx, quoted)?;
    out.extend_from(&buf);
    Ok(())
}

/// Public for use by internal.rs.
pub fn special_byte(b: u8) -> bool {
    matches!(b, b'?' | b'#' | b'$' | b'!' | b'-' | b'*' | b'@')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cherubsh_common::{Environment, Span, VarKind, W_HASDOLLAR};
    use cherubsh_parser::WordDesc;

    use crate::{expand_word_list, ExpandError, ExpandFlags, NullRunner};

    #[derive(Default)]
    struct TestEnv {
        vars: BTreeMap<String, String>,
        arrays: BTreeMap<String, BTreeMap<i64, String>>,
        assoc: BTreeMap<String, BTreeMap<String, String>>,
        positionals: Vec<String>,
        posix: bool,
        nounset: bool,
        nocasematch: bool,
    }

    impl Environment for TestEnv {
        fn get(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }

        fn set(&mut self, name: &str, value: String) {
            self.vars.insert(name.to_string(), value);
        }

        fn unset(&mut self, name: &str) {
            self.vars.remove(name);
            self.assoc.remove(name);
        }

        fn exported(&self, _name: &str) -> bool {
            false
        }

        fn export(&mut self, _name: &str) {}

        fn positional(&self, index: usize) -> Option<String> {
            self.positionals.get(index).cloned()
        }

        fn positional_count(&self) -> usize {
            self.positionals.len().saturating_sub(1)
        }

        fn set_positionals(&mut self, params: Vec<String>) {
            self.positionals = params;
        }

        fn last_status(&self) -> i32 {
            0
        }

        fn set_last_status(&mut self, _status: i32) {}

        fn option(&self, name: &str) -> bool {
            match name {
                "posix" => self.posix,
                "nounset" => self.nounset,
                "nocasematch" => self.nocasematch,
                _ => false,
            }
        }

        fn all_var_names_with_prefix(&self, prefix: &str) -> Vec<String> {
            let mut out: Vec<String> = self
                .vars
                .keys()
                .chain(self.arrays.keys())
                .chain(self.assoc.keys())
                .filter(|name| name.starts_with(prefix))
                .cloned()
                .collect();
            out.sort();
            out.dedup();
            out
        }

        fn get_array_all(&self, name: &str) -> Option<Vec<(i64, String)>> {
            self.arrays.get(name).map(|items| {
                items
                    .iter()
                    .map(|(key, value)| (*key, value.clone()))
                    .collect()
            })
        }

        fn get_array_indexed(&self, name: &str, index: i64) -> Option<String> {
            self.arrays
                .get(name)
                .and_then(|items| items.get(&index))
                .cloned()
        }

        fn array_keys(&self, name: &str) -> Option<Vec<i64>> {
            self.arrays
                .get(name)
                .map(|items| items.keys().copied().collect())
        }

        fn array_len(&self, name: &str) -> usize {
            self.arrays.get(name).map(|items| items.len()).unwrap_or(0)
        }

        fn get_array_assoc(&self, name: &str, key: &str) -> Option<String> {
            self.assoc
                .get(name)
                .and_then(|items| items.get(key))
                .cloned()
        }

        fn assoc_all(&self, name: &str) -> Option<Vec<(String, String)>> {
            self.assoc.get(name).map(|items| {
                items
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
        }

        fn assoc_keys(&self, name: &str) -> Option<Vec<String>> {
            self.assoc
                .get(name)
                .map(|items| items.keys().cloned().collect())
        }

        fn assoc_len(&self, name: &str) -> usize {
            self.assoc.get(name).map(|items| items.len()).unwrap_or(0)
        }

        fn kind(&self, name: &str) -> VarKind {
            if self.assoc.contains_key(name) {
                VarKind::Assoc
            } else if self.arrays.contains_key(name) {
                VarKind::Indexed
            } else if self.vars.contains_key(name) {
                VarKind::Scalar
            } else {
                VarKind::Unset
            }
        }
    }

    fn expand_one(env: &mut TestEnv, text: &str) -> Vec<String> {
        let mut runner = NullRunner::default();
        let flags = text
            .as_bytes()
            .iter()
            .any(|b| matches!(b, b'$' | b'`'))
            .then_some(W_HASDOLLAR)
            .unwrap_or(0);
        let words = expand_word_list(
            &[WordDesc {
                text: text.to_string(),
                flags,
                span: Span::dummy(),
                raw: None,
            }],
            env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap();
        words.into_iter().map(|word| word.text).collect()
    }

    fn expand_error(env: &mut TestEnv, text: &str) -> ExpandError {
        let mut runner = NullRunner::default();
        let flags = text
            .as_bytes()
            .iter()
            .any(|b| matches!(b, b'$' | b'`'))
            .then_some(W_HASDOLLAR)
            .unwrap_or(0);
        expand_word_list(
            &[WordDesc {
                text: text.to_string(),
                flags,
                span: Span::dummy(),
                raw: None,
            }],
            env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err()
    }

    #[test]
    fn quoted_star_substring_joins_with_first_ifs_char() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "uv\x01\x01wx".into(), "uv\x01\x01wx".into()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#""${*:1:2}""#),
            vec!["uv\x01\x01wx uv\x01\x01wx"]
        );
        assert_eq!(expand_one(&mut env, r#""${*:1:0}""#), vec![""]);
    }

    #[test]
    fn quoted_at_substring_preserves_separate_fields() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "uv\x01\x01wx".into(), "uv\x01\x01wx".into()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#""${@:1:2}""#),
            vec!["uv\x01\x01wx", "uv\x01\x01wx"]
        );
    }

    #[test]
    fn assoc_scalar_substring_reads_key_zero() {
        let mut env = TestEnv::default();
        env.assoc.insert(
            "assoc".into(),
            BTreeMap::from([
                ("0".into(), "uv\x01\x01wx".into()),
                ("1".into(), "other".into()),
            ]),
        );

        assert_eq!(expand_one(&mut env, "${assoc:0:4}"), vec!["uv\x01\x01"]);
        assert_eq!(
            expand_one(&mut env, r#""${assoc:0:4}""#),
            vec!["uv\x01\x01"]
        );
    }

    #[test]
    fn assoc_subscript_skips_quoted_closing_bracket() {
        let mut env = TestEnv::default();
        env.assoc.insert(
            "myarray".into(),
            BTreeMap::from([("a]a".into(), "abc".into())]),
        );
        assert_eq!(expand_one(&mut env, r#"${myarray["a]a"]}"#), vec!["abc"]);
    }

    #[test]
    fn indexed_array_substring_uses_sparse_indexes() {
        let mut env = TestEnv::default();
        env.arrays.insert(
            "a".into(),
            BTreeMap::from([(1, "one".into()), (3, "three".into()), (10, "ten".into())]),
        );

        assert_eq!(
            expand_one(&mut env, r#""${a[@]:1}""#),
            vec!["one", "three", "ten"]
        );
        assert_eq!(
            expand_one(&mut env, r#""${a[@]:2:2}""#),
            vec!["three", "ten"]
        );
        assert_eq!(expand_one(&mut env, r#""${a[@]: -1}""#), vec!["ten"]);
    }

    #[test]
    fn assoc_length_uses_entry_count() {
        let mut env = TestEnv::default();
        env.assoc.insert(
            "m".into(),
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())]),
        );

        assert_eq!(expand_one(&mut env, "${#m[@]}"), vec!["2"]);
    }

    #[test]
    fn literal_remove_and_substitute_match_pattern_output() {
        let mut env = TestEnv::default();
        env.vars.insert("s".into(), "alpha_beta_gamma_delta".into());

        assert_eq!(
            expand_one(&mut env, "${s//alpha/omega}"),
            vec!["omega_beta_gamma_delta"]
        );
        assert_eq!(
            expand_one(&mut env, "${s#alpha_}"),
            vec!["beta_gamma_delta"]
        );
        assert_eq!(
            expand_one(&mut env, "${s%_delta}"),
            vec!["alpha_beta_gamma"]
        );
    }

    #[test]
    fn scalar_all_ref_substring_uses_scalar_value() {
        let mut env = TestEnv::default();
        env.vars.insert("var".into(), "blah".into());

        assert_eq!(expand_one(&mut env, r#""${var[@]:3}""#), vec!["h"]);
        assert_eq!(expand_one(&mut env, r#""${var[*]:3}""#), vec!["h"]);
        assert_eq!(expand_one(&mut env, r#""${var[@]:0:0}""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#""${var[*]:0:0}""#), vec![""]);

        env.vars.remove("var");
        assert_eq!(
            expand_one(&mut env, r#""${var[@]:3}""#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, r#""${var[*]:3}""#), vec![""]);
    }

    #[test]
    fn substring_offset_ternary_colon_is_not_length_separator() {
        let mut env = TestEnv::default();
        env.vars.insert("PARAM".into(), "abcdefg".into());

        assert_eq!(expand_one(&mut env, "${PARAM:1 ? 4 : 2}"), vec!["efg"]);
        assert_eq!(expand_one(&mut env, "${PARAM:1 ? 4 : 2:1}"), vec!["e"]);
        assert_eq!(expand_one(&mut env, "${PARAM: 4<5 ? 4 : 2}"), vec!["efg"]);
        assert_eq!(expand_one(&mut env, "${PARAM: 5>4 ? 4 : 2:1}"), vec!["e"]);
    }

    #[test]
    fn substring_arithmetic_diagnostics_match_bash() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "a".into()],
            ..Default::default()
        };
        env.vars.insert("PARAM".into(), "abcdefg".into());
        env.vars.insert("bad".into(), "}".into());

        let err = expand_error(&mut env, "${PARAM:bad}");
        assert!(matches!(
            err,
            ExpandError::ArithSyntax(msg)
                if msg == r#"PARAM: }: arithmetic syntax error: operand expected (error token is "}")"#
        ));

        let err = expand_error(&mut env, r#"${@:1:$(($# - 2))}"#);
        assert!(matches!(
            err,
            ExpandError::Other(msg) if msg == "$(($# - 2)): substring expression < 0"
        ));
    }

    #[test]
    fn malformed_nested_pattern_substitution_reports_outer_braces() {
        let mut env = TestEnv::default();
        env.vars.insert("c".into(), String::new());

        let err = expand_error(&mut env, r#"${c//${$(($#-1))}/x/}"#);
        assert!(matches!(
            err,
            ExpandError::BadSubstitution(msg) if msg == r#"${$(($#-1))}"#
        ));
    }

    #[test]
    fn parameter_patterns_honor_nocasematch() {
        let mut env = TestEnv {
            nocasematch: true,
            ..Default::default()
        };
        env.vars.insert("string".into(), "abcd".into());

        assert_eq!(expand_one(&mut env, "${string//A/z}"), vec!["zbcd"]);
        assert_eq!(expand_one(&mut env, "${string//BC/x}"), vec!["axd"]);
        assert_eq!(expand_one(&mut env, "${string//[BC]/x}"), vec!["axxd"]);
        assert_eq!(expand_one(&mut env, "${string//[bC]/x}"), vec!["axxd"]);
    }

    #[test]
    fn pattern_substitution_unclosed_bracket_still_finds_separator() {
        let mut env = TestEnv::default();
        env.vars.insert("var".into(), "[hello".into());

        assert_eq!(expand_one(&mut env, r#""${var//[/}""#), vec!["hello"]);
    }

    #[test]
    fn transform_declare_quotes_array_control_and_raw_bytes() {
        let mut env = TestEnv::default();
        env.arrays.insert(
            "array".into(),
            BTreeMap::from([(0, "x\u{1}y\u{7f}z".into())]),
        );
        assert_eq!(
            expand_one(&mut env, r#""${array[@]@A}""#),
            vec![r#"declare -a array=([0]=$'x\001y\177z')"#]
        );

        env.assoc.insert(
            "assoc".into(),
            BTreeMap::from([(
                "x\u{1}y\u{7f}z".into(),
                crate::quote::bytes_to_shell_string(&[b'a', 0xa2, b'b', 0x02, b'c']),
            )]),
        );
        assert_eq!(
            expand_one(&mut env, r#""${assoc[@]@A}""#),
            vec![r#"declare -A assoc=([$'x\001y\177z']=$'a\242b\002c' )"#]
        );
    }

    #[test]
    fn quoted_empty_parameter_preserves_field() {
        let mut env = TestEnv::default();
        env.vars.insert("empty".into(), String::new());

        assert_eq!(expand_one(&mut env, r#""$empty""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#"x"$empty"y"#), vec!["xy"]);
    }

    #[test]
    fn quoted_default_alt_treats_single_quotes_as_literals() {
        let mut env = TestEnv::default();
        env.vars.insert("set".into(), "value".into());

        assert_eq!(expand_one(&mut env, r#""${set:+'$set'}""#), vec!["'value'"]);
        assert_eq!(expand_one(&mut env, r#""${unset:-''}""#), vec!["''"]);
        assert_eq!(expand_one(&mut env, r#""${unset:-}""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#""${set:+}""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#""${set:+"\p"}""#), vec!["p"]);
    }

    #[test]
    fn quoted_default_alt_expands_top_level_dollar_quotes() {
        let mut env = TestEnv::default();

        assert_eq!(expand_one(&mut env, r#""${unset:-$'\t'}""#), vec!["\t"]);
        assert_eq!(expand_one(&mut env, r#""${unset:-$"hi"}""#), vec!["hi"]);
        assert_eq!(
            expand_one(&mut env, r#""${unset:-"$'\t'"}""#),
            vec![r#"$'t'"#]
        );
    }

    #[test]
    fn posix_quoted_default_alt_single_quote_does_not_protect_brace() {
        let mut env = TestEnv {
            posix: true,
            ..Default::default()
        };
        env.vars.insert("IFS".into(), " \t\n".into());

        assert_eq!(
            expand_one(&mut env, r#""${IFS+'bar} baz""#),
            vec!["'bar baz"]
        );
    }

    #[test]
    fn quoted_default_alt_line_continuation_is_removed() {
        let mut env = TestEnv {
            posix: true,
            ..Default::default()
        };
        env.vars.insert("IFS".into(), " \t\n".into());

        assert_eq!(
            expand_one(&mut env, "\"${IFS+foo \"b\\\nar\" baz}\""),
            vec!["foo bar baz"]
        );
    }

    #[test]
    fn legacy_dollar_bracket_arithmetic_expands() {
        let mut env = TestEnv::default();

        assert_eq!(expand_one(&mut env, "$[ 13 * 2 ]"), vec!["26"]);
        env.vars.insert("i".into(), "2".into());
        env.arrays
            .insert("a".into(), BTreeMap::from([(2, "20".into())]));
        assert_eq!(expand_one(&mut env, "$[ a[i] + 1 ]"), vec!["21"]);
    }

    #[test]
    fn positional_star_default_and_substring_preserve_empty_and_del() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), String::new()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#"${*-fallback}"#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, r#""${*:1}""#), vec![""]);

        env.positionals = vec!["shell".into(), "\x7f".into()];
        assert_eq!(expand_one(&mut env, r#"${*-fallback}"#), vec!["\x7f"]);
        assert_eq!(expand_one(&mut env, r#""${*:1}""#), vec!["\x7f"]);
    }

    #[test]
    fn quoted_at_preserves_empty_positional() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), String::new()],
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#""$@""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#"${undef-"$@"}"#), vec![""]);

        env.positionals = vec!["shell".into()];
        assert_eq!(expand_one(&mut env, r#""${undef-$@}""#), vec![""]);
        assert_eq!(
            expand_one(&mut env, r#"${undef-"$@"}"#),
            Vec::<String>::new()
        );
        env.vars.insert("empty".into(), String::new());
        env.vars.insert("also_empty".into(), String::new());
        assert_eq!(expand_one(&mut env, r#""$empty$@""#), Vec::<String>::new());
        assert_eq!(
            expand_one(&mut env, r#""$empty$also_empty$@""#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, "\"\"$empty$@"), vec![""]);
    }

    #[test]
    fn quoted_at_alternate_preserves_set_empty_elements() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), String::new()],
            arrays: BTreeMap::from([("a".into(), BTreeMap::from([(0, String::new())]))]),
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#""${@:+nonnull}""#), vec![""]);
        env.positionals = vec!["shell".into()];
        assert_eq!(
            expand_one(&mut env, r#""${@:+nonnull}""#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, r#""${a[@]:+nonnull}""#), vec![""]);
        env.arrays.clear();
        assert_eq!(
            expand_one(&mut env, r#""${a[@]:+nonnull}""#),
            Vec::<String>::new()
        );
    }

    #[test]
    fn simple_numeric_parameter_matches_bash() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "A".into()],
            ..Default::default()
        };
        assert_eq!(expand_one(&mut env, "$10"), vec!["A0"]);

        let mut env = TestEnv {
            positionals: vec!["shell".into()],
            nounset: true,
            ..Default::default()
        };
        let err = crate::expand_word_list(
            &[WordDesc {
                text: "$9".to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner::default(),
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();
        assert!(matches!(err, ExpandError::UnboundVariable(name) if name == "$9"));

        let err = crate::expand_word_list(
            &[WordDesc {
                text: "${9}".to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner::default(),
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();
        assert!(matches!(err, ExpandError::UnboundVariable(name) if name == "9"));
    }

    #[test]
    fn quoted_default_alt_bare_at_preserves_fields() {
        let mut env = TestEnv {
            positionals: vec![
                "shell".into(),
                "a b".into(),
                "c".into(),
                "d".into(),
                "e".into(),
                "f".into(),
            ],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#""${1+$@}""#),
            vec!["a b", "c", "d", "e", "f"]
        );
    }

    #[test]
    fn hash_special_parameter_operators_match_bash() {
        let mut env = TestEnv {
            positionals: vec![
                "shell".into(),
                "1".into(),
                "2".into(),
                "3".into(),
                "4".into(),
                "5".into(),
            ],
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#"${#:foo}"#), vec!["5"]);
        assert_eq!(expand_one(&mut env, r#"${#:-foo}"#), vec!["5"]);
        assert_eq!(expand_one(&mut env, r#"${#-posparams}"#), vec!["5"]);
        assert_eq!(expand_one(&mut env, r#"${#:-posparams}"#), vec!["5"]);

        env.positionals = vec!["shell".into()];
        assert_eq!(expand_one(&mut env, r#"${#:foo}"#), vec!["0"]);
        assert_eq!(expand_one(&mut env, r#"${#:-foo}"#), vec!["0"]);
        assert_eq!(expand_one(&mut env, r#"${#-posparams}"#), vec!["0"]);
        assert_eq!(
            expand_one(&mut env, r#"${!:-posparams}"#),
            vec!["posparams"]
        );
        assert_eq!(expand_one(&mut env, r#"${#-}"#), vec!["0"]);
        assert!(crate::expand_word_list(
            &[WordDesc {
                text: r#"${#:}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner::default(),
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .is_err());
        assert!(crate::expand_word_list(
            &[WordDesc {
                text: r#"${#1xyz}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner::default(),
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .is_err());
    }

    #[test]
    fn literal_ifs_bytes_inside_words_do_not_split() {
        let mut env = TestEnv::default();
        env.vars.insert("IFS".into(), "+".into());
        env.vars.insert("x".into(), "a".into());
        env.vars.insert("y".into(), "b".into());

        assert_eq!(expand_one(&mut env, r#"$x+$y"#), vec!["a+b"]);
        assert_eq!(expand_one(&mut env, r#"+"$@""#), vec!["+"]);

        let mut runner = NullRunner::default();
        let words = expand_word_list(
            &[
                WordDesc {
                    text: "+".to_string(),
                    flags: 0,
                    span: Span::dummy(),
                    raw: None,
                },
                WordDesc {
                    text: r#""$@""#.to_string(),
                    flags: W_HASDOLLAR,
                    span: Span::dummy(),
                    raw: None,
                },
            ],
            &mut env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap();
        assert_eq!(
            words.into_iter().map(|word| word.text).collect::<Vec<_>>(),
            vec!["+"]
        );
    }

    #[test]
    fn ansi_c_quote_is_literal_inside_double_quotes() {
        let mut env = TestEnv::default();

        assert_eq!(expand_one(&mut env, r#""$'\x41'""#), vec![r#"$'\x41'"#]);
    }

    #[test]
    fn command_substitution_heredoc_delimiter_can_touch_closing_paren() {
        let src = b"cat <<EOF\nhere is the text\nEOF)";
        let (body, end) = super::extract_paren(src, 0).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "cat <<EOF\nhere is the text\nEOF"
        );
        assert_eq!(end, src.len());
    }

    #[test]
    fn command_substitution_heredoc_delimiter_removes_escaped_newline() {
        let src = b"cat <<\\EOT\\\n4\nd \\\ng\nEOT4\n)";
        let (body, end) = super::extract_paren(src, 0).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "cat <<\\EOT4\nd \\\ng\nEOT4\n"
        );
        assert_eq!(end, src.len());
    }

    #[test]
    fn command_substitution_double_quotes_track_nested_substitutions() {
        let src = br#"echo "foo$(echo ")")")"#;
        let (body, end) = super::extract_paren(src, 0).unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), r#"echo "foo$(echo ")")""#);
        assert_eq!(end, src.len());
    }

    #[test]
    fn parameter_brace_backticks_protect_closing_brace() {
        let src = br#"HOME:`echo }`}"#;
        let (body, end) = super::extract_brace_body(src, 0, false).unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), r#"HOME:`echo }`"#);
        assert_eq!(end, src.len());
    }

    #[test]
    fn indirect_default_and_special_array_targets_match_bash() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "a".into(), "ef".into(), "op".into()],
            ..Default::default()
        };
        env.vars.insert("z".into(), "abcdefghijklmnop".into());
        env.vars.insert("ef".into(), "4".into());
        env.vars.insert("a".into(), String::new());

        assert_eq!(expand_one(&mut env, "${!9:-$z}"), vec!["abcdefghijklmnop"]);
        assert_eq!(expand_one(&mut env, "${!2}"), vec!["4"]);
        assert_eq!(expand_one(&mut env, "${!#}"), vec!["op"]);
        assert_eq!(expand_one(&mut env, "${!1:-$z}"), vec!["abcdefghijklmnop"]);
        assert_eq!(expand_one(&mut env, "${!1-$z}"), Vec::<String>::new());

        env.positionals = vec!["shell".into(), "a".into(), "b c".into(), "d".into()];
        env.vars.insert("foo".into(), "@".into());
        assert_eq!(expand_one(&mut env, "${!foo}"), vec!["a", "b", "c", "d"]);
        assert_eq!(expand_one(&mut env, r#""${!foo}""#), vec!["a", "b c", "d"]);
    }

    #[test]
    fn default_assignment_rejects_positional_parameters() {
        let mut env = TestEnv::default();
        let mut runner = NullRunner::default();
        let err = crate::expand_word_list(
            &[WordDesc {
                text: r#"${6=arg6}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();

        assert!(matches!(err, ExpandError::Other(msg) if msg == "$6: cannot assign in this way"));
    }

    #[test]
    fn quoted_indirect_prefix_star_and_at_match_bash() {
        let mut env = TestEnv::default();
        env.vars.insert("IFS".into(), "- \t\n".into());
        for name in [
            "_QUANTITY",
            "_QUART",
            "_QUEST",
            "_QUILL",
            "_QUOTA",
            "_QUOTE",
        ] {
            env.vars.insert(name.into(), String::new());
        }

        assert_eq!(
            expand_one(&mut env, r#""${!_Q*}""#),
            vec!["_QUANTITY-_QUART-_QUEST-_QUILL-_QUOTA-_QUOTE"]
        );
        assert_eq!(
            expand_one(&mut env, r#""${!_Q@}""#),
            vec![
                "_QUANTITY",
                "_QUART",
                "_QUEST",
                "_QUILL",
                "_QUOTA",
                "_QUOTE"
            ]
        );
        assert_eq!(expand_one(&mut env, r#"${!*}"#), Vec::<String>::new());

        let mut runner = NullRunner::default();
        let err = crate::expand_word_list(
            &[WordDesc {
                text: r#"${!1*}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();
        assert!(matches!(err, ExpandError::BadSubstitution(msg) if msg == "${!1*}"));
    }

    #[test]
    fn positional_at_pattern_substitution_maps_each_arg() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "uv\x01\x01wx".into(), "uv\x01\x01wx".into()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#"${@/$'\001'/A}"#),
            vec!["uvA\x01wx", "uvA\x01wx"]
        );
        assert_eq!(
            expand_one(&mut env, r#""${@/w/W}""#),
            vec!["uv\x01\x01Wx", "uv\x01\x01Wx"]
        );
    }

    #[test]
    fn positional_star_pattern_substitution_uses_joined_scalar() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "ax".into(), "ay".into()],
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#""${*/a/A}""#), vec!["Ax Ay"]);
        assert_eq!(expand_one(&mut env, r#"${*/a/A}"#), vec!["Ax", "Ay"]);
    }
}
