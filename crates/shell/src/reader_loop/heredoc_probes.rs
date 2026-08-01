fn has_unclosed_heredoc(input: &str) -> bool {
    let mut pending: std::collections::VecDeque<(String, bool, bool)> =
        std::collections::VecDeque::new();
    let mut lines = input.lines();
    let mut command_subst_depth = 0usize;
    while let Some(line) = lines.next() {
        if let Some((delimiter, strip_tabs, remove_escaped_newlines)) = pending.front().cloned() {
            if command_subst_depth > 0 {
                command_subst_depth =
                    command_subst_depth_after_line_for_heredoc_probe(line, command_subst_depth);
                continue;
            }
            let mut logical_line = line.to_string();
            if remove_escaped_newlines {
                while logical_line.ends_with('\\') {
                    logical_line.pop();
                    let Some(next) = lines.next() else {
                        return true;
                    };
                    logical_line.push_str(next);
                }
            }
            let candidate = if strip_tabs {
                logical_line.trim_start_matches('\t')
            } else {
                logical_line.as_str()
            };
            if candidate == delimiter.as_str() {
                pending.pop_front();
            }
            continue;
        }
        let mut command_line = line.to_string();
        while command_line.ends_with('\\') {
            command_line.pop();
            let Some(next) = lines.next() else {
                return true;
            };
            command_line.push_str(next);
        }
        command_subst_depth =
            command_subst_depth_after_line_for_heredoc_probe(&command_line, command_subst_depth);
        for spec in heredocs_in_line(&command_line) {
            pending.push_back(spec);
        }
    }
    !pending.is_empty()
}

fn command_subst_depth_after_line_for_heredoc_probe(line: &str, mut depth: usize) -> usize {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut single = false;
    let mut double = false;
    while i < bytes.len() {
        let b = bytes[i];
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if double {
            if b == b'"' {
                double = false;
                i += 1;
                continue;
            }
            if b == b'\\' && i + 1 < bytes.len() {
                let n = bytes[i + 1];
                if matches!(n, b'"' | b'\\' | b'$' | b'`') {
                    i += 2;
                    continue;
                }
            }
        }
        match b {
            b'\'' if !double => {
                single = true;
                i += 1;
            }
            b'"' => {
                double = true;
                i += 1;
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                depth = depth.saturating_add(1);
                i += 2;
            }
            b')' if depth > 0 => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    depth
}

fn top_level_heredocs_in_line_update_depth(
    line: &str,
    depth: &mut usize,
    arithmetic_depth: &mut usize,
) -> Vec<(String, bool, bool)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut single = false;
    let mut double = false;

    while i < bytes.len() {
        let b = bytes[i];
        if *arithmetic_depth > 0 {
            match b {
                b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
                b'\'' | b'"' | b'`' => {
                    if let Some(end) = skip_quoted_for_parse(bytes, i) {
                        i = end;
                    } else {
                        break;
                    }
                }
                b'(' => {
                    *arithmetic_depth = (*arithmetic_depth).saturating_add(1);
                    i += 1;
                }
                b')' => {
                    *arithmetic_depth = (*arithmetic_depth).saturating_sub(1);
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if double {
            match b {
                b'"' => {
                    double = false;
                    i += 1;
                }
                b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                    if i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                        *arithmetic_depth = 2;
                        i += 3;
                    } else if command_substitution_is_arithmetic(bytes, i) {
                        i = skip_arithmetic_substitution_for_parse(bytes, i + 3)
                            .unwrap_or(bytes.len());
                    } else {
                        *depth = (*depth).saturating_add(1);
                        i += 2;
                    }
                }
                b')' if *depth > 0 => {
                    *depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        match b {
            b'\'' => {
                single = true;
                i += 1;
            }
            b'"' => {
                double = true;
                i += 1;
            }
            b'`' => {
                if let Some(end) = skip_backtick_body(bytes, i) {
                    i = end;
                } else {
                    break;
                }
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                if i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                    *arithmetic_depth = 2;
                    i += 3;
                } else if command_substitution_is_arithmetic(bytes, i) {
                    i = skip_arithmetic_substitution_for_parse(bytes, i + 3).unwrap_or(bytes.len());
                } else {
                    *depth = (*depth).saturating_add(1);
                    i += 2;
                }
            }
            b'(' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                *arithmetic_depth = 2;
                i += 2;
            }
            b')' if *depth > 0 => {
                *depth -= 1;
                i += 1;
            }
            b'<' if *depth == 0 && i + 1 < bytes.len() && bytes[i + 1] == b'<' => {
                if i > 0 && bytes[i - 1] == b'<' {
                    i += 1;
                    continue;
                }
                i += 2;
                let strip_tabs = if i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                    true
                } else {
                    false
                };
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                let start = i;
                let mut quote: Option<u8> = None;
                let mut quoted = false;
                let mut text = String::new();
                while i < bytes.len() {
                    let c = bytes[i];
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        } else {
                            text.push(c as char);
                        }
                        i += 1;
                        continue;
                    }
                    if c == b'\'' || c == b'"' {
                        quoted = true;
                        quote = Some(c);
                        i += 1;
                        continue;
                    }
                    if c == b'\\' {
                        quoted = true;
                        i += 1;
                        if i < bytes.len() {
                            text.push(bytes[i] as char);
                            i += 1;
                        }
                        continue;
                    }
                    if c.is_ascii_whitespace()
                        || matches!(c, b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')')
                    {
                        break;
                    }
                    text.push(c as char);
                    i += 1;
                }
                if strip_tabs {
                    text = text.trim_start_matches('\t').to_string();
                }
                if !text.is_empty() || i > start {
                    out.push((text, strip_tabs, !quoted));
                }
            }
            _ => i += 1,
        }
    }

    out
}

fn heredocs_in_line(line: &str) -> Vec<(String, bool, bool)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    while i + 1 < bytes.len() {
        let b = bytes[i];
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if double {
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => {
                single = true;
                i += 1;
            }
            b'"' => {
                double = true;
                i += 1;
            }
            b'<' if bytes[i + 1] == b'<' => {
                if i > 0 && bytes[i - 1] == b'<' {
                    i += 1;
                    continue;
                }
                i += 2;
                let strip_tabs = if i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                    true
                } else {
                    false
                };
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                let start = i;
                let mut quote: Option<u8> = None;
                let mut quoted = false;
                let mut text = String::new();
                while i < bytes.len() {
                    let c = bytes[i];
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        } else {
                            text.push(c as char);
                        }
                        i += 1;
                        continue;
                    }
                    if c == b'\'' || c == b'"' {
                        quoted = true;
                        quote = Some(c);
                        i += 1;
                        continue;
                    }
                    if c == b'\\' {
                        quoted = true;
                        i += 1;
                        if i < bytes.len() {
                            text.push(bytes[i] as char);
                            i += 1;
                        }
                        continue;
                    }
                    if c.is_ascii_whitespace()
                        || matches!(c, b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')')
                    {
                        break;
                    }
                    text.push(c as char);
                    i += 1;
                }
                if strip_tabs {
                    text = text.trim_start_matches('\t').to_string();
                }
                if !text.is_empty() || i > start {
                    out.push((text, strip_tabs, !quoted));
                }
            }
            _ => i += 1,
        }
    }
    out
}

