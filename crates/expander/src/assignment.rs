use std::borrow::Cow;
use std::collections::BTreeMap;

use cherubsh_common::{
    Environment, Span, VarAttrs, VarKind, W_ASSIGNMENT, W_COMPASSIGN, W_HASDOLLAR,
};
use cherubsh_parser::WordDesc;

use crate::{expand_assignment_rhs, expand_string_to_string, expand_word_list};
use crate::{CommandRunner, ExpandError, ExpandFlags};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpandedAssignment {
    Scalar {
        name: String,
        value: String,
    },
    IndexedElem {
        name: String,
        index: i64,
        value: String,
    },
    AssocElem {
        name: String,
        key: String,
        value: String,
    },
    IndexedArray {
        name: String,
        entries: Vec<(Option<i64>, String)>,
        append: bool,
    },
    AssocArray {
        name: String,
        values: Vec<(String, String)>,
        append: bool,
    },
}

impl ExpandedAssignment {
    pub fn scalar_pair(&self) -> Option<(String, String)> {
        match self {
            Self::Scalar { name, value } => Some((name.clone(), value.clone())),
            _ => None,
        }
    }
}

pub fn looks_like_assignment(word: &WordDesc) -> bool {
    if word.flags & W_ASSIGNMENT != 0 {
        return true;
    }
    split_assignment_text(&word.text)
        .map(|(lhs, _, _)| assignment_lhs_valid(lhs))
        .unwrap_or(false)
}

pub fn expand_assignment_word(
    word: &WordDesc,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Option<ExpandedAssignment>, ExpandError> {
    if compound_parts(&word.text).is_some() {
        return expand_compound_assignment(&word.text, env, runner).map(Some);
    }
    if word.flags & W_COMPASSIGN != 0 && !compound_assignment_has_trailing_text(&word.text) {
        return expand_compound_assignment(&word.text, env, runner).map(Some);
    }

    let Some((lhs, rhs, append)) = split_assignment_text(&word.text) else {
        return Ok(None);
    };

    if let Some((name, subscript)) = array_lhs(lhs) {
        let target = assignment_target_name(env, name);
        let target_name = target.as_ref();
        let value = expand_assignment_rhs(rhs, env, runner)?;
        return if env.kind(target_name) == VarKind::Assoc {
            let key = expand_subscript_text(subscript, env, runner)?.into_owned();
            let value = if append {
                append_value(
                    env.get_array_assoc(target_name, &key).unwrap_or_default(),
                    value,
                    env.attrs(target_name).contains(VarAttrs::INTEGER),
                    env,
                    runner,
                )?
            } else {
                maybe_arith_value(
                    value,
                    env.attrs(target_name).contains(VarAttrs::INTEGER),
                    env,
                    runner,
                )?
            };
            Ok(Some(ExpandedAssignment::AssocElem {
                name: name.to_string(),
                key,
                value,
            }))
        } else {
            let idx_text = expand_subscript_text(subscript, env, runner)?;
            let trimmed = idx_text.trim();
            if subscript.is_empty() || matches!(trimmed, "*" | "@") {
                return Err(ExpandError::Other(format!(
                    "{name}[{trimmed}]: bad array subscript"
                )));
            }
            let index = eval_indexed_subscript_text(&idx_text, env, runner)?;
            let index = normalize_indexed_subscript(
                env,
                target_name,
                index,
                &format!("{name}[{trimmed}]"),
            )?;
            let value = if append {
                append_value(
                    env.get_array_indexed(target_name, index)
                        .unwrap_or_default(),
                    value,
                    env.attrs(target_name).contains(VarAttrs::INTEGER),
                    env,
                    runner,
                )?
            } else {
                maybe_arith_value(
                    value,
                    env.attrs(target_name).contains(VarAttrs::INTEGER),
                    env,
                    runner,
                )?
            };
            Ok(Some(ExpandedAssignment::IndexedElem {
                name: name.to_string(),
                index,
                value,
            }))
        };
    }

    if !is_valid_name(lhs) {
        return Ok(None);
    }
    let target = assignment_target_name(env, lhs);
    let target_name = target.as_ref();
    if target_name != lhs {
        if let Some((name, subscript)) = array_lhs(target_name) {
            let value = expand_assignment_rhs(rhs, env, runner)?;
            return expand_array_element_assignment(name, subscript, value, append, env, runner);
        }
    }
    let mut value = expand_assignment_rhs(rhs, env, runner)?;
    let unresolved_nameref_invalid_target =
        !append && unresolved_nameref(env, lhs) && !assignment_lhs_valid(&value);
    if append {
        let old = if target_name == lhs && env.attrs(lhs).contains(VarAttrs::NAMEREF) {
            String::new()
        } else {
            env.get(target_name).unwrap_or_default()
        };
        value = append_value(
            old,
            value,
            env.attrs(target_name).contains(VarAttrs::INTEGER),
            env,
            runner,
        )?;
    } else if !unresolved_nameref_invalid_target {
        value = maybe_arith_value(
            value,
            env.attrs(target_name).contains(VarAttrs::INTEGER),
            env,
            runner,
        )?;
    }
    Ok(Some(ExpandedAssignment::Scalar {
        name: lhs.to_string(),
        value,
    }))
}

fn expand_array_element_assignment(
    name: &str,
    subscript: &str,
    value: String,
    append: bool,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Option<ExpandedAssignment>, ExpandError> {
    if env.kind(name) == VarKind::Assoc {
        let key = expand_subscript_text(subscript, env, runner)?.into_owned();
        let value = if append {
            append_value(
                env.get_array_assoc(name, &key).unwrap_or_default(),
                value,
                env.attrs(name).contains(VarAttrs::INTEGER),
                env,
                runner,
            )?
        } else {
            maybe_arith_value(
                value,
                env.attrs(name).contains(VarAttrs::INTEGER),
                env,
                runner,
            )?
        };
        return Ok(Some(ExpandedAssignment::AssocElem {
            name: name.to_string(),
            key,
            value,
        }));
    }

    let idx_text = expand_subscript_text(subscript, env, runner)?;
    let trimmed = idx_text.trim();
    if subscript.is_empty() || matches!(trimmed, "*" | "@") {
        return Err(ExpandError::Other(format!(
            "{name}[{trimmed}]: bad array subscript"
        )));
    }
    let index = eval_indexed_subscript_text(&idx_text, env, runner)?;
    let index = normalize_indexed_subscript(env, name, index, &format!("{name}[{trimmed}]"))?;
    let value = if append {
        append_value(
            env.get_array_indexed(name, index).unwrap_or_default(),
            value,
            env.attrs(name).contains(VarAttrs::INTEGER),
            env,
            runner,
        )?
    } else {
        maybe_arith_value(
            value,
            env.attrs(name).contains(VarAttrs::INTEGER),
            env,
            runner,
        )?
    };
    Ok(Some(ExpandedAssignment::IndexedElem {
        name: name.to_string(),
        index,
        value,
    }))
}

fn expand_subscript_text<'a>(
    subscript: &'a str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Cow<'a, str>, ExpandError> {
    if let Some(expanded) = fast_expand_simple_subscript(subscript, env) {
        return Ok(Cow::Owned(expanded));
    }
    let needs_expansion = subscript
        .as_bytes()
        .iter()
        .any(|b| matches!(*b, b'$' | b'`' | b'\\' | b'\'' | b'"'));
    if needs_expansion {
        expand_string_to_string(subscript, env, runner).map(Cow::Owned)
    } else {
        Ok(Cow::Borrowed(subscript))
    }
}

fn fast_expand_simple_subscript(subscript: &str, env: &dyn Environment) -> Option<String> {
    if !subscript.is_ascii() {
        return None;
    }
    let bytes = subscript.as_bytes();
    let body = if bytes.len() >= 2 && bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"') {
        &subscript[1..subscript.len() - 1]
    } else if bytes.iter().any(|b| matches!(*b, b'\'' | b'"')) {
        return None;
    } else {
        subscript
    };
    if body
        .as_bytes()
        .iter()
        .any(|b| matches!(*b, b'\\' | b'\'' | b'"' | b'`' | b'~'))
    {
        return None;
    }
    let body_bytes = body.as_bytes();
    if !body_bytes.contains(&b'$') {
        return (body.len() != subscript.len()).then(|| body.to_string());
    }
    let nounset = env.option("nounset");
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < body_bytes.len() {
        if body_bytes[i] != b'$' {
            out.push(body_bytes[i] as char);
            i += 1;
            continue;
        }
        let Some(next) = body_bytes.get(i + 1).copied() else {
            return None;
        };
        if !(next == b'_' || next.is_ascii_alphabetic()) {
            return None;
        }
        let start = i + 1;
        let mut end = start + 1;
        while end < body_bytes.len()
            && (body_bytes[end] == b'_' || body_bytes[end].is_ascii_alphanumeric())
        {
            end += 1;
        }
        let name = &body[start..end];
        if env.attrs(name).contains(VarAttrs::NAMEREF) {
            return None;
        }
        match env.get_cow(name) {
            Some(value) => out.push_str(value.as_ref()),
            None if nounset => return None,
            None => {}
        }
        i = end;
    }
    Some(out)
}

fn eval_indexed_subscript_text(
    text: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<i64, ExpandError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    if let Some(value) = fast_decimal_arith_value(trimmed, env) {
        return Ok(value);
    }
    crate::arith::eval_preexpanded(text, &mut crate::ExpCtx::new(env, runner))
}

fn fast_decimal_arith_value(expr: &str, env: &dyn Environment) -> Option<i64> {
    if let Some(value) = parse_plain_decimal(expr) {
        return Some(value);
    }
    let first = expr.as_bytes().first().copied()?;
    if !(first == b'_' || first.is_ascii_alphabetic())
        || !expr
            .as_bytes()
            .iter()
            .all(|b| *b == b'_' || b.is_ascii_alphanumeric())
    {
        return None;
    }
    let Some(value) = env.get_cow(expr) else {
        return Some(0);
    };
    let value = value.trim();
    if value.is_empty() {
        return Some(0);
    }
    parse_plain_decimal(value)
}

fn parse_plain_decimal(value: &str) -> Option<i64> {
    let (negative, digits) = value
        .strip_prefix('-')
        .map_or((false, value), |rest| (true, rest));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    let parsed = digits.parse::<i64>().ok()?;
    Some(if negative {
        parsed.wrapping_neg()
    } else {
        parsed
    })
}

fn assignment_target_name<'a>(env: &dyn Environment, name: &'a str) -> Cow<'a, str> {
    if env.attrs(name).contains(VarAttrs::NAMEREF) {
        Cow::Owned(
            env.resolve_nameref(name)
                .unwrap_or_else(|| name.to_string()),
        )
    } else {
        Cow::Borrowed(name)
    }
}

fn unresolved_nameref(env: &dyn Environment, name: &str) -> bool {
    env.attrs(name).contains(VarAttrs::NAMEREF)
        && env
            .nameref_target(name)
            .as_deref()
            .unwrap_or_default()
            .is_empty()
}

fn expand_compound_assignment(
    text: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<ExpandedAssignment, ExpandError> {
    let Some((name, body, append)) = compound_parts(text) else {
        return Err(ExpandError::Other(format!(
            "{text}: invalid compound assignment"
        )));
    };
    if array_lhs(name).is_some() {
        return Err(ExpandError::Other(format!(
            "{name}: cannot assign list to array member"
        )));
    }
    if !is_valid_name(name) {
        return Err(ExpandError::Other(format!(
            "{name}: not a valid identifier"
        )));
    }
    if let Some(token) = invalid_compound_metachar(body) {
        return Err(ExpandError::Other(format!(
            "syntax error near unexpected token `{token}'"
        )));
    }

    let target = assignment_target_name(env, name);
    let target_name = target.as_ref();
    if env.kind(target_name) == VarKind::Assoc {
        let entries = parse_assoc_compound(target_name, body, append, env, runner)?;
        return Ok(ExpandedAssignment::AssocArray {
            name: name.to_string(),
            values: entries,
            append,
        });
    }

    let entries = parse_indexed_compound(target_name, body, append, env, runner)?;
    Ok(ExpandedAssignment::IndexedArray {
        name: name.to_string(),
        entries,
        append,
    })
}

fn normalize_indexed_subscript(
    env: &dyn Environment,
    name: &str,
    index: i64,
    label: &str,
) -> Result<i64, ExpandError> {
    if index >= 0 {
        return Ok(index);
    }
    let max = match env.kind(name) {
        VarKind::Indexed => env.array_max_index(name),
        VarKind::Scalar if env.get(name).is_some() => Some(0),
        _ => None,
    }
    .ok_or_else(|| ExpandError::InvalidArraySubscript(label.to_string()))?;
    let resolved = max + 1 + index;
    if resolved < 0 {
        Err(ExpandError::InvalidArraySubscript(label.to_string()))
    } else {
        Ok(resolved)
    }
}

fn invalid_compound_metachar(body: &str) -> Option<String> {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i = (i + 2).min(bytes.len());
            }
            b'\'' | b'"' => {
                i = skip_quoted(bytes, i, bytes[i]);
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                i = skip_balanced(bytes, i + 2, b'(', b')');
            }
            b'<' | b'>' if bytes.get(i + 1) == Some(&b'(') => {
                i = skip_balanced(bytes, i + 2, b'(', b')');
            }
            b'&' | b'|' | b';' => return Some((bytes[i] as char).to_string()),
            b'<' if bytes.get(i + 1) == Some(&b'>') => return Some("<>".to_string()),
            b'<' | b'>' => return Some((bytes[i] as char).to_string()),
            _ => i += 1,
        }
    }
    None
}

fn skip_quoted(bytes: &[u8], mut i: usize, quote: u8) -> usize {
    i += 1;
    while i < bytes.len() {
        if quote == b'"' && bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
        } else if bytes[i] == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    i
}

fn skip_double_quoted_word(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'$' if bytes.get(i + 1) == Some(&b'{') => i = skip_balanced(bytes, i + 1, b'{', b'}'),
            b'$' if bytes.get(i + 1) == Some(&b'(') => i = skip_balanced(bytes, i + 2, b'(', b')'),
            b'`' => i = skip_backticks(bytes, i),
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

fn skip_backticks(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
        } else if bytes[i] == b'`' {
            return i + 1;
        } else {
            i += 1;
        }
    }
    i
}

fn skip_balanced(bytes: &[u8], mut i: usize, open: u8, close: u8) -> usize {
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' | b'"' => i = skip_quoted(bytes, i, bytes[i]),
            b if b == open => {
                depth += 1;
                i += 1;
            }
            b if b == close => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

fn parse_indexed_compound(
    name: &str,
    body: &str,
    compound_append: bool,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Vec<(Option<i64>, String)>, ExpandError> {
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    let words = lex_words(body);
    let mut out = Vec::new();
    let mut current = BTreeMap::new();
    let mut next = if compound_append {
        env.array_keys(name)
            .and_then(|keys| keys.into_iter().max().map(|max| max + 1))
            .unwrap_or_else(|| if env.get(name).is_some() { 1 } else { 0 })
    } else {
        0
    };
    let arithmetic = env.attrs(name).contains(cherubsh_common::VarAttrs::INTEGER);
    for word in words {
        if let Some((index_raw, value_raw, append)) = bracket_assignment(&word.text) {
            let idx_text = expand_string_to_string(index_raw, env, runner)?;
            let trimmed = idx_text.trim();
            let label = bracket_assignment_label(index_raw, value_raw, append);
            if trimmed.is_empty() {
                report_compound_assignment_error(env, &label, "bad array subscript");
                continue;
            }
            if matches!(trimmed, "*" | "@") {
                report_compound_assignment_error(env, &label, "cannot assign to non-numeric index");
                continue;
            }
            let mut index = match crate::arith::eval_preexpanded(
                &idx_text,
                &mut crate::ExpCtx::new(env, runner),
            ) {
                Ok(index) => index,
                Err(err) => {
                    if compound_subscript_needs_invalid_operator_recovery(&idx_text) {
                        report_compound_arith_invalid_operator(env, &idx_text);
                        continue;
                    }
                    return Err(err);
                }
            };
            if index < 0 {
                if compound_append {
                    if let Some(max) = env.array_max_index(name) {
                        let resolved = max + 1 + index;
                        if resolved >= 0 {
                            index = resolved;
                        } else {
                            report_compound_assignment_error(env, &label, "bad array subscript");
                            continue;
                        }
                    } else {
                        report_compound_assignment_error(env, &label, "bad array subscript");
                        continue;
                    }
                } else {
                    report_compound_assignment_error(env, &label, "bad array subscript");
                    continue;
                }
            }
            let mut value = expand_compound_value(value_raw, env, runner)?;
            if append {
                let old = current.get(&index).cloned().or_else(|| {
                    if compound_append {
                        env.get_array_indexed(name, index)
                    } else {
                        None
                    }
                });
                value = append_value(old.unwrap_or_default(), value, arithmetic, env, runner)?;
            } else {
                value = maybe_arith_value(value, arithmetic, env, runner)?;
            }
            current.insert(index, value.clone());
            next = index + 1;
            out.push((Some(index), value));
            continue;
        }

        let expanded = expand_word_list(
            &[word],
            env,
            runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::EXPAND_GLOB | ExpandFlags::QUOTE_REMOVAL,
        )?;
        for value in expanded {
            let value = maybe_arith_value(value.text, arithmetic, env, runner)?;
            current.insert(next, value.clone());
            next += 1;
            out.push((None, value));
        }
    }
    Ok(out)
}

fn bracket_assignment_label(index_raw: &str, value_raw: &str, append: bool) -> String {
    if append {
        format!("[{index_raw}]+={value_raw}")
    } else {
        format!("[{index_raw}]={value_raw}")
    }
}

fn report_compound_assignment_error(env: &dyn Environment, label: &str, message: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {label}: {message}");
    } else {
        eprintln!("cherubsh: {label}: {message}");
    }
}

fn compound_subscript_needs_invalid_operator_recovery(expr: &str) -> bool {
    crate::quote::shell_string_to_bytes(expr).contains(&0x01)
}

fn report_compound_arith_invalid_operator(env: &dyn Environment, expr: &str) {
    let mut bytes = crate::quote::shell_string_to_bytes(expr);
    bytes.retain(|b| *b != 0x01);
    let token_start = arithmetic_operator_token_start(&bytes);
    let display = String::from_utf8_lossy(&bytes).into_owned();
    let token = String::from_utf8_lossy(&bytes[token_start..]).into_owned();
    let message = format!(
        "{display}: syntax error: invalid arithmetic operator (error token is \"{}\")",
        token.replace('\\', "\\\\").replace('"', "\\\"")
    );
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {message}");
    } else {
        eprintln!("cherubsh: {message}");
    }
}

fn arithmetic_operator_token_start(bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphabetic()) {
        i += 1;
        while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
            i += 1;
        }
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i.min(bytes.len())
}

fn parse_assoc_compound(
    name: &str,
    body: &str,
    compound_append: bool,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<Vec<(String, String)>, ExpandError> {
    let words = lex_words(body);
    let mut out = Vec::new();
    let mut current = BTreeMap::new();
    let mut iter = words.into_iter().peekable();
    let mut bracketed_syntax = None;
    let arithmetic = env.attrs(name).contains(cherubsh_common::VarAttrs::INTEGER);
    while let Some(word) = iter.next() {
        if let Some((key_raw, value_raw, append)) = bracket_assignment(&word.text) {
            bracketed_syntax = Some(true);
            let key = expand_string_to_string(key_raw, env, runner)?;
            if key.is_empty() {
                report_compound_assignment_error(env, "\"\"", "bad array subscript");
                continue;
            }
            let mut value = expand_compound_value(value_raw, env, runner)?;
            if append {
                let old = current.get(&key).cloned().or_else(|| {
                    if compound_append {
                        env.get_array_assoc(name, &key)
                    } else {
                        None
                    }
                });
                value = append_value(old.unwrap_or_default(), value, arithmetic, env, runner)?;
            } else {
                value = maybe_arith_value(value, arithmetic, env, runner)?;
            }
            current.insert(key.clone(), value.clone());
            out.push((key, value));
            continue;
        }

        if bracketed_syntax == Some(true) {
            report_assoc_compound_needs_subscript(env, name, &word.text);
            continue;
        }
        bracketed_syntax = Some(false);
        let key = expand_string_to_string(&word.text, env, runner)?;
        let Some(value_word) = iter.next() else {
            if key.is_empty() {
                report_compound_assignment_error(env, &word.text, "bad array subscript");
                break;
            }
            let value = maybe_arith_value(String::new(), arithmetic, env, runner)?;
            current.insert(key.clone(), value.clone());
            out.push((key, value));
            break;
        };
        if key.is_empty() {
            report_compound_assignment_error(env, &word.text, "bad array subscript");
            continue;
        }
        let value = expand_compound_value(&value_word.text, env, runner)?;
        let value = maybe_arith_value(value, arithmetic, env, runner)?;
        current.insert(key.clone(), value.clone());
        out.push((key, value));
    }
    Ok(out)
}

fn report_assoc_compound_needs_subscript(env: &dyn Environment, name: &str, word: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!(
            "{source}: line {line}: {name}: {word}: must use subscript when assigning associative array"
        );
    } else {
        eprintln!("cherubsh: {name}: {word}: must use subscript when assigning associative array");
    }
}

fn lex_words(body: &str) -> Vec<WordDesc> {
    let mut words = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        let start = i;
        let mut at_word_start = true;
        while i < bytes.len() {
            match bytes[i] {
                b if b.is_ascii_whitespace() => break,
                b'\\' => i = (i + 2).min(bytes.len()),
                b'\'' => {
                    i = skip_quoted(bytes, i, bytes[i]);
                    at_word_start = false;
                }
                b'"' => {
                    i = skip_double_quoted_word(bytes, i);
                    at_word_start = false;
                }
                b'$' if bytes.get(i + 1) == Some(&b'{') => {
                    i = skip_balanced(bytes, i + 1, b'{', b'}');
                    at_word_start = false;
                }
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    i = skip_balanced(bytes, i + 2, b'(', b')');
                    at_word_start = false;
                }
                b'<' | b'>' if bytes.get(i + 1) == Some(&b'(') => {
                    i = skip_balanced(bytes, i + 2, b'(', b')');
                    at_word_start = false;
                }
                b'[' if at_word_start => {
                    if let Some(next) = skip_bracket_assignment(bytes, i) {
                        i = next;
                    } else {
                        i += 1;
                    }
                    at_word_start = false;
                }
                _ => {
                    i += 1;
                    at_word_start = false;
                }
            }
        }

        let text = body[start..i].to_string();
        let mut flags = 0;
        if text.as_bytes().contains(&b'$') {
            flags |= W_HASDOLLAR;
        }
        words.push(WordDesc {
            text,
            flags,
            span: Span::dummy(),
            raw: None,
        });
    }
    words
}

fn trim_compound_value(raw: &str) -> &str {
    let trimmed = raw.trim_end();
    if trimmed.len() == raw.len() || trailing_whitespace_is_escaped(raw, trimmed.len()) {
        raw
    } else {
        trimmed
    }
}

fn expand_compound_value(
    raw: &str,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, ExpandError> {
    let mut value = expand_assignment_rhs(trim_compound_value(raw), env, runner)?;
    if compound_value_has_quoted_at(raw) && value.ends_with(' ') {
        value.pop();
    }
    Ok(value)
}

fn compound_value_has_quoted_at(raw: &str) -> bool {
    raw.contains('@')
}

fn trailing_whitespace_is_escaped(raw: &str, ws_start: usize) -> bool {
    if ws_start == 0 {
        return false;
    }
    let bytes = raw.as_bytes();
    let mut slash_count = 0usize;
    let mut i = ws_start;
    while i > 0 && bytes[i - 1] == b'\\' {
        slash_count += 1;
        i -= 1;
    }
    slash_count % 2 == 1
}

fn skip_bracket_assignment(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut i = start + 1;
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => i = skip_quoted(bytes, i, bytes[i]),
            b'"' => i = skip_double_quoted_word(bytes, i),
            b'$' if bytes.get(i + 1) == Some(&b'{') => i = skip_balanced(bytes, i + 1, b'{', b'}'),
            b'$' if bytes.get(i + 1) == Some(&b'(') => i = skip_balanced(bytes, i + 2, b'(', b')'),
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return None;
    }
    if bytes.get(i) == Some(&b'=') {
        return Some(i + 1);
    }
    if bytes.get(i) == Some(&b'+') && bytes.get(i + 1) == Some(&b'=') {
        return Some(i + 2);
    }
    None
}

fn compound_parts(text: &str) -> Option<(&str, &str, bool)> {
    let (pos, append) = if let Some(pos) = text.find("+=(") {
        (pos, true)
    } else {
        (text.find("=(")?, false)
    };
    let name = &text[..pos];
    if !text.ends_with(')') {
        return None;
    }
    let body_start = if append { pos + 3 } else { pos + 2 };
    Some((name, &text[body_start..text.len() - 1], append))
}

fn compound_assignment_has_trailing_text(text: &str) -> bool {
    let (pos, body_start) = if let Some(pos) = text.find("+=(") {
        (pos, pos + 3)
    } else if let Some(pos) = text.find("=(") {
        (pos, pos + 2)
    } else {
        return false;
    };
    if pos == 0 {
        return false;
    }
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    let mut i = body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' | b'"' => i = skip_quoted(bytes, i, bytes[i]),
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i < bytes.len();
                }
            }
            _ => i += 1,
        }
    }
    false
}

fn split_assignment_text(text: &str) -> Option<(&str, &str, bool)> {
    let (pos, append) = assignment_operator(text)?;
    if append {
        Some((&text[..pos - 1], &text[pos + 1..], true))
    } else {
        Some((&text[..pos], &text[pos + 1..], false))
    }
}

fn assignment_operator(text: &str) -> Option<(usize, bool)> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                if let Some(next) = skip_assignment_subscript(bytes, i) {
                    i = next;
                } else {
                    return None;
                }
            }
            b'=' => return Some((i, i > 0 && bytes.get(i - 1) == Some(&b'+'))),
            b'\'' => i = skip_quoted(bytes, i, b'\''),
            b'"' => i = skip_double_quoted_word(bytes, i),
            b'\\' => i = (i + 2).min(bytes.len()),
            _ => i += 1,
        }
    }
    None
}

fn skip_assignment_subscript(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'\'' => i = skip_quoted(bytes, i, b'\''),
            b'"' => i = skip_double_quoted_word(bytes, i),
            b'\\' => i = (i + 2).min(bytes.len()),
            b'$' if bytes.get(i + 1) == Some(&b'(') => i = skip_balanced(bytes, i + 2, b'(', b')'),
            _ => i += 1,
        }
    }
    None
}

fn assignment_lhs_valid(lhs: &str) -> bool {
    is_valid_name(lhs) || array_lhs(lhs).is_some()
}

fn array_lhs(lhs: &str) -> Option<(&str, &str)> {
    let open = lhs.find('[')?;
    let close = lhs.rfind(']')?;
    if close + 1 != lhs.len() {
        return None;
    }
    let name = &lhs[..open];
    if !is_valid_name(name) {
        return None;
    }
    Some((name, &lhs[open + 1..close]))
}

fn bracket_assignment(text: &str) -> Option<(&str, &str, bool)> {
    if !text.starts_with('[') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut i = 1usize;
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'\'' => i = skip_quoted(bytes, i, bytes[i]),
            b'"' => i = skip_double_quoted_word(bytes, i),
            b'$' if bytes.get(i + 1) == Some(&b'{') => i = skip_balanced(bytes, i + 1, b'{', b'}'),
            b'$' if bytes.get(i + 1) == Some(&b'(') => i = skip_balanced(bytes, i + 2, b'(', b')'),
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let close = i;
                    i += 1;
                    if bytes.get(i) == Some(&b'=') {
                        return Some((&text[1..close], &text[i + 1..], false));
                    }
                    if bytes.get(i) == Some(&b'+') && bytes.get(i + 1) == Some(&b'=') {
                        return Some((&text[1..close], &text[i + 2..], true));
                    }
                    return None;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn maybe_arith_value(
    value: String,
    arithmetic: bool,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, ExpandError> {
    if !arithmetic {
        return Ok(value);
    }
    if value.is_empty() {
        return Ok("0".to_string());
    }
    let value = crate::arith::eval_preexpanded(&value, &mut crate::ExpCtx::new(env, runner))?;
    Ok(value.to_string())
}

fn append_value(
    old: String,
    rhs: String,
    arithmetic: bool,
    env: &mut dyn Environment,
    runner: &mut dyn CommandRunner,
) -> Result<String, ExpandError> {
    if old.is_empty() {
        return Ok(rhs);
    }
    if arithmetic {
        let expr = format!("({old}) + ({rhs})");
        let value = crate::arith::eval_preexpanded(&expr, &mut crate::ExpCtx::new(env, runner))?;
        return Ok(value.to_string());
    }
    Ok(format!("{old}{rhs}"))
}

fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[allow(dead_code)]
fn dummy_word(text: String) -> WordDesc {
    WordDesc {
        text,
        flags: 0,
        span: Span::dummy(),
        raw: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{bracket_assignment, invalid_compound_metachar, lex_words, split_assignment_text};

    #[test]
    fn compound_assignment_rejects_unquoted_metacharacters() {
        assert_eq!(
            invalid_compound_metachar("first & second"),
            Some("&".into())
        );
        assert_eq!(invalid_compound_metachar("[1]=<>"), Some("<>".into()));
        assert_eq!(invalid_compound_metachar(r#""<>" a\&b <(echo hi)"#), None);
    }

    #[test]
    fn bracket_assignment_skips_quoted_closing_bracket() {
        assert_eq!(
            bracket_assignment(r#"["a]a"]=abc"#),
            Some((r#""a]a""#, "abc", false))
        );
        assert_eq!(
            bracket_assignment(r#"[$(echo ])]=def"#),
            Some(("$(echo ])", "def", false))
        );
    }

    #[test]
    fn assignment_split_skips_equals_inside_subscript() {
        assert_eq!(
            split_assignment_text(r#"myarray['a]=test2;#a']="def""#),
            Some((r#"myarray['a]=test2;#a']"#, r#""def""#, false))
        );
        assert_eq!(
            split_assignment_text(r#"myarray["foo[bar"]=bleh"#),
            Some((r#"myarray["foo[bar"]"#, "bleh", false))
        );
    }

    #[test]
    fn compound_assignment_ignores_comments_between_words() {
        let words = lex_words("[1]=x # comment words\n[2]=y a#b");
        let texts = words.into_iter().map(|w| w.text).collect::<Vec<_>>();
        assert_eq!(texts, vec!["[1]=x", "[2]=y", "a#b"]);
    }

    #[test]
    fn compound_value_trim_keeps_escaped_space_only() {
        assert_eq!(super::trim_compound_value(r"\ "), r"\ ");
        assert_eq!(super::trim_compound_value(r"\\ "), r"\\");
        assert_eq!(super::trim_compound_value(r#""${a[@]}" "#), r#""${a[@]}""#);
        assert_eq!(super::trim_compound_value(r#"$'x '"#), r#"$'x '"#);
        assert!(super::compound_value_has_quoted_at(r#""${a[@]}""#));
        assert!(super::compound_value_has_quoted_at(r#""$@""#));
        assert!(!super::compound_value_has_quoted_at(r#""${a[*]}""#));
    }
}
