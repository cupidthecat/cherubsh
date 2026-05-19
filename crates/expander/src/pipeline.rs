//! Pipeline orchestrator. Mirrors bash's `expand_words` flow:
//!   1. brace expansion
//!   2. tilde + parameter + command + arithmetic + process substitution
//!   3. word-splitting
//!   4. pathname (glob) expansion
//!   5. quote removal

use cherubsh_common::{
    Span, W_ASSIGNMENT, W_HASDOLLAR, W_NOBRACE, W_NOGLOB, W_NOPROCSUB, W_NOSPLIT,
};
use cherubsh_parser::WordDesc as PWordDesc;

use crate::brace;
use crate::buf::{CTLESC, CTLFIELD, CTLRAW};
use crate::ctx::{ExpCtx, ExpandFlags};
use crate::error::ExpandError;
use crate::glob;
use crate::internal;
use crate::pattern::{has_glob_meta, GlobOpts};
use crate::quoteremoval;
use crate::split;
use crate::wd::Wd;

pub fn run(
    words: &[PWordDesc],
    ctx: &mut ExpCtx,
    flags: ExpandFlags,
) -> Result<Vec<PWordDesc>, ExpandError> {
    let prev_split_fields = ctx.split_fields;
    ctx.split_fields = flags.contains(ExpandFlags::SPLIT_FIELDS);
    let result = run_with_context(words, ctx, flags);
    ctx.split_fields = prev_split_fields;
    result
}

fn run_with_context(
    words: &[PWordDesc],
    ctx: &mut ExpCtx,
    flags: ExpandFlags,
) -> Result<Vec<PWordDesc>, ExpandError> {
    let mut after_brace: Vec<Wd> = Vec::new();
    for w in words {
        if !flags.contains(ExpandFlags::NO_BRACE)
            && w.flags & W_NOBRACE == 0
            && w.flags & W_ASSIGNMENT == 0
        {
            for expanded in brace::brace_expand(w.text.as_bytes()) {
                after_brace.push(Wd::from_bytes_with_flags(expanded, w.flags, w.span));
            }
        } else {
            after_brace.push(Wd::from_parser(w));
        }
    }

    let no_tilde = flags.contains(ExpandFlags::NO_TILDE);

    let mut after_internal: Vec<Wd> = Vec::with_capacity(after_brace.len());
    for w in after_brace {
        let exp = internal::expand_word_internal(&w, ctx, no_tilde)?;
        after_internal.push(exp);
    }

    let after_array_fields = split_field_markers(after_internal);

    let mut after_split: Vec<Wd> = Vec::with_capacity(after_array_fields.len());
    for w in after_array_fields {
        if flags.contains(ExpandFlags::SPLIT_FIELDS)
            && w.flags & W_NOSPLIT == 0
            && w.flags & W_HASDOLLAR != 0
        {
            let mut parts = split::split(&w.buf, &ctx.ifs);
            for p in &mut parts {
                p.flags = w.flags;
                p.span = w.span;
            }
            after_split.extend(parts);
        } else {
            after_split.push(w);
        }
    }

    let mut after_glob: Vec<Wd> = Vec::with_capacity(after_split.len());
    let glob_opts = GlobOpts {
        extglob: ctx.env.option("extglob"),
        nocaseglob: ctx.env.option("nocaseglob"),
        ..Default::default()
    };
    let glob_on = flags.contains(ExpandFlags::EXPAND_GLOB) && !ctx.env.option("noglob");
    for w in after_split {
        if glob_on && w.flags & W_NOGLOB == 0 && has_glob_meta(w.buf.as_bytes(), glob_opts) {
            let g = glob::pathname_expand(&w, ctx.env)?;
            after_glob.extend(g);
        } else {
            after_glob.push(w);
        }
    }

    let do_remove = flags.contains(ExpandFlags::QUOTE_REMOVAL);
    let out: Vec<PWordDesc> = after_glob
        .into_iter()
        .map(|w| {
            if do_remove {
                quoteremoval::to_parser(w)
            } else {
                w.into_parser()
            }
        })
        .collect();
    Ok(out)
}

fn split_field_markers(words: Vec<Wd>) -> Vec<Wd> {
    let mut out = Vec::new();
    for w in words {
        if !w.buf.as_bytes().contains(&CTLFIELD) {
            out.push(w);
            continue;
        }
        let span = w.span;
        let flags = w.flags;
        for part in split_unescaped_field_markers(w.buf.as_bytes()) {
            let mut next = Wd::new();
            next.flags = flags;
            next.span = span;
            if part.is_empty() {
                next.buf.push_quoted_null();
            } else {
                next.buf.bytes = part;
            }
            out.push(next);
        }
    }
    out
}

fn split_unescaped_field_markers(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut parts = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == CTLESC && bytes[i + 1] == CTLRAW {
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
            parts.push(std::mem::take(&mut cur));
            i += 1;
            continue;
        }
        cur.push(bytes[i]);
        i += 1;
    }
    parts.push(cur);
    parts
}

/// Expand an inner string (heredoc body, prompt template, $((...)) inner).
/// Only stages 3 (internal) and 6 (dequote) fire.
pub fn run_string(s: &str, ctx: &mut ExpCtx) -> Result<String, ExpandError> {
    let wd = Wd::from_bytes_with_flags(s.as_bytes().to_vec(), 0, Span::dummy());
    let exp = internal::expand_word_internal(&wd, ctx, true)?;
    Ok(quoteremoval::to_string(exp))
}

/// Expand an unquoted here-doc body. Bash performs parameter, command, and
/// arithmetic expansion here, but shell quotes are literal text.
pub fn run_heredoc_string(s: &str, ctx: &mut ExpCtx) -> Result<String, ExpandError> {
    let wd = Wd::from_bytes_with_flags(
        s.as_bytes().to_vec(),
        crate::INTERNAL_QUOTED_CONTEXT | crate::INTERNAL_HEREDOC_CONTEXT,
        Span::dummy(),
    );
    let prev = ctx.heredoc_context;
    ctx.heredoc_context = true;
    let result = internal::expand_word_internal(&wd, ctx, true).map(quoteremoval::to_string);
    ctx.heredoc_context = prev;
    result
}

/// Expand an arithmetic body. Arithmetic grammar owns `<(` and `>(`, so these
/// must not be interpreted as process substitutions during pre-expansion.
pub fn run_arith_string(s: &str, ctx: &mut ExpCtx) -> Result<String, ExpandError> {
    let wd = Wd::from_bytes_with_flags(
        s.as_bytes().to_vec(),
        W_NOPROCSUB | crate::INTERNAL_ARITH_CONTEXT,
        Span::dummy(),
    );
    let exp = internal::expand_word_internal(&wd, ctx, true)?;
    Ok(quoteremoval::to_string(exp))
}

/// Expand a word inside the context of an operator (e.g. `${var:-WORD}`):
/// runs tilde + internal but does NOT split or glob.
pub fn run_word_inner(s: &str, ctx: &mut ExpCtx, quoted: bool) -> Result<Vec<u8>, ExpandError> {
    let wd = Wd::from_bytes_with_flags(s.as_bytes().to_vec(), 0, Span::dummy());
    let exp = internal::expand_word_internal(&wd, ctx, false)?;
    if quoted {
        Ok(exp.buf.bytes)
    } else {
        Ok(exp.buf.bytes)
    }
}
