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

include!("constructs.rs");
include!("braces.rs");
include!("defaults.rs");
include!("operators.rs");
include!("transforms.rs");
include!("prompts.rs");
include!("values.rs");
include!("scanning.rs");
include!("public_api.rs");
include!("tests.rs");
