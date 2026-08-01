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
        return value.to_string();
    }
    let bytes = quote::shell_string_to_bytes(value);
    if shell_string_needs_ansi_quote(value, &bytes) {
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

