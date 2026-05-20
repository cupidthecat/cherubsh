//! Port of `expand_word_internal` (subst.c:10928). Drives a byte-by-byte loop
//! over a word, handling quotes, escapes, tilde, `$`-prefixed substitutions,
//! backtick command substitution, and process substitution.

use cherubsh_common::{W_ASSIGNMENT, W_ASSIGNRHS, W_NOSPLIT};

use crate::buf::{ExpandBuf, CTLNUL};
use crate::cmdsub;
use crate::ctx::ExpCtx;
use crate::error::ExpandError;
use crate::param;
use crate::quote;
use crate::tilde;
use crate::wd::{QuotedState, Wd};

/// Run the inner expansion (no brace, no split, no glob) on a single word.
pub fn expand_word_internal(wd: &Wd, ctx: &mut ExpCtx, no_tilde: bool) -> Result<Wd, ExpandError> {
    ctx.enter()?;
    let result = run(wd, ctx, no_tilde);
    ctx.leave();
    result
}

fn run(wd: &Wd, ctx: &mut ExpCtx, no_tilde: bool) -> Result<Wd, ExpandError> {
    let bytes = wd.buf.as_bytes().to_vec();
    let mut out = Wd::with_capacity(bytes.len());
    out.flags = wd.flags;
    out.span = wd.span;
    let mut i = 0;
    let mut had_quotes = false;
    let all_quoted = true;
    let mut had_unquoted = false;

    // Tilde at word start.
    if !no_tilde && wd.flags & cherubsh_common::W_NOTILDE == 0 && bytes.first() == Some(&b'~') {
        let plen = tilde::prefix_len(&bytes);
        if plen > 0 {
            if let Some(replacement) = tilde::try_expand(&bytes[..plen], ctx.env) {
                for b in replacement {
                    out.buf.push_literal(b);
                }
                i = plen;
            }
        }
    }

    let is_assign_rhs = wd.flags & W_ASSIGNRHS != 0;
    let assignment_word_tilde = wd.flags & W_ASSIGNMENT != 0 && !ctx.env.option("posix");
    let mut assignment_word_eq_seen = false;
    let protect_original_ifs = wd.flags & W_NOSPLIT == 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            if i + 1 < bytes.len() {
                let nx = bytes[i + 1];
                let in_heredoc_context = wd.flags & crate::INTERNAL_HEREDOC_CONTEXT != 0;
                let in_param_word_context = wd.flags & crate::INTERNAL_PARAM_WORD_CONTEXT != 0;
                if wd.flags & crate::INTERNAL_QUOTED_CONTEXT != 0
                    && if in_heredoc_context && !in_param_word_context {
                        !matches!(nx, b'\\' | b'$' | b'`' | b'\n')
                    } else {
                        !matches!(nx, b'"' | b'\\' | b'$' | b'`' | b'\n')
                    }
                {
                    out.buf.push_quoted(b'\\');
                    out.buf.push_quoted(nx);
                    i += 2;
                    had_quotes = true;
                    continue;
                }
                // Outside quotes: backslash escapes any character (including
                // newline - line continuation eats it).
                if nx == b'\n' {
                    i += 2;
                    continue;
                }
                out.buf.push_quoted(nx);
                i += 2;
                had_quotes = true;
                had_unquoted = true;
            } else {
                out.buf.push_quoted_null();
                i += 1;
                had_quotes = true;
            }
            continue;
        }
        if b == b'\'' {
            if wd.flags & crate::INTERNAL_ARITH_CONTEXT != 0 {
                push_original_literal(&mut out.buf, ctx, b, protect_original_ifs);
                i += 1;
                had_unquoted = true;
                continue;
            }
            if wd.flags & crate::INTERNAL_QUOTED_CONTEXT != 0 {
                out.buf.push_quoted(b);
                i += 1;
                had_unquoted = true;
                continue;
            }
            had_quotes = true;
            let (body, end) = quote::scan_single_quoted(&bytes, i + 1);
            if body.is_empty() {
                out.buf.push_quoted_null();
            } else {
                for c in body {
                    out.buf.push_quoted(c);
                }
            }
            i = end;
            continue;
        }
        if b == b'"' {
            if wd.flags & crate::INTERNAL_HEREDOC_CONTEXT != 0
                && wd.flags & crate::INTERNAL_PARAM_WORD_CONTEXT == 0
            {
                out.buf.push_quoted(b);
                i += 1;
                had_unquoted = true;
                continue;
            }
            had_quotes = true;
            i += 1;
            scan_double_quoted_into_mode(
                &bytes,
                &mut i,
                ctx,
                &mut out.buf,
                wd.flags & crate::INTERNAL_QUOTED_CONTEXT != 0,
            )?;
            continue;
        }
        if b == b'$' {
            let mut new_i = i;
            had_unquoted = true;
            let in_heredoc_context = wd.flags & crate::INTERNAL_HEREDOC_CONTEXT != 0;
            let in_quoted_context = wd.flags & crate::INTERNAL_QUOTED_CONTEXT != 0;
            if !in_heredoc_context
                && in_quoted_context
                && wd.flags & crate::INTERNAL_PARAM_WORD_CONTEXT != 0
                && param::dollar_quote_expand(&bytes, &mut new_i, ctx, &mut out.buf)?
            {
                i = new_i;
                continue;
            }
            let handled = param::param_expand(
                &bytes,
                &mut new_i,
                ctx,
                in_heredoc_context || in_quoted_context,
                &mut out.buf,
            )?;
            if handled {
                drop_empty_param_before_dollar_at(&bytes, new_i, ctx, &mut out.buf);
                i = new_i;
                continue;
            }
            if in_heredoc_context {
                out.buf.push_quoted(b);
            } else {
                push_original_literal(&mut out.buf, ctx, b, protect_original_ifs);
            }
            i += 1;
            continue;
        }
        if b == b'`' {
            had_unquoted = true;
            i += 1;
            let body_start = i;
            while i < bytes.len() && bytes[i] != b'`' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let body_bytes = &bytes[body_start..i];
            if i >= bytes.len() {
                if body_bytes.is_empty() {
                    push_original_literal(&mut out.buf, ctx, b'`', protect_original_ifs);
                    continue;
                }
                return Err(ExpandError::BadSubstitution(
                    "no closing ` in command substitution".to_string(),
                ));
            }
            i += 1;
            let body_str = String::from_utf8_lossy(body_bytes);
            let unbash = cmdsub::unbackslash_backticks(&body_str, false);
            let buf = cmdsub::command_substitute(&unbash, ctx, false, true, None)?;
            out.buf.extend_from(&buf);
            continue;
        }
        if (b == b'<' || b == b'>')
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'('
            && wd.flags & cherubsh_common::W_NOPROCSUB == 0
        {
            had_unquoted = true;
            let mut new_i = i;
            param::process_subst_expand(&bytes, &mut new_i, ctx, false, &mut out.buf)?;
            i = new_i;
            continue;
        }
        // Assignment-looking words expand tilde after the first `=` in
        // non-POSIX mode; assignment RHS strings are already sliced after it.
        if assignment_word_tilde && b == b'=' && !assignment_word_eq_seen {
            assignment_word_eq_seen = true;
            push_original_literal(&mut out.buf, ctx, b, protect_original_ifs);
            i += 1;
            if i < bytes.len() && bytes[i] == b'~' {
                let plen = tilde::prefix_len(&bytes[i..]);
                if plen > 0 {
                    if let Some(replacement) = tilde::try_expand(&bytes[i..i + plen], ctx.env) {
                        for r in replacement {
                            out.buf.push_literal(r);
                        }
                        i += plen;
                        continue;
                    }
                }
            }
            continue;
        }
        // Assignment RHS - tilde after unquoted `:`.
        if (is_assign_rhs || assignment_word_tilde) && b == b':' {
            push_original_literal(&mut out.buf, ctx, b, protect_original_ifs);
            i += 1;
            if i < bytes.len() && bytes[i] == b'~' {
                let plen = tilde::prefix_len(&bytes[i..]);
                if plen > 0 {
                    if let Some(replacement) = tilde::try_expand(&bytes[i..i + plen], ctx.env) {
                        for r in replacement {
                            out.buf.push_literal(r);
                        }
                        i += plen;
                        continue;
                    }
                }
            }
            continue;
        }
        push_original_literal(&mut out.buf, ctx, b, protect_original_ifs);
        i += 1;
        had_unquoted = true;
    }
    out.quoted_state = match (had_quotes, had_unquoted) {
        (true, false) => QuotedState::Wholly,
        (true, true) => QuotedState::Partially,
        (false, _) => QuotedState::Unquoted,
    };
    let _ = all_quoted;
    Ok(out)
}

fn push_original_literal(out: &mut ExpandBuf, ctx: &ExpCtx, b: u8, protect_ifs: bool) {
    if protect_ifs && ctx.ifs.is_ifs_nonws(b) && !matches!(b, b'*' | b'?' | b'[') {
        out.push_quoted(b);
    } else {
        out.push_literal(b);
    }
}

/// Scan a double-quoted run starting just past the opening `"`. Pushes into
/// `out` with CTLESC-protection. Stops at the closing `"`. Recognizes `$`,
/// backticks, and standard double-quote escapes.
pub fn scan_double_quoted_into(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    out: &mut ExpandBuf,
) -> Result<(), ExpandError> {
    scan_double_quoted_into_mode(bytes, i, ctx, out, false)
}

fn scan_double_quoted_into_mode(
    bytes: &[u8],
    i: &mut usize,
    ctx: &mut ExpCtx,
    out: &mut ExpandBuf,
    escape_all: bool,
) -> Result<(), ExpandError> {
    let mut any_content = false;
    while *i < bytes.len() && bytes[*i] != b'"' {
        let b = bytes[*i];
        if b == b'\\' && *i + 1 < bytes.len() {
            let n = bytes[*i + 1];
            if n == b'\n' {
                *i += 2;
                continue;
            }
            if escape_all {
                out.push_quoted(n);
                *i += 2;
                any_content = true;
                continue;
            }
            if matches!(n, b'"' | b'\\' | b'$' | b'`') {
                out.push_quoted(n);
                *i += 2;
                any_content = true;
                continue;
            }
            out.push_quoted(b);
            out.push_quoted(n);
            *i += 2;
            any_content = true;
            continue;
        }
        if b == b'$' {
            let mut new_i = *i;
            let handled = crate::param::param_expand(bytes, &mut new_i, ctx, true, out)?;
            if handled {
                drop_empty_param_before_dollar_at(bytes, new_i, ctx, out);
                *i = new_i;
                any_content = true;
                continue;
            }
            out.push_quoted(b);
            *i += 1;
            any_content = true;
            continue;
        }
        if b == b'`' {
            *i += 1;
            let body_start = *i;
            while *i < bytes.len() && bytes[*i] != b'`' {
                if bytes[*i] == b'\\' && *i + 1 < bytes.len() {
                    *i += 2;
                } else {
                    *i += 1;
                }
            }
            let body_bytes = &bytes[body_start..*i];
            if *i >= bytes.len() {
                if body_bytes.is_empty() {
                    out.push_quoted(b'`');
                    any_content = true;
                    continue;
                }
                return Err(ExpandError::BadSubstitution(
                    "no closing ` in command substitution".to_string(),
                ));
            }
            *i += 1;
            let body_str = String::from_utf8_lossy(body_bytes);
            let unbash = cmdsub::unbackslash_backticks(&body_str, true);
            let buf = cmdsub::command_substitute(&unbash, ctx, true, true, None)?;
            out.extend_from(&buf);
            any_content = true;
            continue;
        }
        out.push_quoted(b);
        *i += 1;
        any_content = true;
    }
    if *i < bytes.len() {
        *i += 1; // past closing "
    }
    if !any_content {
        out.push_quoted_null();
    }
    Ok(())
}

fn drop_empty_param_before_dollar_at(bytes: &[u8], next: usize, ctx: &ExpCtx, out: &mut ExpandBuf) {
    if ctx.env.positional_count() != 0 {
        return;
    }
    if !next_is_dollar_at(bytes, next) {
        return;
    }
    while let Some(len) = out.len().checked_sub(1) {
        if out.as_bytes().get(len) != Some(&CTLNUL) {
            break;
        }
        let Some(pos) = out.param_nulls.iter().position(|pos| *pos == len) else {
            break;
        };
        out.param_nulls.remove(pos);
        out.bytes.truncate(len);
    }
    out.had_quoted_null = out.as_bytes().contains(&CTLNUL);
}

fn next_is_dollar_at(bytes: &[u8], next: usize) -> bool {
    if next + 1 >= bytes.len() || bytes[next] != b'$' {
        return false;
    }
    if bytes[next + 1] == b'@' {
        return true;
    }
    next + 3 < bytes.len()
        && bytes[next + 1] == b'{'
        && bytes[next + 2] == b'@'
        && bytes[next + 3] == b'}'
}
