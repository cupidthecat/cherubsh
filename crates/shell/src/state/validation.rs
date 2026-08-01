fn state_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn state_valid_alias_name(name: &str) -> bool {
    !name.is_empty()
        && (name.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '!' | '%' | ':' | '@')
        }) || state_valid_name(name))
}

fn xtrace_fd_value_is_valid(value: &str) -> bool {
    let Ok(fd) = value.parse::<i32>() else {
        return false;
    };
    fd >= 0 && unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

fn state_array_reference(value: &str) -> Option<(&str, &str)> {
    let open = value.find('[')?;
    let close = value.rfind(']')?;
    if close + 1 != value.len() {
        return None;
    }
    let name = &value[..open];
    if !state_valid_name(name) {
        return None;
    }
    let subscript = &value[open + 1..close];
    if subscript.starts_with(['[', ']']) {
        return None;
    }
    Some((name, subscript))
}

fn state_nameref_target_is_valid(target: &str) -> bool {
    state_valid_name(target) || state_array_reference(target).is_some()
}
