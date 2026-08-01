fn expand_pattern(raw: &[u8], ctx: &mut ExpCtx) -> Result<Vec<u8>, ExpandError> {
    let s = std::str::from_utf8(raw).unwrap_or("");
    let prev_split_fields = ctx.split_fields;
    ctx.split_fields = false;
    let result = crate::expand_word_string(s, ctx, false);
    ctx.split_fields = prev_split_fields;
    let expanded = result?;
    // Patterns preserve their internal CTLESC markers (quoting denotes literal).
    Ok(expanded.into_bytes())
}

/// Locate the closing `}` for Bash 5.3's current-shell command substitution
/// forms (`${ command; }` and `${| command; }`). Unlike parameter expansion,
/// the body is shell code, so literal function/group braces are balanced too.
fn extract_current_brace_body(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut i = start;
    let mut brace_depth = 0usize;
    let mut body = Vec::new();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && i + 1 < bytes.len() {
            body.push(b);
            body.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if append_ansi_c_literal(bytes, &mut i, &mut body) {
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            append_quoted_bytes(bytes, &mut i, &mut body, b)?;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n == b'{' {
                body.extend_from_slice(b"${");
                i += 2;
                if current_subst_mode(bytes, i).is_some() {
                    let body_start = if bytes[i] == b'|' { i + 1 } else { i };
                    if bytes[i] == b'|' {
                        body.push(b'|');
                    }
                    let (inner, end) = extract_current_brace_body(bytes, body_start)?;
                    body.extend_from_slice(&inner);
                    body.push(b'}');
                    i = end;
                } else {
                    let (inner, end) = extract_brace_body(bytes, i, false)?;
                    body.extend_from_slice(&inner);
                    body.push(b'}');
                    i = end;
                }
                continue;
            }
            if n == b'(' {
                body.extend_from_slice(b"$(");
                i += 2;
                if i < bytes.len() && bytes[i] == b'(' {
                    body.push(b'(');
                    i += 1;
                    let (inner, end) = extract_double_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.extend_from_slice(b"))");
                    i = end;
                } else {
                    let (inner, end) = extract_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.push(b')');
                    i = end;
                }
                continue;
            }
        }
        if b == b'{' {
            brace_depth = brace_depth.saturating_add(1);
            body.push(b);
            i += 1;
            continue;
        }
        if b == b'}' {
            if brace_depth == 0 {
                return Ok((body, i + 1));
            }
            brace_depth -= 1;
            body.push(b);
            i += 1;
            continue;
        }
        body.push(b);
        i += 1;
    }
    Err(ExpandError::BadSubstitution("missing '}'".into()))
}

/// Locate matching `}` accounting for nested `{}`, `$()`, `$(())`, `${}`, and
/// quoted runs.
fn extract_brace_body(
    bytes: &[u8],
    start: usize,
    posix_single_quote: bool,
) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut i = start;
    let mut depth: i32 = 1;
    let mut body = Vec::new();
    let mut swallowed_closing_brace_in_quote = false;
    while i < bytes.len() {
        let b = bytes[i];
        if append_ansi_c_literal(bytes, &mut i, &mut body) {
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'}' {
            body.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            body.push(bytes[i]);
            body.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if b == b'\'' && !posix_single_quote {
            let mut j = i + 1;
            let mut saw_escaped_quote = false;
            let mut closes_before_quote = false;
            while j < bytes.len() && bytes[j] != b'\'' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    if bytes[j + 1] == b'\'' {
                        saw_escaped_quote = true;
                    }
                    j += 2;
                    continue;
                }
                if bytes[j] == b'}' && saw_escaped_quote {
                    closes_before_quote = true;
                    break;
                }
                j += 1;
            }
            if closes_before_quote {
                body.push(b);
                i += 1;
                continue;
            }
            body.push(b);
            i += 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                body.push(bytes[i]);
                i += 1;
            }
            if i < bytes.len() {
                body.push(bytes[i]);
                i += 1;
            }
            continue;
        }
        if b == b'"' {
            let quote_start = i;
            let body_len = body.len();
            let mut swallowed_in_this_quote = false;
            body.push(b);
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'}' {
                    swallowed_in_this_quote = true;
                }
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    body.push(bytes[i]);
                    body.push(bytes[i + 1]);
                    i += 2;
                } else {
                    body.push(bytes[i]);
                    i += 1;
                }
            }
            if i >= bytes.len() {
                if !posix_single_quote {
                    body.truncate(body_len);
                    body.push(b);
                    i = quote_start + 1;
                    continue;
                }
                return Err(ExpandError::Other(
                    "unexpected EOF while looking for matching `}'".into(),
                ));
            }
            if swallowed_in_this_quote {
                swallowed_closing_brace_in_quote = true;
            }
            body.push(bytes[i]);
            i += 1;
            continue;
        }
        if b == b'`' {
            append_quoted_bytes(bytes, &mut i, &mut body, b)?;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n == b'{' {
                body.push(b);
                body.push(n);
                i += 2;
                let (inner, end) = extract_brace_body(bytes, i, posix_single_quote)?;
                body.extend_from_slice(&inner);
                body.push(b'}');
                i = end;
                continue;
            }
            if n == b'(' {
                body.push(b);
                body.push(n);
                i += 2;
                let is_arith = i < bytes.len() && bytes[i] == b'(';
                if is_arith {
                    body.push(b'(');
                    i += 1;
                    let (inner, end) = extract_double_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.extend_from_slice(b"))");
                    i = end;
                } else {
                    let (inner, end) = extract_paren(bytes, i)?;
                    body.extend_from_slice(&inner);
                    body.push(b')');
                    i = end;
                }
                continue;
            }
        }
        if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Ok((body, i + 1));
            }
        }
        body.push(b);
        i += 1;
    }
    if swallowed_closing_brace_in_quote {
        return Err(ExpandError::Other(
            "unexpected EOF while looking for matching `}'".into(),
        ));
    }
    Err(ExpandError::BadSubstitution("missing '}'".into()))
}

/// Find matching `)` starting just after `$(`.
fn extract_paren(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut i = start;
    let mut depth = 1i32;
    let mut case_depth = 0usize;
    let mut comment_ok = true;
    let mut heredocs: std::collections::VecDeque<(Vec<u8>, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut body = Vec::new();
    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let line_end = i;
                let mut candidate_start = line_start;
                let mut candidate = &bytes[candidate_start..line_end];
                if strip_tabs {
                    while candidate.first() == Some(&b'\t') {
                        candidate_start += 1;
                        candidate = &candidate[1..];
                    }
                }
                if depth == 1 {
                    if let Some(close_offset) =
                        heredoc_delimiter_closing_paren(candidate, delimiter.as_slice())
                    {
                        body.extend_from_slice(&bytes[line_start..candidate_start]);
                        body.extend_from_slice(delimiter.as_slice());
                        heredocs.pop_front();
                        i = candidate_start + close_offset + 1;
                        return Ok((body, i));
                    }
                }
                body.extend_from_slice(&bytes[line_start..line_end]);
                if candidate == delimiter.as_slice() {
                    heredocs.pop_front();
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    body.push(bytes[i]);
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            body.push(b);
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                body.push(bytes[i]);
                i += 1;
            }
            continue;
        }

        if b == b'\\' && i + 1 < bytes.len() {
            body.push(b);
            body.push(bytes[i + 1]);
            i += 2;
            comment_ok = false;
            continue;
        }

        if append_ansi_c_literal(bytes, &mut i, &mut body) {
            comment_ok = false;
            continue;
        }

        if matches!(b, b'\'' | b'"' | b'`') {
            append_quoted_bytes(bytes, &mut i, &mut body, b)?;
            comment_ok = false;
            continue;
        }

        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            append_heredoc_redirect(bytes, &mut i, &mut body, &mut heredocs);
            comment_ok = false;
            continue;
        }

        if is_name_start(b) {
            let start_word = i;
            body.push(bytes[i]);
            i += 1;
            while i < bytes.len() && is_name_byte(bytes[i]) {
                body.push(bytes[i]);
                i += 1;
            }
            let previous = previous_significant_byte(bytes, start_word);
            match &bytes[start_word..i] {
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" if !matches!(previous, Some(b'|' | b'(')) => {
                    case_depth = case_depth.saturating_sub(1)
                }
                _ => {}
            }
            comment_ok = false;
            continue;
        }

        if b == b'(' {
            depth += 1;
        } else if b == b')' {
            if case_depth > 0 && depth == 1 {
                body.push(b);
                i += 1;
                comment_ok = true;
                continue;
            }
            depth -= 1;
            if depth == 0 {
                return Ok((body, i + 1));
            }
        }
        body.push(b);
        i += 1;
        comment_ok = b.is_ascii_whitespace() || is_shell_metacharacter(b);
    }
    Err(ExpandError::BadSubstitution("missing ')'".into()))
}

fn heredoc_delimiter_closing_paren(candidate: &[u8], delimiter: &[u8]) -> Option<usize> {
    if !candidate.starts_with(delimiter) {
        return None;
    }
    let mut offset = delimiter.len();
    while offset < candidate.len() && matches!(candidate[offset], b' ' | b'\t') {
        offset += 1;
    }
    if candidate.get(offset) == Some(&b')') {
        Some(offset)
    } else {
        None
    }
}

/// Find matching `))` of `$((...))`. `start` is the position just after the
/// inner `(`.
fn extract_double_paren(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    if let DParenScan::Arithmetic(end) = arith_dparen_scan(bytes, start) {
        Ok((bytes[start..end - 2].to_vec(), end))
    } else {
        Err(ExpandError::ArithSyntax("missing '))'".into()))
    }
}

fn extract_legacy_arith(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), ExpandError> {
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += if i + 1 < bytes.len() { 2 } else { 1 };
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            skip_quoted_arith(bytes, &mut i, b);
            continue;
        }
        if b == b'[' {
            depth += 1;
            i += 1;
            continue;
        }
        if b == b']' {
            depth -= 1;
            if depth == 0 {
                return Ok((bytes[start..i].to_vec(), i + 1));
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    Err(ExpandError::ArithSyntax("missing ']'".into()))
}

fn skip_quoted_arith(bytes: &[u8], i: &mut usize, quote: u8) {
    *i += 1;
    while *i < bytes.len() {
        if bytes[*i] == b'\\' && *i + 1 < bytes.len() {
            *i += 2;
            continue;
        }
        if bytes[*i] == quote {
            *i += 1;
            return;
        }
        *i += 1;
    }
}

fn append_ansi_c_literal(bytes: &[u8], i: &mut usize, body: &mut Vec<u8>) -> bool {
    if *i + 1 >= bytes.len() || bytes[*i] != b'$' || bytes[*i + 1] != b'\'' {
        return false;
    }
    let (_, end) = quote::scan_ansi_c_quoted(bytes, *i + 2);
    body.extend_from_slice(&bytes[*i..end]);
    *i = end;
    true
}

fn append_quoted_bytes(
    bytes: &[u8],
    i: &mut usize,
    body: &mut Vec<u8>,
    quote: u8,
) -> Result<(), ExpandError> {
    if quote == b'"' {
        return append_double_quoted_bytes(bytes, i, body);
    }
    body.push(quote);
    *i += 1;
    while *i < bytes.len() && bytes[*i] != quote {
        if bytes[*i] == b'\\' && *i + 1 < bytes.len() {
            body.push(bytes[*i]);
            body.push(bytes[*i + 1]);
            *i += 2;
        } else {
            body.push(bytes[*i]);
            *i += 1;
        }
    }
    if *i < bytes.len() {
        body.push(bytes[*i]);
        *i += 1;
    }
    Ok(())
}

fn append_double_quoted_bytes(
    bytes: &[u8],
    i: &mut usize,
    body: &mut Vec<u8>,
) -> Result<(), ExpandError> {
    body.push(b'"');
    *i += 1;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'"' {
            body.push(b'"');
            *i += 1;
            return Ok(());
        }
        if b == b'\\' && *i + 1 < bytes.len() {
            body.push(bytes[*i]);
            body.push(bytes[*i + 1]);
            *i += 2;
            continue;
        }
        if b == b'$' && *i + 1 < bytes.len() {
            match bytes[*i + 1] {
                b'{' => {
                    body.extend_from_slice(b"${");
                    *i += 2;
                    let (inner, end) = extract_brace_body(bytes, *i, false)?;
                    body.extend_from_slice(&inner);
                    body.push(b'}');
                    *i = end;
                    continue;
                }
                b'(' => {
                    body.extend_from_slice(b"$(");
                    *i += 2;
                    if *i < bytes.len() && bytes[*i] == b'(' {
                        body.push(b'(');
                        *i += 1;
                        let (inner, end) = extract_double_paren(bytes, *i)?;
                        body.extend_from_slice(&inner);
                        body.extend_from_slice(b"))");
                        *i = end;
                    } else {
                        let (inner, end) = extract_paren(bytes, *i)?;
                        body.extend_from_slice(&inner);
                        body.push(b')');
                        *i = end;
                    }
                    continue;
                }
                _ => {}
            }
        }
        if b == b'`' {
            append_quoted_bytes(bytes, i, body, b'`')?;
            continue;
        }
        body.push(b);
        *i += 1;
    }
    Ok(())
}

fn append_heredoc_redirect(
    bytes: &[u8],
    i: &mut usize,
    body: &mut Vec<u8>,
    heredocs: &mut std::collections::VecDeque<(Vec<u8>, bool)>,
) {
    body.push(b'<');
    *i += 1;
    body.push(b'<');
    *i += 1;
    let strip_tabs = if *i < bytes.len() && bytes[*i] == b'-' {
        body.push(b'-');
        *i += 1;
        true
    } else {
        false
    };
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t') {
        body.push(bytes[*i]);
        *i += 1;
    }
    let mut delimiter = Vec::new();
    let mut quote: Option<u8> = None;
    while *i < bytes.len() {
        let b = bytes[*i];
        if let Some(q) = quote {
            body.push(b);
            *i += 1;
            if b == q {
                quote = None;
            } else {
                delimiter.push(b);
            }
            continue;
        }
        if b == b'\'' || b == b'"' {
            quote = Some(b);
            body.push(b);
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
                body.push(b);
                delimiter.push(bytes[*i]);
                body.push(bytes[*i]);
                *i += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() || is_shell_metacharacter(b) {
            break;
        }
        delimiter.push(b);
        body.push(b);
        *i += 1;
    }
    if strip_tabs {
        while delimiter.first() == Some(&b'\t') {
            delimiter.remove(0);
        }
    }
    if !delimiter.is_empty() {
        heredocs.push_back((delimiter, strip_tabs));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DParenScan {
    Arithmetic(usize),
    CommandSubstitution,
    Missing,
}

fn arith_dparen_scan(bytes: &[u8], start: usize) -> DParenScan {
    let mut i = start;
    let mut depth = 1i32;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                if depth == 1 {
                    if i + 1 < bytes.len() && bytes[i + 1] == b')' {
                        return DParenScan::Arithmetic(i + 2);
                    }
                    let mut j = i + 1;
                    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\r') {
                        j += 1;
                    }
                    if j >= bytes.len() || matches!(bytes[j], b')' | b'\n' | b';' | b'&' | b'|') {
                        return DParenScan::CommandSubstitution;
                    }
                    return DParenScan::Missing;
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    DParenScan::Missing
}

fn is_name_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_name_byte(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit()
}

fn is_valid_name_str(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty() && is_name_start(bytes[0]) && bytes[1..].iter().all(|b| is_name_byte(*b))
}

fn previous_significant_byte(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i > 0 {
        i -= 1;
        if !bytes[i].is_ascii_whitespace() {
            return Some(bytes[i]);
        }
    }
    None
}

fn is_shell_metacharacter(b: u8) -> bool {
    matches!(b, b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')')
}

/// Bridge into ExpandBuf for $@ semantic notes.
trait BufExt {
    fn buf_record_dollar_at(&mut self);
}
impl BufExt for ExpandBuf {
    fn buf_record_dollar_at(&mut self) {
        self.quoted_dollar_at = true;
        self.has_dollar_at = true;
    }
}
