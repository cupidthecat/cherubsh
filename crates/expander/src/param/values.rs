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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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
