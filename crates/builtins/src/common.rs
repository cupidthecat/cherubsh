//! Helpers shared across builtin modules.

use std::ffi::CStr;
use std::io::{self, Write};
use std::path::PathBuf;

use cherubsh_common::{
    AssignError, Environment, VarAttrs, VarKind, W_ASSIGNMENT, W_COMPASSIGN, W_QUOTED,
};
use cherubsh_expander::assignment::ExpandedAssignment;
use cherubsh_expander::{ExpCtx, NullRunner};
use cherubsh_parser::WordDesc;

/// Print error in `cherubsh: <name>: <msg>` form. Returns the supplied
/// status so callers can `return builtin_error(...)`.
pub fn builtin_error(name: &str, msg: &str, status: i32) -> i32 {
    eprintln!("cherubsh: {name}: {msg}");
    status
}

pub fn builtin_usage(name: &str, synopsis: &str) -> i32 {
    eprintln!("cherubsh: {name}: usage: {synopsis}");
    2
}

/// Validate POSIX shell identifier.
pub fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Split a single argument into `(name, value)` if it has the form
/// `NAME=VALUE` and `NAME` is a valid identifier.
pub fn parse_assignment(value: &str) -> Option<(String, String)> {
    parse_assignment_op(value).map(|(name, value, _append)| (name, value))
}

pub fn parse_assignment_op(value: &str) -> Option<(String, String, bool)> {
    let mut parts = value.splitn(2, '=');
    let mut key = parts.next()?;
    let rest = parts.next()?;
    let append = key.ends_with('+');
    if append {
        key = &key[..key.len() - 1];
    }
    if key.is_empty() || !is_valid_name(key) {
        return None;
    }
    Some((key.to_string(), rest.to_string(), append))
}

pub fn split_assignment_op(value: &str) -> Option<(&str, &str, bool)> {
    let (pos, append) = assignment_operator(value)?;
    if append {
        Some((&value[..pos - 1], &value[pos + 1..], true))
    } else {
        Some((&value[..pos], &value[pos + 1..], false))
    }
}

fn assignment_operator(value: &str) -> Option<(usize, bool)> {
    let bytes = value.as_bytes();
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
            b'\'' => i = skip_single_quoted(bytes, i),
            b'"' => i = skip_double_quoted(bytes, i),
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
            b'\'' => i = skip_single_quoted(bytes, i),
            b'"' => i = skip_double_quoted(bytes, i),
            b'\\' => i = (i + 2).min(bytes.len()),
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                i = skip_balanced_pair(bytes, i + 2, b'(', b')')
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_single_quoted(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn skip_double_quoted(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'"' => return i + 1,
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                i = skip_balanced_pair(bytes, i + 2, b'(', b')')
            }
            _ => i += 1,
        }
    }
    i
}

fn skip_balanced_pair(bytes: &[u8], mut i: usize, open: u8, close: u8) -> usize {
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b if b == open && open != close => {
                depth += 1;
                i += 1;
            }
            b if b == close => {
                depth -= 1;
                i += 1;
            }
            b'\'' => i = skip_single_quoted(bytes, i),
            b'"' => i = skip_double_quoted(bytes, i),
            b'\\' => i = (i + 2).min(bytes.len()),
            _ => i += 1,
        }
    }
    i
}

pub fn array_reference(value: &str) -> Option<(&str, &str)> {
    let open = value.find('[')?;
    let close = matching_array_close(value.as_bytes(), open)?;
    if close + 1 != value.len() {
        return None;
    }
    let name = &value[..open];
    if !is_valid_name(name) {
        return None;
    }
    let subscript = &value[open + 1..close];
    if subscript.starts_with(['[', ']']) {
        return None;
    }
    Some((name, subscript))
}

fn matching_array_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            b'\'' => i = skip_single_quoted(bytes, i),
            b'"' => i = skip_double_quoted(bytes, i),
            b'\\' => i = (i + 2).min(bytes.len()),
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                i = skip_balanced_pair(bytes, i + 2, b'(', b')')
            }
            _ => i += 1,
        }
    }
    None
}

pub fn assignment_base_name(lhs: &str) -> Option<&str> {
    if is_valid_name(lhs) {
        Some(lhs)
    } else {
        array_reference(lhs).map(|(name, _)| name)
    }
}

pub fn apply_assignment_arg(env: &mut dyn Environment, arg: &str, compound: bool) -> i32 {
    let word = WordDesc {
        text: arg.to_string(),
        flags: W_ASSIGNMENT | if compound { W_COMPASSIGN } else { 0 },
        span: cherubsh_common::Span::dummy(),
    };
    let mut runner = NullRunner;
    match cherubsh_expander::assignment::expand_assignment_word(&word, env, &mut runner) {
        Ok(Some(assignment)) => apply_expanded_assignment(env, &assignment),
        Ok(None) => 1,
        Err(err) => {
            err.into_shell_error(Some(word.span)).report();
            1
        }
    }
}

pub fn apply_assignment_arg_global(env: &mut dyn Environment, arg: &str, compound: bool) -> i32 {
    let word = WordDesc {
        text: arg.to_string(),
        flags: W_ASSIGNMENT | if compound { W_COMPASSIGN } else { 0 },
        span: cherubsh_common::Span::dummy(),
    };
    let mut runner = NullRunner;
    match cherubsh_expander::assignment::expand_assignment_word(&word, env, &mut runner) {
        Ok(Some(assignment)) => apply_expanded_assignment_global(env, &assignment),
        Ok(None) => 1,
        Err(err) => {
            err.into_shell_error(Some(word.span)).report();
            1
        }
    }
}

pub fn assign_value(
    env: &mut dyn Environment,
    name: &str,
    value: String,
    append: bool,
) -> Result<(), AssignError> {
    let target = assignment_value_target(env, name);
    let attr_name = assignment_attr_name(&target);
    if !append && unresolved_nameref(env, name) && !nameref_target_is_valid(&value) {
        return Err(AssignError::InvalidName(value));
    }
    let mut value = if append {
        if target == name && env.attrs(name).contains(VarAttrs::NAMEREF) {
            append_with_attrs(env, attr_name, String::new(), value)
        } else {
            append_value_for_target(env, &target, value)
        }
    } else {
        value
    };
    if !append && env.attrs(attr_name).contains(VarAttrs::INTEGER) {
        value = arithmetic_value(env, &value);
    }
    env.assign(name, value)
}

pub fn assign_value_global(
    env: &mut dyn Environment,
    name: &str,
    value: String,
    append: bool,
) -> Result<(), AssignError> {
    let target = assignment_value_target(env, name);
    let attr_name = assignment_attr_name(&target);
    if !append && unresolved_nameref(env, name) && !nameref_target_is_valid(&value) {
        return Err(AssignError::InvalidName(value));
    }
    let mut value = if append {
        if target == name && env.attrs(name).contains(VarAttrs::NAMEREF) {
            append_with_attrs(env, attr_name, String::new(), value)
        } else {
            append_value_for_target(env, &target, value)
        }
    } else {
        value
    };
    if !append && env.global_attrs(attr_name).contains(VarAttrs::INTEGER) {
        value = arithmetic_value(env, &value);
    }
    env.assign_global(name, value)
}

pub fn assign_direct_target(
    env: &mut dyn Environment,
    target: &str,
    value: String,
) -> Result<(), AssignError> {
    assign_direct_target_impl(env, target, value, true)
}

pub fn assign_direct_target_with_flags(
    env: &mut dyn Environment,
    target: &str,
    value: String,
    flags: u32,
) -> Result<(), AssignError> {
    assign_direct_target_impl(env, target, value, flags & W_QUOTED == 0)
}

fn assign_direct_target_impl(
    env: &mut dyn Environment,
    target: &str,
    value: String,
    allow_relaxed_assoc: bool,
) -> Result<(), AssignError> {
    if let Some((name, subscript)) = assoc_relaxed_reference(env, target, allow_relaxed_assoc) {
        if env.is_readonly(&name) {
            return Err(AssignError::ReadOnly(name));
        }
        env.set_array_assoc(&name, &subscript, value);
        return Ok(());
    }
    let Some((name, subscript)) = array_reference(target) else {
        if !is_valid_name(target) {
            return Err(AssignError::InvalidName(target.to_string()));
        }
        return env.assign(target, value);
    };
    if env.is_readonly(name) {
        return Err(AssignError::ReadOnly(name.to_string()));
    }
    if env.kind(name) == VarKind::Assoc {
        env.set_array_assoc(name, subscript, value);
        return Ok(());
    }
    let index = resolve_indexed_subscript(env, name, subscript)
        .map_err(|_| AssignError::BadArraySubscript(target.to_string()))?;
    env.set_array_indexed(name, index, value);
    Ok(())
}

pub fn assign_direct_target_op(
    env: &mut dyn Environment,
    target: &str,
    mut value: String,
    append: bool,
) -> Result<(), AssignError> {
    if append {
        let old = if let Some((name, subscript)) = assoc_relaxed_reference(env, target, true) {
            env.get_array_assoc(&name, &subscript).unwrap_or_default()
        } else if let Some((name, subscript)) = array_reference(target) {
            if env.kind(name) == VarKind::Assoc {
                env.get_array_assoc(name, subscript).unwrap_or_default()
            } else {
                match resolve_indexed_subscript(env, name, subscript) {
                    Ok(index) => env.get_array_indexed(name, index).unwrap_or_default(),
                    Err(()) => String::new(),
                }
            }
        } else {
            env.get(target).unwrap_or_default()
        };
        let attr_name = assoc_relaxed_reference(env, target, true)
            .map(|(name, _)| name)
            .or_else(|| array_reference(target).map(|(name, _)| name.to_string()));
        let attr_name = attr_name.as_deref().unwrap_or(target);
        value = if env.attrs(attr_name).contains(VarAttrs::INTEGER) {
            arithmetic_value(env, &format!("({old}) + ({value})"))
        } else if old.is_empty() {
            value
        } else {
            format!("{old}{value}")
        };
    }
    assign_direct_target(env, target, value)
}

pub fn assign_direct_target_op_global(
    env: &mut dyn Environment,
    target: &str,
    value: String,
    append: bool,
) -> Result<(), AssignError> {
    let Some((name, subscript)) = array_reference(target) else {
        if !is_valid_name(target) {
            return Err(AssignError::InvalidName(target.to_string()));
        }
        return assign_value_global(env, target, value, append);
    };
    if env.global_is_readonly(name) {
        return Err(AssignError::ReadOnly(name.to_string()));
    }
    if env.global_kind(name) == VarKind::Assoc {
        env.set_global_array_assoc(name, subscript, value);
        return Ok(());
    }
    let index = subscript.parse::<i64>().unwrap_or(0);
    env.set_global_array_indexed(name, index, value);
    Ok(())
}

fn nameref_array_unset_target(
    env: &dyn Environment,
    name: &str,
    subscript: &str,
) -> Option<String> {
    if !env.attrs(name).contains(VarAttrs::NAMEREF) {
        return None;
    }
    let target = env.resolve_nameref(name)?;
    if target.is_empty() {
        return None;
    }
    if array_reference(&target).is_some() {
        Some(target)
    } else {
        Some(format!("{target}[{subscript}]"))
    }
}

fn resolve_indexed_subscript(
    env: &mut dyn Environment,
    name: &str,
    subscript: &str,
) -> Result<i64, ()> {
    let trimmed = subscript.trim();
    if trimmed.is_empty() || matches!(trimmed, "*" | "@") {
        return Err(());
    }
    let mut runner = NullRunner;
    let index = cherubsh_expander::arith::eval(subscript, &mut ExpCtx::new(env, &mut runner))
        .map_err(|_| ())?;
    if index >= 0 {
        return Ok(index);
    }
    let max = match env.kind(name) {
        VarKind::Indexed => env.array_keys(name).and_then(|keys| keys.into_iter().max()),
        VarKind::Scalar if env.get(name).is_some() => Some(0),
        _ => None,
    }
    .ok_or(())?;
    let resolved = max + 1 + index;
    if resolved < 0 {
        Err(())
    } else {
        Ok(resolved)
    }
}

pub fn unset_array_reference(env: &mut dyn Environment, target: &str) -> Result<bool, String> {
    unset_array_reference_with_options(env, target, false)
}

pub fn unset_array_reference_preserving_arrayref(
    env: &mut dyn Environment,
    target: &str,
) -> Result<bool, String> {
    unset_array_reference_with_options(env, target, true)
}

fn unset_array_reference_with_options(
    env: &mut dyn Environment,
    target: &str,
    preserve_assoc_subscript: bool,
) -> Result<bool, String> {
    if let Some((name, raw_subscript)) = assoc_unset_reference(env, target) {
        if env.is_readonly(&name) {
            return Err(format!("{name}: cannot unset: readonly variable"));
        }
        let subscript = if preserve_assoc_subscript || env.option("assoc_expand_once") {
            raw_subscript
        } else {
            expand_assoc_subscript(env, &raw_subscript)
        };
        env.unset_array_elem(&name, &subscript);
        return Ok(true);
    }
    let Some((name, subscript)) = array_reference(target) else {
        return Ok(false);
    };
    if let Some(resolved_target) = nameref_array_unset_target(env, name, subscript) {
        if resolved_target == target {
            return Ok(true);
        }
        return unset_array_reference_with_options(env, &resolved_target, preserve_assoc_subscript);
    }
    if env.is_readonly(name) {
        return Err(format!("{name}: cannot unset: readonly variable"));
    }
    match env.kind(name) {
        VarKind::Assoc => {
            env.unset_array_elem(name, subscript);
            Ok(true)
        }
        VarKind::Indexed => {
            if subscript == "*" || subscript == "@" {
                if bash_compat_unsets_array_all(env) {
                    env.unset(name);
                    return Ok(true);
                }
                if let Some(keys) = env.array_keys(name) {
                    for key in keys {
                        env.unset_array_elem(name, &key.to_string());
                    }
                }
                env.set_attr(name, VarAttrs::ARRAY, true);
                return Ok(true);
            }
            let index = resolve_indexed_subscript(env, name, subscript)
                .map_err(|_| format!("[{subscript}]: bad array subscript"))?;
            env.unset_array_elem(name, &index.to_string());
            Ok(true)
        }
        VarKind::Scalar => {
            let index = resolve_indexed_subscript(env, name, subscript)
                .map_err(|_| format!("{name}: not an array variable"))?;
            if index == 0 {
                env.unset(name);
                Ok(true)
            } else {
                Err(format!("{name}: not an array variable"))
            }
        }
        VarKind::Unset => Ok(true),
        _ => Err(format!("{name}: not an array variable")),
    }
}

fn bash_compat_unsets_array_all(env: &dyn Environment) -> bool {
    env.get("BASH_COMPAT")
        .and_then(|value| value.parse::<u32>().ok())
        .is_some_and(|compat| compat <= 51)
}

fn assoc_relaxed_reference(
    env: &dyn Environment,
    target: &str,
    allow_relaxed_assoc: bool,
) -> Option<(String, String)> {
    if !allow_relaxed_assoc || !env.option("assoc_expand_once") {
        return None;
    }
    assoc_reference_raw(env, target)
        .map(|(name, subscript)| (name.to_string(), subscript.to_string()))
}

fn assoc_unset_reference(env: &dyn Environment, target: &str) -> Option<(String, String)> {
    assoc_reference_raw(env, target)
        .map(|(name, subscript)| (name.to_string(), subscript.to_string()))
}

fn assoc_reference_raw<'a>(env: &dyn Environment, target: &'a str) -> Option<(&'a str, &'a str)> {
    let open = target.find('[')?;
    if !target.ends_with(']') {
        return None;
    }
    let name = &target[..open];
    if !is_valid_name(name) || !matches!(env.kind(name), VarKind::Assoc) {
        return None;
    }
    let subscript = &target[open + 1..target.len() - 1];
    if subscript.is_empty() {
        return None;
    }
    Some((name, subscript))
}

fn expand_assoc_subscript(env: &mut dyn Environment, subscript: &str) -> String {
    let mut runner = NullRunner;
    match cherubsh_expander::expand_assignment_rhs(subscript, env, &mut runner) {
        Ok(value) => value,
        Err(_) if subscript.contains("$(") || subscript.contains('`') => String::new(),
        Err(_) => subscript.to_string(),
    }
}

pub fn report_assign_error(env: &dyn Environment, err: &AssignError) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        match err {
            AssignError::ReadOnly(name) => {
                eprintln!("{source}: line {line}: {name}: readonly variable");
                return;
            }
            AssignError::BadArraySubscript(name) => {
                eprintln!("{source}: line {line}: {name}: bad array subscript");
                return;
            }
            AssignError::InvalidName(name) => {
                eprintln!("{source}: line {line}: `{name}': not a valid identifier");
                return;
            }
            _ => {}
        }
    }
    err.report();
}

pub fn report_builtin_assign_error(env: &dyn Environment, builtin: &str, err: &AssignError) {
    match err {
        AssignError::InvalidName(name) => {
            report_diagnostic(env, builtin, &format!("`{name}': not a valid identifier"));
        }
        _ => report_assign_error(env, err),
    }
}

pub fn report_builtin_readonly_error(env: &dyn Environment, builtin: &str, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {builtin}: {name}: readonly variable");
    } else {
        eprintln!("cherubsh: {builtin}: {name}: readonly variable");
    }
}

pub fn diagnostic_label(env: &dyn Environment, subject: &str) -> String {
    let subject = diagnostic_subject(subject);
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        format!("{source}: line {line}: {subject}")
    } else {
        format!("cherubsh: {subject}")
    }
}

pub fn report_diagnostic(env: &dyn Environment, subject: &str, message: &str) {
    eprintln!("{}: {message}", diagnostic_label(env, subject));
}

pub fn diagnostic_subject(value: &str) -> String {
    let raw = cherubsh_expander::quote::shell_string_to_bytes(value);
    if raw != value.as_bytes() || std::str::from_utf8(&raw).is_err() {
        ansi_c_quote_bytes(&raw)
    } else {
        value.to_string()
    }
}

pub fn errno_message(err: &io::Error) -> String {
    let Some(errno) = err.raw_os_error() else {
        return err.to_string();
    };
    let ptr = unsafe { libc::strerror(errno) };
    if ptr.is_null() {
        err.to_string()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

fn report_readonly_error(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {name}: readonly variable");
    } else {
        eprintln!("cherubsh: {name}: readonly variable");
    }
}

pub fn apply_expanded_assignment(
    env: &mut dyn Environment,
    assignment: &ExpandedAssignment,
) -> i32 {
    match assignment {
        ExpandedAssignment::Scalar { name, value } => match env.assign(name, value.clone()) {
            Ok(()) => 0,
            Err(err) => {
                report_assign_error(env, &err);
                1
            }
        },
        ExpandedAssignment::IndexedElem { name, index, value } => {
            if env.is_readonly(name) {
                report_readonly_error(env, name);
                return 1;
            }
            env.set_array_indexed(name, *index, value.clone());
            0
        }
        ExpandedAssignment::AssocElem { name, key, value } => {
            if env.is_readonly(name) {
                report_readonly_error(env, name);
                return 1;
            }
            env.set_array_assoc(name, key, value.clone());
            0
        }
        ExpandedAssignment::IndexedArray {
            name,
            entries,
            append,
        } => {
            if env.is_readonly(name) {
                report_readonly_error(env, name);
                return 1;
            }
            if entries.is_empty() && !*append {
                reset_indexed_array_for_assignment(env, name);
                return 0;
            }
            if entries.is_empty() && *append {
                ensure_indexed_array_for_append(env, name);
                return 0;
            }
            let mut next = if *append {
                env.array_keys(name)
                    .and_then(|keys| keys.into_iter().max().map(|max| max + 1))
                    .unwrap_or_else(|| if env.get(name).is_some() { 1 } else { 0 })
            } else {
                reset_indexed_array_for_assignment(env, name);
                0
            };
            for (index, value) in entries {
                match index {
                    Some(index) => {
                        env.set_array_indexed(name, *index, value.clone());
                        next = *index + 1;
                    }
                    None => {
                        env.set_array_indexed(name, next, value.clone());
                        next += 1;
                    }
                }
            }
            0
        }
        ExpandedAssignment::AssocArray {
            name,
            values,
            append,
        } => {
            if env.is_readonly(name) {
                report_readonly_error(env, name);
                return 1;
            }
            let preserve_order = env.take_preserve_assoc_order_for_next_assignment(name);
            let order = preserve_order.then(|| {
                let mut keys = if *append {
                    env.assoc_keys(name).unwrap_or_default()
                } else {
                    Vec::new()
                };
                for (key, _) in values {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
                keys
            });
            if !*append {
                reset_assoc_array_for_assignment(env, name);
            }
            if let Some(keys) = order {
                env.set_assoc_print_order(name, Some(keys));
            } else if !*append {
                env.set_assoc_print_order(name, None);
            }
            if values.is_empty() {
                env.set_array_assoc(name, "", String::new());
                env.unset_array_elem(name, "");
            }
            for (key, value) in values {
                env.set_array_assoc(name, key, value.clone());
            }
            0
        }
    }
}

pub fn apply_expanded_assignment_global(
    env: &mut dyn Environment,
    assignment: &ExpandedAssignment,
) -> i32 {
    match assignment {
        ExpandedAssignment::Scalar { name, value } => {
            match env.assign_global(name, value.clone()) {
                Ok(()) => 0,
                Err(err) => {
                    report_assign_error(env, &err);
                    1
                }
            }
        }
        ExpandedAssignment::IndexedElem { name, index, value } => {
            let target = global_assignment_target(env, name);
            if env.global_is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            env.set_global_array_indexed(&target, *index, value.clone());
            0
        }
        ExpandedAssignment::AssocElem { name, key, value } => {
            let target = global_assignment_target(env, name);
            if env.global_is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            env.set_global_array_assoc(&target, key, value.clone());
            0
        }
        ExpandedAssignment::IndexedArray {
            name,
            entries,
            append,
        } => {
            let target = global_assignment_target(env, name);
            if env.global_is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            if entries.is_empty() && !*append {
                reset_global_indexed_array_for_assignment(env, &target);
                return 0;
            }
            if entries.is_empty() && *append {
                ensure_global_indexed_array_for_append(env, &target);
                return 0;
            }
            let mut next = if *append {
                env.global_array_keys(&target)
                    .and_then(|keys| keys.into_iter().max().map(|max| max + 1))
                    .unwrap_or_else(|| {
                        if env.global_get(&target).is_some() {
                            1
                        } else {
                            0
                        }
                    })
            } else {
                reset_global_indexed_array_for_assignment(env, &target);
                0
            };
            for (index, value) in entries {
                match index {
                    Some(index) => {
                        env.set_global_array_indexed(&target, *index, value.clone());
                        next = *index + 1;
                    }
                    None => {
                        env.set_global_array_indexed(&target, next, value.clone());
                        next += 1;
                    }
                }
            }
            0
        }
        ExpandedAssignment::AssocArray {
            name,
            values,
            append,
        } => {
            let target = global_assignment_target(env, name);
            if env.global_is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            let preserve_order = env.take_preserve_assoc_order_for_next_assignment(&target);
            let order = preserve_order.then(|| {
                let mut keys = if *append {
                    env.assoc_keys(&target).unwrap_or_default()
                } else {
                    Vec::new()
                };
                for (key, _) in values {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
                keys
            });
            if !*append {
                reset_global_assoc_array_for_assignment(env, &target);
            }
            if let Some(keys) = order {
                env.set_assoc_print_order(&target, Some(keys));
            } else if !*append {
                env.set_assoc_print_order(&target, None);
            }
            if values.is_empty() {
                env.set_global_array_assoc(&target, "", String::new());
                env.unset_global_array_elem(&target, "");
            }
            for (key, value) in values {
                env.set_global_array_assoc(&target, key, value.clone());
            }
            0
        }
    }
}

fn global_assignment_target(env: &dyn Environment, name: &str) -> String {
    if env.global_attrs(name).contains(VarAttrs::NAMEREF) {
        env.global_get(name)
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    }
}

fn reset_indexed_array_for_assignment(env: &mut dyn Environment, name: &str) {
    let attrs = env.attrs(name);
    let exported = env.exported(name);
    env.set_array(name, Vec::new());
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_attr(name, attr, true);
        }
    }
    env.set_attr(name, VarAttrs::ARRAY, true);
    if exported {
        env.export(name);
    }
}

fn ensure_indexed_array_for_append(env: &mut dyn Environment, name: &str) {
    let attrs = env.attrs(name);
    let exported = env.exported(name);
    if matches!(env.kind(name), VarKind::Indexed) && env.array_keys(name).is_some() {
        return;
    }
    let values = env.get(name).map(|value| vec![value]).unwrap_or_default();
    env.set_array(name, values);
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_attr(name, attr, true);
        }
    }
    env.set_attr(name, VarAttrs::ARRAY, true);
    if exported {
        env.export(name);
    }
}

fn reset_global_indexed_array_for_assignment(env: &mut dyn Environment, name: &str) {
    let attrs = env.global_attrs(name);
    let exported = env.global_exported(name);
    env.set_global_array(name, Vec::new());
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_global_attr(name, attr, true);
        }
    }
    env.set_global_attr(name, VarAttrs::ARRAY, true);
    if exported {
        env.export_global(name);
    }
}

fn ensure_global_indexed_array_for_append(env: &mut dyn Environment, name: &str) {
    let attrs = env.global_attrs(name);
    let exported = env.global_exported(name);
    if matches!(env.global_kind(name), VarKind::Indexed) && env.global_array_keys(name).is_some() {
        return;
    }
    let values = env
        .global_get(name)
        .map(|value| vec![value])
        .unwrap_or_default();
    env.set_global_array(name, values);
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_global_attr(name, attr, true);
        }
    }
    env.set_global_attr(name, VarAttrs::ARRAY, true);
    if exported {
        env.export_global(name);
    }
}

fn reset_assoc_array_for_assignment(env: &mut dyn Environment, name: &str) {
    let attrs = env.attrs(name);
    let exported = env.exported(name);
    env.unset(name);
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_attr(name, attr, true);
        }
    }
    env.set_attr(name, VarAttrs::ASSOC, true);
    if exported {
        env.export(name);
    }
}

fn reset_global_assoc_array_for_assignment(env: &mut dyn Environment, name: &str) {
    let attrs = env.global_attrs(name);
    let exported = env.global_exported(name);
    env.unset_global(name);
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_global_attr(name, attr, true);
        }
    }
    env.set_global_attr(name, VarAttrs::ASSOC, true);
    if exported {
        env.export_global(name);
    }
}

fn assignment_value_target(env: &dyn Environment, name: &str) -> String {
    if env.attrs(name).contains(VarAttrs::NAMEREF) {
        env.resolve_nameref(name)
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    }
}

fn assignment_attr_name(target: &str) -> &str {
    array_reference(target)
        .map(|(name, _)| name)
        .unwrap_or(target)
}

fn unresolved_nameref(env: &dyn Environment, name: &str) -> bool {
    env.attrs(name).contains(VarAttrs::NAMEREF)
        && env
            .iter_vars()
            .into_iter()
            .find(|snap| snap.name == name)
            .and_then(|snap| snap.nameref_target)
            .as_deref()
            .unwrap_or_default()
            .is_empty()
}

fn nameref_target_is_valid(target: &str) -> bool {
    is_valid_name(target) || array_reference(target).is_some()
}

fn append_value_for_target(env: &mut dyn Environment, target: &str, rhs: String) -> String {
    if let Some((name, subscript)) = array_reference(target) {
        let old = if env.kind(name) == VarKind::Assoc {
            env.get_array_assoc(name, subscript).unwrap_or_default()
        } else {
            match resolve_indexed_subscript(env, name, subscript) {
                Ok(index) => env.get_array_indexed(name, index).unwrap_or_default(),
                Err(()) => String::new(),
            }
        };
        return append_with_attrs(env, name, old, rhs);
    }
    let old = env.get(target).unwrap_or_default();
    append_with_attrs(env, target, old, rhs)
}

fn append_with_attrs(
    env: &mut dyn Environment,
    attr_name: &str,
    old: String,
    rhs: String,
) -> String {
    if old.is_empty() {
        return rhs;
    }
    if env.attrs(attr_name).contains(VarAttrs::INTEGER) {
        return arithmetic_value(env, &format!("({old}) + ({rhs})"));
    }
    format!("{old}{rhs}")
}

fn arithmetic_value(env: &mut dyn Environment, expr: &str) -> String {
    let mut runner = NullRunner;
    let value =
        cherubsh_expander::arith::eval_preexpanded(expr, &mut ExpCtx::new(env, &mut runner))
            .unwrap_or(0);
    value.to_string()
}

/// Quote a value for `declare -p` / `set` / `export -p` output: matches bash
/// `sh_double_quote_word` / single-quote-safe rendering. Always wraps in
/// double quotes and escapes `"`, `$`, `\``, and `\`.
pub fn shell_quote(value: &str) -> String {
    if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return ansi_c_quote(value);
    }

    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Quote a scalar value for `set` assignment output. Bash prints values raw
/// unless they contain shell metacharacters; non-POSIX mode uses ANSI-C quotes
/// for control bytes.
pub fn assignment_quote(value: &str, posix: bool) -> String {
    if !posix && value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return ansi_c_quote(value);
    }
    if contains_shell_metas(value) {
        single_quote_assignment(value)
    } else {
        value.to_string()
    }
}

fn contains_shell_metas(value: &str) -> bool {
    for (idx, ch) in value.chars().enumerate() {
        match ch {
            ' ' | '\t' | '\n' | '\'' | '"' | '\\' | '|' | '&' | ';' | '(' | ')' | '<' | '>'
            | '!' | '{' | '}' | '*' | '[' | '?' | ']' | '^' | '$' | '`' => return true,
            '~' if idx == 0 => return true,
            '#' if idx == 0 => return true,
            _ => {}
        }
    }
    false
}

fn single_quote_assignment(value: &str) -> String {
    if value == "'" {
        return "\\'".to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        out.push(ch);
        if ch == '\'' {
            out.push_str("\\''");
        }
    }
    out.push('\'');
    out
}

/// Quote a string for `${var@Q}` / `printf %q`: single-quote any value
/// containing special characters; otherwise return the bare token.
pub fn ansi_c_quote(value: &str) -> String {
    let raw = cherubsh_expander::quote::shell_string_to_bytes(value);
    if raw != value.as_bytes() || std::str::from_utf8(&raw).is_err() {
        return ansi_c_quote_bytes(&raw);
    }
    if value.is_empty() {
        return "''".to_string();
    }
    let safe = value.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | '-' | '+' | ':' | '@' | ',')
    });
    if safe {
        return value.to_string();
    }
    // Check if any control characters → use $'...' form.
    if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        let mut out = String::from("$'");
        for b in value.bytes() {
            match b {
                b'\'' => out.push_str("\\'"),
                b'\\' => out.push_str("\\\\"),
                b'\n' => out.push_str("\\n"),
                b'\t' => out.push_str("\\t"),
                b'\r' => out.push_str("\\r"),
                b'\x07' => out.push_str("\\a"),
                b'\x08' => out.push_str("\\b"),
                b'\x0c' => out.push_str("\\f"),
                b'\x0b' => out.push_str("\\v"),
                b if b < 0x20 || b == 0x7f => {
                    out.push_str(&format!("\\{:03o}", b));
                }
                b => out.push(b as char),
            }
        }
        out.push('\'');
        out
    } else {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('\'');
        for ch in value.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }
}

fn ansi_c_quote_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "''".to_string();
    }
    let mut out = String::from("$'");
    for &b in bytes {
        match b {
            b'\'' => out.push_str("\\'"),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            b'\x07' => out.push_str("\\a"),
            b'\x08' => out.push_str("\\b"),
            b'\x0c' => out.push_str("\\f"),
            b'\x0b' => out.push_str("\\v"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{:03o}", b)),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::{array_reference, assignment_quote, split_assignment_op};

    #[test]
    fn set_assignment_quote_matches_bash_metas() {
        assert_eq!(assignment_quote("abc", false), "abc");
        assert_eq!(assignment_quote("ab#cd", false), "ab#cd");
        assert_eq!(assignment_quote("#abcd", false), "'#abcd'");
        assert_eq!(assignment_quote("~", false), "'~'");
        assert_eq!(assignment_quote("a b", false), "'a b'");
        assert_eq!(assignment_quote("'", false), "\\'");
        assert_eq!(assignment_quote("a'b", false), "'a'\\''b'");
    }

    #[test]
    fn set_assignment_quote_uses_ansi_for_controls_only_outside_posix() {
        assert_eq!(assignment_quote("i\n", false), "$'i\\n'");
        assert_eq!(assignment_quote("i\n", true), "'i\n'");
    }

    #[test]
    fn assignment_split_skips_equals_inside_subscript() {
        assert_eq!(
            split_assignment_op(r#"myarray['a]=test2;#a']="def""#),
            Some((r#"myarray['a]=test2;#a']"#, r#""def""#, false))
        );
        assert_eq!(
            split_assignment_op(r#"myarray["foo[bar"]=bleh"#),
            Some((r#"myarray["foo[bar"]"#, "bleh", false))
        );
    }

    #[test]
    fn array_reference_rejects_trailing_text_after_close() {
        assert_eq!(array_reference("foo[foo]bar]"), None);
        assert_eq!(array_reference("foo[foo[bar]]"), Some(("foo", "foo[bar]")));
    }
}

/// Look up an executable in $PATH; mirrors `cherubsh-exec::util::search_path`.
pub fn search_path(name: &str, env: &dyn Environment) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        if is_executable(&p) {
            return Some(p);
        }
        return None;
    }
    let path = env.get("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let mut candidate = PathBuf::from(dir);
        candidate.push(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub fn is_executable(path: &std::path::Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(cpath) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    unsafe { libc::access(cpath.as_ptr(), libc::X_OK) == 0 }
}

/// `set` no-arg / `export -p` / `readonly -p` formatting helper: emit a single
/// variable in `declare -<flags> name=value` form.
pub fn format_var(snapshot: &cherubsh_common::VarSnapshot, kind_prefix: Option<&str>) -> String {
    use cherubsh_common::{VarAttrs, VarKind};
    let mut flags = String::new();
    let prefix = kind_prefix.unwrap_or("declare");
    if snapshot.attrs.contains(VarAttrs::ARRAY) || matches!(snapshot.kind, VarKind::Indexed) {
        flags.push('a');
    }
    if snapshot.attrs.contains(VarAttrs::ASSOC) || matches!(snapshot.kind, VarKind::Assoc) {
        flags.push('A');
    }
    if snapshot.attrs.contains(VarAttrs::INTEGER) {
        flags.push('i');
    }
    if snapshot.attrs.contains(VarAttrs::NAMEREF) || matches!(snapshot.kind, VarKind::Nameref) {
        flags.push('n');
    }
    if snapshot.attrs.contains(VarAttrs::READONLY) && prefix != "readonly" {
        flags.push('r');
    }
    if snapshot.attrs.contains(VarAttrs::EXPORT) {
        flags.push('x');
    }
    if snapshot.attrs.contains(VarAttrs::TRACE) {
        flags.push('t');
    }
    if snapshot.attrs.contains(VarAttrs::UPPERCASE) {
        flags.push('u');
    }
    if snapshot.attrs.contains(VarAttrs::LOWERCASE) {
        flags.push('l');
    }
    if snapshot.attrs.contains(VarAttrs::CAPCASE) {
        flags.push('c');
    }
    let flag_str = if flags.is_empty() {
        "--".to_string()
    } else {
        format!("-{flags}")
    };

    match snapshot.kind {
        VarKind::Indexed => {
            let inner = snapshot
                .indexed
                .as_ref()
                .map(|v| {
                    v.iter()
                        .map(|(k, val)| format!("[{k}]={}", shell_quote(val)))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            if inner.is_empty() {
                if snapshot.indexed.is_some() {
                    return format!("{prefix} {flag_str} {}=()", snapshot.name);
                }
                return format!("{prefix} {flag_str} {}", snapshot.name);
            }
            format!("{prefix} {flag_str} {}=({inner})", snapshot.name)
        }
        VarKind::Assoc => {
            let inner = snapshot
                .assoc
                .as_ref()
                .map(|v| {
                    let body = v
                        .iter()
                        .map(|(k, val)| format!("[{}]={}", assoc_key_quote(k), shell_quote(val)))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if body.is_empty() {
                        body
                    } else {
                        format!("{body} ")
                    }
                })
                .unwrap_or_default();
            if inner.is_empty() {
                if snapshot.assoc.is_some() {
                    return format!("{prefix} {flag_str} {}=()", snapshot.name);
                }
                return format!("{prefix} {flag_str} {}", snapshot.name);
            }
            format!("{prefix} {flag_str} {}=({inner})", snapshot.name)
        }
        VarKind::Nameref => {
            let target = snapshot.nameref_target.clone().unwrap_or_default();
            format!(
                "{prefix} {flag_str} {}={}",
                snapshot.name,
                shell_quote(&target)
            )
        }
        VarKind::Scalar => match &snapshot.scalar {
            Some(v) => format!("{prefix} {flag_str} {}={}", snapshot.name, shell_quote(v)),
            None => format!("{prefix} {flag_str} {}", snapshot.name),
        },
        VarKind::Unset => format!("{prefix} {flag_str} {}", snapshot.name),
    }
}

pub fn format_set_var(snapshot: &cherubsh_common::VarSnapshot, posix: bool) -> Option<String> {
    use cherubsh_common::VarKind;
    match snapshot.kind {
        VarKind::Scalar | VarKind::Nameref => snapshot
            .scalar
            .as_ref()
            .map(|v| format!("{}={}", snapshot.name, assignment_quote(v, posix))),
        VarKind::Indexed => {
            let entries = snapshot.indexed.as_ref()?;
            if entries.is_empty() {
                return None;
            }
            let body = entries
                .iter()
                .map(|(k, val)| format!("[{k}]={}", shell_quote(val)))
                .collect::<Vec<_>>()
                .join(" ");
            Some(format!("{}=({body})", snapshot.name))
        }
        VarKind::Assoc => {
            let entries = snapshot.assoc.as_ref()?;
            if entries.is_empty() {
                return None;
            }
            let body = entries
                .iter()
                .map(|(k, val)| format!("[{}]={}", assoc_key_quote(k), shell_quote(val)))
                .collect::<Vec<_>>()
                .join(" ");
            Some(format!("{}=({body} )", snapshot.name))
        }
        VarKind::Unset => None,
    }
}

fn assoc_key_quote(value: &str) -> String {
    if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return ansi_c_quote(value);
    }
    let safe = value.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '/' | '.' | '-' | '+' | ':' | ',' | '=' | '%')
    });
    if safe {
        value.to_string()
    } else {
        shell_quote(value)
    }
}

/// Best-effort stdout flush after a builtin writes (`sh_chkwrite` in bash).
pub fn flush_stdout() -> io::Result<()> {
    io::stdout().lock().flush()
}

pub fn write_str(s: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
}

pub fn writeln_str(s: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
}
