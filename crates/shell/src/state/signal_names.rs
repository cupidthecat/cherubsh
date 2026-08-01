/// Map raw input like "0", "9", "INT", "SIGINT" to a canonical short name
/// usable as a `traps` map key. Reserved names (`EXIT`, `ERR`, `RETURN`,
/// `DEBUG`) pass through.
pub fn canonical_trap_signal(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
    match stripped {
        "0" => return "EXIT".to_string(),
        "EXIT" | "ERR" | "RETURN" | "DEBUG" => return stripped.to_string(),
        _ => {}
    }
    if let Ok(num) = stripped.parse::<i32>() {
        if let Some(short) = signal_short_name(num) {
            return short.to_string();
        }
    }
    stripped.to_string()
}

pub fn signal_short_name(num: i32) -> Option<&'static str> {
    Some(match num {
        1 => "HUP",
        2 => "INT",
        3 => "QUIT",
        4 => "ILL",
        5 => "TRAP",
        6 => "ABRT",
        7 => "BUS",
        8 => "FPE",
        9 => "KILL",
        10 => "USR1",
        11 => "SEGV",
        12 => "USR2",
        13 => "PIPE",
        14 => "ALRM",
        15 => "TERM",
        16 => "STKFLT",
        17 => "CHLD",
        18 => "CONT",
        19 => "STOP",
        20 => "TSTP",
        21 => "TTIN",
        22 => "TTOU",
        23 => "URG",
        24 => "XCPU",
        25 => "XFSZ",
        26 => "VTALRM",
        27 => "PROF",
        28 => "WINCH",
        29 => "IO",
        30 => "PWR",
        31 => "SYS",
        _ => return None,
    })
}

fn signal_number_for_short_name(name: &str) -> Option<i32> {
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "TRAP" => 5,
        "ABRT" => 6,
        "BUS" => 7,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        "STKFLT" => 16,
        "CHLD" => 17,
        "CONT" => 18,
        "STOP" => 19,
        "TSTP" => 20,
        "TTIN" => 21,
        "TTOU" => 22,
        "URG" => 23,
        "XCPU" => 24,
        "XFSZ" => 25,
        "VTALRM" => 26,
        "PROF" => 27,
        "WINCH" => 28,
        "IO" => 29,
        "PWR" => 30,
        "SYS" => 31,
        _ => return None,
    })
}

fn startup_ignored_trap_action(canonical_signal: &str) -> Option<String> {
    let sig = signal_number_for_short_name(canonical_signal)?;
    if sig != libc::SIGPIPE && crate::signals::signal_ignored_at_start(sig) {
        Some(String::new())
    } else {
        None
    }
}

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
