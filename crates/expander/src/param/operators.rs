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
                    out.extend(std::iter::repeat_n(b'\\', count / 2));
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

