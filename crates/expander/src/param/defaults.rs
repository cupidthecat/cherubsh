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

