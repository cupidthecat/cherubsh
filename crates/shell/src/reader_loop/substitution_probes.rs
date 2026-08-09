fn skip_parameter_brace_for_probe(
    bytes: &[u8],
    mut i: usize,
    posix_single_quote: bool,
) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
            b'\'' if !posix_single_quote => {
                i = skip_single_quoted_for_probe(bytes, i)?;
            }
            b'"' => i = skip_simple_quoted_for_probe(bytes, i, bytes[i])?,
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'\'' => {
                i = skip_ansi_c_quoted_for_probe(bytes, i + 2)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                if current_subst_probe_starts(bytes, i + 2) {
                    i = skip_current_subst_brace_for_probe(bytes, i + 2)?;
                } else {
                    depth += 1;
                    i += 2;
                }
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                i = skip_command_substitution_for_probe(bytes, i + 2)?
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn probe_name_is_reserved(bytes: &[u8], start: usize) -> bool {
    start == 0
        || bytes[start - 1].is_ascii_whitespace()
        || matches!(bytes[start - 1], b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}')
}

fn probe_name_is_case_terminator(bytes: &[u8], start: usize) -> bool {
    probe_name_is_reserved(bytes, start)
        && !matches!(previous_probe_significant_byte(bytes, start), Some(b'|' | b'('))
}

fn previous_probe_significant_byte(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i > 0 {
        i -= 1;
        if !bytes[i].is_ascii_whitespace() {
            return Some(bytes[i]);
        }
    }
    None
}

fn has_unclosed_command_substitution(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut case_depth = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'$'
                && i + 1 < bytes.len()
                && bytes[i + 1] == b'{'
                && current_subst_probe_starts(bytes, i + 2)
            {
                let Some(end) = skip_current_subst_brace_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return true;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok && !double {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\'' if !double => {
                    ansi_c = true;
                    i += 2;
                    comment_ok = false;
                    continue;
                }
                b'(' => {
                    depth = depth.saturating_add(1);
                    i += 2;
                    comment_ok = true;
                    continue;
                }
                b'{' if current_subst_probe_starts(bytes, i + 2) => {
                    let Some(end) = skip_current_subst_brace_for_probe(bytes, i + 2) else {
                        return true;
                    };
                    i = end;
                    comment_ok = false;
                    continue;
                }
                _ => {}
            }
        }
        if depth > 0 && is_probe_name_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_probe_name_byte(bytes[i]) {
                i += 1;
            }
            match &bytes[start..i] {
                _ if !probe_name_is_reserved(bytes, start) => {}
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" if probe_name_is_case_terminator(bytes, start) => {
                    case_depth = case_depth.saturating_sub(1)
                }
                _ => {}
            }
            comment_ok = false;
            continue;
        }
        if b == b'(' && depth > 0 {
            depth = depth.saturating_add(1);
            i += 1;
            comment_ok = true;
            continue;
        }
        if b == b')' && depth > 0 {
            if case_depth > 0 && depth == 1 {
                i += 1;
                comment_ok = true;
                continue;
            }
            depth -= 1;
            i += 1;
            comment_ok = true;
            continue;
        }
        match b {
            b'\'' if !double => single = true,
            b'"' => double = !double,
            b'\n' => comment_ok = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    depth > 0
}

fn current_subst_probe_starts(bytes: &[u8], start: usize) -> bool {
    matches!(
        bytes.get(start).copied(),
        Some(b'|') | Some(b' ' | b'\t' | b'\n' | b'\r')
    )
}

fn skip_current_subst_brace_for_probe(bytes: &[u8], mut i: usize) -> Option<usize> {
    if bytes.get(i) == Some(&b'|') {
        i += 1;
    }
    let mut brace_depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
            b'\'' => i = skip_single_quoted_for_probe(bytes, i)?,
            b'"' | b'`' => i = skip_simple_quoted_for_probe(bytes, i, bytes[i])?,
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'\'' => {
                i = skip_ansi_c_quoted_for_probe(bytes, i + 2)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                i = skip_command_substitution_for_probe(bytes, i + 2)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                if current_subst_probe_starts(bytes, i + 2) {
                    i = skip_current_subst_brace_for_probe(bytes, i + 2)?;
                } else {
                    i = skip_parameter_brace_for_probe(bytes, i + 2, false)?;
                }
            }
            b'{' => {
                brace_depth = brace_depth.saturating_add(1);
                i += 1;
            }
            b'}' if brace_depth == 0 => return Some(i + 1),
            b'}' => {
                brace_depth = brace_depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_command_substitution_for_probe(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
    let mut case_depth = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let mut candidate = &bytes[line_start..i];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == delimiter.as_bytes() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if ansi_c {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'\'' {
                ansi_c = false;
            }
            i += 1;
            continue;
        }
        if single {
            if b == b'\'' {
                single = false;
            }
            i += 1;
            continue;
        }
        if double {
            if b == b'\\' {
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                i = skip_command_substitution_for_probe(bytes, i + 2)?;
                continue;
            }
            if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                i = skip_command_substitution_for_probe(bytes, i + 2)?;
                continue;
            }
            if b == b'`' {
                i = skip_backtick_body(bytes, i)?;
                continue;
            }
            if b == b'"' {
                double = false;
            }
            i += 1;
            continue;
        }
        if b == b'`' {
            i = skip_backtick_body(bytes, i)?;
            comment_ok = false;
            continue;
        }
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            comment_ok = false;
            continue;
        }
        if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            i = skip_command_substitution_for_probe(bytes, i + 2)?;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\'' => {
                    ansi_c = true;
                    i += 2;
                    comment_ok = false;
                    continue;
                }
                b'(' => {
                    depth = depth.saturating_add(1);
                    i += 2;
                    comment_ok = true;
                    continue;
                }
                _ => {}
            }
        }
        if is_probe_name_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_probe_name_byte(bytes[i]) {
                i += 1;
            }
            match &bytes[start..i] {
                _ if !probe_name_is_reserved(bytes, start) => {}
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" if probe_name_is_case_terminator(bytes, start) => {
                    case_depth = case_depth.saturating_sub(1)
                }
                _ => {}
            }
            comment_ok = false;
            continue;
        }
        if b == b'(' {
            depth = depth.saturating_add(1);
            i += 1;
            comment_ok = true;
            continue;
        }
        if b == b')' {
            if case_depth > 0 && depth == 1 {
                i += 1;
                comment_ok = true;
                continue;
            }
            depth = depth.saturating_sub(1);
            i += 1;
            if depth == 0 {
                return Some(i);
            }
            comment_ok = true;
            continue;
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    None
}

fn skip_heredoc_redirect_for_probe(
    bytes: &[u8],
    i: &mut usize,
    heredocs: &mut std::collections::VecDeque<(String, bool)>,
) {
    *i += 2;
    let strip_tabs = if *i < bytes.len() && bytes[*i] == b'-' {
        *i += 1;
        true
    } else {
        false
    };
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t') {
        *i += 1;
    }
    let mut delimiter = String::new();
    let mut quote: Option<u8> = None;
    while *i < bytes.len() {
        let b = bytes[*i];
        if let Some(q) = quote {
            *i += 1;
            if b == q {
                quote = None;
            } else {
                delimiter.push(b as char);
            }
            continue;
        }
        if b == b'\'' || b == b'"' {
            quote = Some(b);
            *i += 1;
            continue;
        }
        if b == b'\\' {
            *i += 1;
            if *i < bytes.len() {
                if bytes[*i] == b'\n' {
                    *i += 1;
                    continue;
                }
                delimiter.push(bytes[*i] as char);
                *i += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')') {
            break;
        }
        delimiter.push(b as char);
        *i += 1;
    }
    if strip_tabs {
        delimiter = delimiter.trim_start_matches('\t').to_string();
    }
    if !delimiter.is_empty() {
        heredocs.push_back((delimiter, strip_tabs));
    }
}

fn is_probe_name_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_probe_name_byte(b: u8) -> bool {
    is_probe_name_start(b) || b.is_ascii_digit()
}

fn skip_backtick_body(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}
