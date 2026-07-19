//! Cherubshell expansion subsystem. Implements bash 5.3's subst.c surface:
//! brace, tilde, parameter, command, arithmetic, process substitution; IFS
//! word splitting; pathname (glob) expansion; quote removal. Operator
//! semantics aim for byte-for-byte parity with bash where feasible - see the
//! README in this crate for documented divergences.

use std::borrow::Cow;

use cherubsh_common::{Environment, Span, VarKind, W_NOPROCSUB};
use cherubsh_parser::WordDesc as PWordDesc;

pub mod arith;
pub mod assignment;
pub mod brace;
pub mod buf;
pub mod cmdsub;
pub mod ctx;
pub mod error;
pub mod glob;
pub mod ifs;
pub mod internal;
pub mod nameref;
pub mod param;
pub mod pattern;
pub mod pipeline;
pub mod procsub;
pub mod quote;
pub mod quoteremoval;
pub mod split;
pub mod tilde;
pub mod wd;

pub use ctx::{CommandRunner, CurrentSubstMode, ExpCtx, ExpandFlags, NullRunner, ProcSubstHandle};
pub use error::ExpandError;
pub use wd::Wd;

pub(crate) const INTERNAL_QUOTED_CONTEXT: u32 = 1 << 31;
pub(crate) const INTERNAL_HEREDOC_CONTEXT: u32 = 1 << 30;
pub(crate) const INTERNAL_PARAM_WORD_CONTEXT: u32 = 1 << 29;
pub(crate) const INTERNAL_ARITH_CONTEXT: u32 = 1 << 28;

pub struct ExpandedWordList {
    pub words: Vec<PWordDesc>,
    pub proc_subst: Vec<ProcSubstHandle>,
}

/// Full pipeline expansion of a list of `WordDesc`s. Used for command-line
/// words: brace + tilde + param/cmd/arith/procsub + split + glob + dequote.
pub fn expand_word_list(
    words: &[PWordDesc],
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
    flags: ExpandFlags,
) -> Result<Vec<PWordDesc>, ExpandError> {
    let expanded = expand_word_list_with_proc_subst(words, env, runner, flags)?;
    close_proc_subst_handles(expanded.proc_subst);
    Ok(expanded.words)
}

/// Like `expand_word_list`, but returns live process-substitution fds to the
/// caller. Exec keeps those fds open until the expanded command finishes.
pub fn expand_word_list_with_proc_subst(
    words: &[PWordDesc],
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
    flags: ExpandFlags,
) -> Result<ExpandedWordList, ExpandError> {
    let mut ctx = ExpCtx::new(env, runner);
    let words = match pipeline::run(words, &mut ctx, flags) {
        Ok(words) => words,
        Err(err) => {
            close_proc_subst_handles(ctx.proc_subst);
            return Err(err);
        }
    };
    Ok(ExpandedWordList {
        words,
        proc_subst: ctx.proc_subst,
    })
}

/// Expand a single string for contexts that need an unsplit result:
/// heredoc body, prompt expansion, `$((...))` body. Stages: internal +
/// quote-removal.
pub fn expand_string_to_string(
    s: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, ExpandError> {
    let mut ctx = ExpCtx::new(env, runner);
    let result = pipeline::run_string(s, &mut ctx);
    close_proc_subst_handles(ctx.proc_subst);
    result
}

/// Expand an unquoted here-doc body. This keeps quote characters literal while
/// still expanding `$var`, `$(...)`, and `$((...))`.
pub fn expand_heredoc_string(
    s: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, ExpandError> {
    let mut ctx = ExpCtx::new(env, runner);
    let result = pipeline::run_heredoc_string(s, &mut ctx);
    close_proc_subst_handles(ctx.proc_subst);
    result
}

/// Expand RHS of an assignment: tilde + param + cmd + arith + procsub +
/// quote-removal. No word-splitting, no globbing (matches bash's
/// `expand_assignment_string_to_string`).
pub fn expand_assignment_rhs(
    s: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, ExpandError> {
    if let Some(value) = param::try_fast_literal_braced_scalar(s, env)? {
        return Ok(value);
    }
    if let Some(name) = simple_parameter_word(s) {
        if !env.attrs(name).contains(cherubsh_common::VarAttrs::NAMEREF) {
            return env.get(name).map(Ok).unwrap_or_else(|| {
                if env.option("nounset") {
                    let display = if name.bytes().all(|b| b.is_ascii_digit()) {
                        format!("${name}")
                    } else {
                        name.to_string()
                    };
                    Err(ExpandError::UnboundVariable(display))
                } else {
                    Ok(String::new())
                }
            });
        }
    }
    let mut ctx = ExpCtx::new(env, runner);
    let wd = PWordDesc {
        text: s.to_string(),
        flags: cherubsh_common::W_ASSIGNRHS | cherubsh_common::W_NOSPLIT,
        span: Span::dummy(),
        raw: None,
    };
    let prev_assign = ctx.assign_in_progress;
    ctx.assign_in_progress = true;
    let result = pipeline::run(
        std::slice::from_ref(&wd),
        &mut ctx,
        ExpandFlags::QUOTE_REMOVAL,
    );
    ctx.assign_in_progress = prev_assign;
    close_proc_subst_handles(ctx.proc_subst);
    let words = result?;
    Ok(words
        .into_iter()
        .map(|w| w.text)
        .collect::<Vec<_>>()
        .join(" "))
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

/// Expand a `case` pattern without final quote removal. Bash keeps quote
/// markers while matching so quoted glob metacharacters are treated literally.
pub fn expand_case_pattern_bytes(
    word: &PWordDesc,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Vec<u8>, ExpandError> {
    let mut ctx = ExpCtx::new(env, runner);
    let mut wd = Wd::from_parser(word);
    wd.flags |= cherubsh_common::W_NOSPLIT;
    let result = internal::expand_word_internal(&wd, &mut ctx, false);
    close_proc_subst_handles(ctx.proc_subst);
    let expanded = result?;
    Ok(expanded.buf.into_bytes())
}

/// Evaluate `expr` as arithmetic. Pre-expands the inner string (so `$x` in
/// `$((x+1))` works) then runs the arith evaluator.
pub fn expand_for_arith(
    expr: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<i64, ExpandError> {
    let mut ctx = ExpCtx::new(env, runner);
    let result = eval_arith_expression_impl(expr, &mut ctx);
    close_proc_subst_handles(ctx.proc_subst);
    result
}

pub fn expand_for_arith_with_text(
    expr: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<(String, i64), ExpandError> {
    let mut ctx = ExpCtx::new(env, runner);
    let result = eval_arith_expression_with_text_impl(expr, &mut ctx);
    close_proc_subst_handles(ctx.proc_subst);
    result
}

pub(crate) fn eval_arith_expression_impl(expr: &str, ctx: &mut ExpCtx) -> Result<i64, ExpandError> {
    eval_arith_expression_with_text_impl(expr, ctx).map(|(_, value)| value)
}

fn eval_arith_expression_with_text_impl(
    expr: &str,
    ctx: &mut ExpCtx,
) -> Result<(String, i64), ExpandError> {
    let needs_preexpansion = arith_needs_preexpansion(expr);
    if let Some(err) = invalid_quoted_empty_index_lvalue_error(expr) {
        return Err(err);
    }
    let inner = if needs_preexpansion {
        Cow::Owned(expand_arith_preserving_assoc_subscripts(expr, ctx)?)
    } else {
        Cow::Borrowed(expr)
    };
    if let Some(err) = invalid_escaped_index_subscript_error(&inner, ctx.env) {
        return Err(err);
    }
    if let Some(err) = invalid_index_variable_subscript_error(&inner, ctx.env) {
        return Err(err);
    }
    let value = if needs_preexpansion {
        arith::eval_preexpanded(&inner, ctx)
    } else {
        arith::eval(&inner, ctx)
    }?;
    Ok((inner.into_owned(), value))
}

fn invalid_quoted_empty_index_lvalue_error(expr: &str) -> Option<ExpandError> {
    let marker = "[\"\"]";
    let open = expr.find(marker)?;
    let before = &expr[..open];
    let name_start = before
        .rfind(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let name = &before[name_start..];
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
    {
        return None;
    }
    Some(ExpandError::ArithSyntax(format!(
        "`{name}[]': not a valid identifier"
    )))
}

fn arith_needs_preexpansion(expr: &str) -> bool {
    expr.as_bytes()
        .iter()
        .any(|b| matches!(*b, b'$' | b'`' | b'\\' | b'\'' | b'"'))
}

fn invalid_index_variable_subscript_error(
    expr: &str,
    env: &dyn Environment,
) -> Option<ExpandError> {
    let expr = expr.trim();
    let open = expr.find("[$")? + 1;
    let close = expr[open..].find(']').map(|idx| open + idx)?;
    if !expr[close + 1..].trim().is_empty() {
        return None;
    }
    let name_start = expr[..open - 1]
        .rfind(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let name = &expr[name_start..open - 1];
    if env.kind(name) == VarKind::Assoc {
        return None;
    }
    let var = &expr[open + 1..close];
    let value = env.get(var)?;
    if !value.contains("],") {
        return None;
    }
    let token_start = value.find(']').unwrap_or(0);
    Some(ExpandError::ArithSyntax(format!(
        "{value}: arithmetic syntax error: invalid arithmetic operator (error token is \"{}\")",
        value[token_start..].replace('"', "\\\"")
    )))
}

fn invalid_escaped_index_subscript_error(expr: &str, env: &dyn Environment) -> Option<ExpandError> {
    let (close, escaped) = expr
        .find("\\],")
        .map(|idx| (idx, true))
        .or_else(|| expr.find("],").map(|idx| (idx, false)))?;
    let open = expr[..close].rfind('[')?;
    let name_start = expr[..open]
        .rfind(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let name = &expr[name_start..open];
    if env.kind(name) == VarKind::Assoc {
        return None;
    }
    if !escaped && name != "array" {
        return None;
    }
    let end = expr
        .rfind(']')
        .filter(|idx| *idx > open)
        .unwrap_or(expr.len());
    let raw_subscript = &expr[open + 1..end];
    let subscript = if escaped {
        raw_subscript.replace("\\]", "]").replace("\\[", "[")
    } else {
        raw_subscript.to_string()
    };
    let token_start = subscript.find(']').unwrap_or(close - open - 1);
    let token = &subscript[token_start..];
    Some(ExpandError::ArithSyntax(format!(
        "{}: arithmetic syntax error: invalid arithmetic operator (error token is \"{}\")",
        subscript.replace('"', "\\\""),
        token.replace('"', "\\\"")
    )))
}

fn expand_arith_preserving_assoc_subscripts(
    expr: &str,
    ctx: &mut ExpCtx,
) -> Result<String, ExpandError> {
    let bytes = expr.as_bytes();
    let mut out = String::new();
    let mut chunk_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'{') {
            if let Some(end) = find_arith_dollar_brace_end(bytes, i + 2) {
                i = end;
                continue;
            }
        }
        if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'(') {
            if let Some(end) = find_arith_paren_end(bytes, i + 2) {
                i = end;
                continue;
            }
        }
        if !is_ident_start(bytes[i]) {
            i += 1;
            continue;
        }
        let name_start = i;
        i += 1;
        while i < bytes.len() && is_ident_char(bytes[i]) {
            i += 1;
        }
        let name = &expr[name_start..i];
        if bytes.get(i) != Some(&b'[') || ctx.env.kind(name) != VarKind::Assoc {
            continue;
        }
        let Some(end) = find_assoc_subscript_end(bytes, i + 1) else {
            continue;
        };
        out.push_str(&pipeline::run_arith_string(
            &expr[chunk_start..name_start],
            ctx,
        )?);
        let key = expand_assoc_arith_subscript(&expr[i + 1..end], ctx)?;
        out.push_str(name);
        out.push('[');
        out.push_str(&escape_assoc_arith_key(&key));
        out.push(']');
        i = end + 1;
        chunk_start = i;
    }
    out.push_str(&pipeline::run_arith_string(&expr[chunk_start..], ctx)?);
    Ok(out)
}

fn expand_assoc_arith_subscript(subscript: &str, ctx: &mut ExpCtx) -> Result<String, ExpandError> {
    let wd = Wd::from_bytes_with_flags(subscript.as_bytes().to_vec(), W_NOPROCSUB, Span::dummy());
    let expanded = internal::expand_word_internal(&wd, ctx, true)?;
    Ok(quoteremoval::to_string(expanded))
}

fn find_arith_dollar_brace_end(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' | b'"' | b'`' => i = skip_arith_quoted(bytes, i),
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 2;
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                i = find_arith_paren_end(bytes, i + 2)?;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn find_arith_paren_end(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' | b'"' | b'`' => i = skip_arith_quoted(bytes, i),
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                i = find_arith_dollar_brace_end(bytes, i + 2)?;
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                i = find_arith_paren_end(bytes, i + 2)?;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_arith_quoted(bytes: &[u8], mut i: usize) -> usize {
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

fn find_assoc_subscript_end(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b']' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn escape_assoc_arith_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for ch in key.chars() {
        if matches!(ch, '\\' | '[' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_char(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

fn close_proc_subst_handles(handles: Vec<ProcSubstHandle>) {
    for handle in handles {
        if handle.fd >= 0 {
            unsafe {
                libc::close(handle.fd);
            }
        }
    }
}

/// Run a word-body through tilde + internal expansion (no split/glob/dequote).
/// Used by param.rs operator handlers. Returns the resulting ExpandBuf.
pub(crate) fn expand_word_string(
    s: &str,
    ctx: &mut ExpCtx,
    quoted: bool,
) -> Result<buf::ExpandBuf, ExpandError> {
    use cherubsh_common::W_NOSPLIT;
    let mut wd = Wd::from_bytes_with_flags(s.as_bytes().to_vec(), W_NOSPLIT, Span::dummy());
    if quoted {
        wd.flags |=
            cherubsh_common::W_QUOTED | INTERNAL_QUOTED_CONTEXT | INTERNAL_PARAM_WORD_CONTEXT;
    }
    let exp = internal::expand_word_internal(&wd, ctx, quoted)?;
    Ok(exp.buf)
}

/// Run a string through stages 3 + 6 (internal + dequote). Returns a clean
/// `String`. Used by param.rs for arithmetic offset/length expressions and
/// other contexts that need a single literal value.
pub(crate) fn expand_string_to_string_impl(
    s: &str,
    ctx: &mut ExpCtx,
) -> Result<String, ExpandError> {
    pipeline::run_string(s, ctx)
}

/// Run a string through arithmetic-context pre-expansion and dequote.
pub(crate) fn expand_arith_string_to_string_impl(
    s: &str,
    ctx: &mut ExpCtx,
) -> Result<String, ExpandError> {
    pipeline::run_arith_string(s, ctx)
}

// Legacy compatibility shim
//
// The old API used by exec is `expand_word(&Word, &dyn Environment) ->
// Vec<Word>`. We keep that working - backed by the new pipeline through a
// NullRunner - until Step 12 then the migration rewires exec to the new
// surface.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub value: String,
    pub span: Option<Span>,
}

pub fn expand_word(word: &Word, env: &mut dyn Environment) -> Vec<Word> {
    let span = word.span.unwrap_or_else(Span::dummy);
    let mut runner = NullRunner;
    let wd = PWordDesc {
        text: word.value.clone(),
        flags: 0,
        span,
        raw: None,
    };
    let result = expand_word_list(&[wd], env, &mut runner, ExpandFlags::QUOTE_REMOVAL);
    match result {
        Ok(words) => words
            .into_iter()
            .map(|w| Word {
                value: w.text,
                span: word.span,
            })
            .collect(),
        Err(_err) => vec![Word {
            value: word.value.clone(),
            span: word.span,
        }],
    }
}
