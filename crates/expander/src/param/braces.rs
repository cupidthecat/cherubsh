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
        push_parameter_value(ctx, name, quoted, out)?;
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
            return handle_substring(name, &body[p + 1..], ctx, quoted, out);
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
            handle_default_alt(name, op_byte, colon, word, ctx, quoted, out)
        }
        b'#' => {
            let longest = body.get(op_pos) == Some(&b'#');
            let pat_offset = if longest { op_pos + 1 } else { op_pos };
            handle_remove(name, &body[pat_offset..], true, longest, ctx, quoted, out)
        }
        b'%' => {
            let longest = body.get(op_pos) == Some(&b'%');
            let pat_offset = if longest { op_pos + 1 } else { op_pos };
            handle_remove(name, &body[pat_offset..], false, longest, ctx, quoted, out)
        }
        b'/' => handle_patsub(name, &body[op_pos..], ctx, quoted, out),
        b'^' => {
            let all = body.get(op_pos) == Some(&b'^');
            let pat_offset = if all { op_pos + 1 } else { op_pos };
            handle_casemod(
                name,
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
                name,
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
                name,
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
        b'@' => handle_transform(name, &body[op_pos..], ctx, quoted, out),
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
