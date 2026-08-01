fn history_record_line(state: &ShellState, input_text: &str) -> String {
    let trimmed = input_text.trim_end_matches('\n');
    if contains_heredoc_redirect(trimmed) {
        return input_text.to_string();
    }
    if !state.option("cmdhist")
        || state.option("lithist")
        || !trimmed.contains('\n')
        || newline_inside_quotes(trimmed)
    {
        return trimmed.to_string();
    }
    compact_cmdhist_line(trimmed)
}

fn contains_heredoc_redirect(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            return true;
        }
        i += 1;
    }
    false
}

fn newline_inside_quotes(input: &str) -> bool {
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for byte in input.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some(b'\'') => {
                if byte == b'\n' {
                    return true;
                }
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(b'"') => {
                if byte == b'\n' {
                    return true;
                }
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quote = None;
                }
            }
            _ => {
                if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                }
            }
        }
    }
    false
}

fn compact_cmdhist_line(input: &str) -> String {
    let mut out = String::new();
    let mut previous_trimmed = String::new();
    for raw in input.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(line);
        } else {
            let sep = if matches!(
                previous_trimmed.as_str(),
                "do" | "then" | "else" | "elif" | "{"
            ) || previous_trimmed.ends_with(" do")
                || previous_trimmed.ends_with("; do")
                || previous_trimmed.ends_with(" then")
            {
                " "
            } else {
                "; "
            };
            out.push_str(sep);
            out.push_str(line);
        }
        previous_trimmed = line.trim().to_string();
    }
    out
}

fn should_record_history(state: &ShellState, line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let Some(raw) = state.get("HISTIGNORE") else {
        return true;
    };
    let opts = cherubsh_expander::pattern::GlobOpts {
        extglob: state.option("extglob"),
        globasciiranges: state.option("globasciiranges"),
        ..Default::default()
    };
    for pat in raw.split(':') {
        if pat.is_empty() {
            continue;
        }
        if pat == "&" {
            if state
                .history_table
                .last()
                .map(|entry| entry.line.as_str() == line)
                .unwrap_or(false)
            {
                return false;
            }
            continue;
        }
        if cherubsh_expander::pattern::fnmatch(pat.as_bytes(), line.as_bytes(), opts) {
            return false;
        }
    }
    true
}

