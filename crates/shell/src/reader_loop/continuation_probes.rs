fn parse_error_wants_more_input(err: &ParseError, input: &str) -> bool {
    let at_end = err
        .span
        .as_ref()
        .map(|span| span.end >= input.len())
        .unwrap_or(true);
    at_end
        && (err.message.starts_with("expected")
            || err.message == "function body must be a compound command"
            || err.message == "syntax error: unexpected end of file")
}

fn has_open_quotes(input: &str, posix_mode: bool) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
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
            if b == b'\\' && i + 1 < bytes.len() {
                let n = bytes[i + 1];
                if matches!(n, b'"' | b'\\' | b'$' | b'`' | b'\n') {
                    i += 2;
                    continue;
                }
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if (b == b'<' || b == b'>') && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return true;
                };
                i = end;
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                let Some(end) = skip_parameter_brace_for_probe(bytes, i + 2, posix_mode) else {
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
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
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
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            ansi_c = true;
            i += 2;
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
            let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                return true;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b'\n' => comment_ok = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    single || double || ansi_c
}

fn has_trailing_line_continuation(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    let mut ansi_c = false;
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
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    return i + 2 == bytes.len();
                }
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
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    return i + 2 == bytes.len();
                }
                i += if i + 1 < bytes.len() { 2 } else { 1 };
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                    return false;
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return false;
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
        if b == b'\\' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                return i + 2 == bytes.len();
            }
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return false;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            ansi_c = true;
            i += 2;
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
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b'\n' => comment_ok = true,
            b' ' | b'\t' | b'\r' => comment_ok = true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => comment_ok = true,
            _ => comment_ok = false,
        }
        i += 1;
    }
    false
}

fn has_unclosed_compound_assignment(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0usize;
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

        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            comment_ok = false;
            continue;
        }
        if b == b'\'' || b == b'"' {
            i = skip_simple_quoted_for_probe(bytes, i, b).unwrap_or(bytes.len());
            comment_ok = false;
            continue;
        }
        if b == b'`' {
            let Some(end) = skip_backtick_body(bytes, i) else {
                return false;
            };
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            i = skip_ansi_c_quoted_for_probe(bytes, i + 2).unwrap_or(bytes.len());
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let Some(end) = skip_command_substitution_for_probe(bytes, i + 2) else {
                return false;
            };
            i = end;
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
        if b == b'='
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'('
            && compound_assignment_lhs_before(bytes, i)
            && skip_compound_assignment_body(bytes, i + 2).is_none()
        {
            return true;
        }
        comment_ok = b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>');
        i += 1;
    }

    false
}

fn compound_assignment_lhs_before(bytes: &[u8], eq: usize) -> bool {
    let mut end = eq;
    if end > 0 && bytes[end - 1] == b'+' {
        end -= 1;
    }
    if end == 0 {
        return false;
    }
    if bytes[end - 1] == b']' {
        let Some(open) = bytes[..end - 1].iter().rposition(|b| *b == b'[') else {
            return false;
        };
        return valid_identifier_bytes(&bytes[..open]);
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1] == b'_' || bytes[start - 1].is_ascii_alphanumeric()) {
        start -= 1;
    }
    valid_identifier_bytes(&bytes[start..end])
}

fn valid_identifier_bytes(bytes: &[u8]) -> bool {
    let Some(first) = bytes.first() else {
        return false;
    };
    (*first == b'_' || first.is_ascii_alphabetic())
        && bytes[1..]
            .iter()
            .all(|b| *b == b'_' || b.is_ascii_alphanumeric())
}

fn skip_compound_assignment_body(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += if i + 1 < bytes.len() { 2 } else { 1 },
            b'\'' | b'"' => i = skip_simple_quoted_for_probe(bytes, i, bytes[i])?,
            b'`' => i = skip_backtick_body(bytes, i)?,
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'\'' => {
                i = skip_ansi_c_quoted_for_probe(bytes, i + 2)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                i = skip_parameter_brace_for_probe(bytes, i + 2, false)?
            }
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => {
                i = skip_command_substitution_for_probe(bytes, i + 2)?
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
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

fn skip_simple_quoted_for_probe(bytes: &[u8], mut i: usize, quote: u8) -> Option<usize> {
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn skip_ansi_c_quoted_for_probe(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if bytes[i] == b'\'' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}
