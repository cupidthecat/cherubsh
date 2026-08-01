fn parse_text(
    input_text: &str,
    extglob_patterns: bool,
    posix_mode: bool,
    comments_enabled: bool,
) -> Result<Command, ParseError> {
    validate_command_substitutions_for_parse(
        input_text,
        extglob_patterns,
        posix_mode,
        comments_enabled,
    )?;
    let mut lexer = Lexer::new(input_text);
    lexer.set_extglob_patterns(extglob_patterns);
    lexer.set_posix_mode(posix_mode);
    lexer.set_comments_enabled(comments_enabled);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, input_text);
    parser.parse().map(|ast| ast.root)
}

fn validate_command_substitutions_for_parse(
    input_text: &str,
    extglob_patterns: bool,
    posix_mode: bool,
    comments_enabled: bool,
) -> Result<(), ParseError> {
    let bytes = input_text.as_bytes();
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
                if command_substitution_is_arithmetic(bytes, i) {
                    i = skip_arithmetic_substitution_for_parse(bytes, i + 3).unwrap_or(bytes.len());
                    continue;
                }
                let Some((end, body_start, body, close_offset)) =
                    extract_command_substitution_for_parse(input_text, i + 2)
                else {
                    validate_unclosed_command_substitution_body(
                        input_text,
                        i + 2,
                        extglob_patterns,
                        posix_mode,
                        comments_enabled,
                    )?;
                    return Ok(());
                };
                validate_command_substitution_body(
                    input_text,
                    body_start,
                    close_offset,
                    &body,
                    extglob_patterns,
                    posix_mode,
                    comments_enabled,
                )?;
                i = end;
                continue;
            }
            if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                let Some(end) = skip_parameter_brace_for_probe(bytes, i + 2, posix_mode) else {
                    return Ok(());
                };
                i = end;
                continue;
            }
            if b == b'`' {
                let Some(end) = skip_backtick_body(bytes, i) else {
                    return Ok(());
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
                return Ok(());
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
        if comments_enabled && b == b'#' && comment_ok {
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
                b'\'' => {
                    ansi_c = true;
                    i += 2;
                    comment_ok = false;
                    continue;
                }
                b'(' => {
                    if command_substitution_is_arithmetic(bytes, i) {
                        i = skip_arithmetic_substitution_for_parse(bytes, i + 3)
                            .unwrap_or(bytes.len());
                        continue;
                    }
                    let Some((end, body_start, body, close_offset)) =
                        extract_command_substitution_for_parse(input_text, i + 2)
                    else {
                        validate_unclosed_command_substitution_body(
                            input_text,
                            i + 2,
                            extglob_patterns,
                            posix_mode,
                            comments_enabled,
                        )?;
                        return Ok(());
                    };
                    validate_command_substitution_body(
                        input_text,
                        body_start,
                        close_offset,
                        &body,
                        extglob_patterns,
                        posix_mode,
                        comments_enabled,
                    )?;
                    i = end;
                    comment_ok = false;
                    continue;
                }
                _ => {}
            }
        }
        match b {
            b'\'' => single = true,
            b'"' => double = true,
            b' ' | b'\t' | b'\r' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => {
                comment_ok = true
            }
            _ => comment_ok = false,
        }
        i += 1;
    }
    Ok(())
}

fn validate_command_substitution_body(
    input_text: &str,
    body_start: usize,
    close_offset: usize,
    body: &str,
    extglob_patterns: bool,
    posix_mode: bool,
    comments_enabled: bool,
) -> Result<(), ParseError> {
    validate_command_substitutions_for_parse(body, extglob_patterns, posix_mode, comments_enabled)
        .map_err(|err| {
            map_command_substitution_parse_error(input_text, body_start, close_offset, body, err)
        })?;

    let mut lexer = Lexer::new(body);
    lexer.set_extglob_patterns(extglob_patterns);
    lexer.set_posix_mode(posix_mode);
    lexer.set_comments_enabled(comments_enabled);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, body);
    match parser.parse_input_unit() {
        Ok(_) => Ok(()),
        Err(err) => Err(map_command_substitution_parse_error(
            input_text,
            body_start,
            close_offset,
            body,
            err,
        )),
    }
}

fn validate_unclosed_command_substitution_body(
    input_text: &str,
    body_start: usize,
    extglob_patterns: bool,
    posix_mode: bool,
    comments_enabled: bool,
) -> Result<(), ParseError> {
    let body = &input_text[body_start.min(input_text.len())..];
    let mut lexer = Lexer::new(body);
    lexer.set_extglob_patterns(extglob_patterns);
    lexer.set_posix_mode(posix_mode);
    lexer.set_comments_enabled(comments_enabled);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    let mut parser = Parser::new(tokens, body);
    match parser.parse_input_unit() {
        Ok(_) => Ok(()),
        Err(err) if parse_error_is_at_body_end(body, &err) => Ok(()),
        Err(err) => Err(map_command_substitution_parse_error(
            input_text,
            body_start,
            input_text.len().saturating_sub(1),
            body,
            err,
        )),
    }
}

fn parse_error_is_at_body_end(body: &str, err: &ParseError) -> bool {
    let at_body_end = err
        .span
        .as_ref()
        .map(|span| span.start >= body.len() || span.end >= body.len())
        .unwrap_or(true);
    at_body_end
        && (err.message.starts_with("expected")
            || err.message == "syntax error: unexpected end of file")
}

fn map_command_substitution_parse_error(
    input_text: &str,
    body_start: usize,
    close_offset: usize,
    body: &str,
    err: ParseError,
) -> ParseError {
    if parse_error_is_at_body_end(body, &err) {
        return ParseError {
            message: "unexpected token ')'".to_string(),
            span: Some(cherubsh_common::Span::new(
                0,
                close_offset.min(input_text.len()),
                (close_offset + 1).min(input_text.len()),
            )),
        };
    }

    let span = err.span.map(|span| {
        cherubsh_common::Span::new(
            0,
            body_start.saturating_add(span.start).min(input_text.len()),
            body_start.saturating_add(span.end).min(input_text.len()),
        )
    });
    ParseError {
        message: command_substitution_error_message(err.message),
        span,
    }
}

fn command_substitution_error_message(message: String) -> String {
    if message.starts_with("unexpected token '") && !message.contains("while looking for matching")
    {
        let trimmed = message.trim_end_matches('\'');
        format!("{trimmed}' while looking for matching `)'")
    } else if message.starts_with("syntax error near unexpected token `")
        && !message.contains("while looking for matching")
    {
        format!("{message} while looking for matching `)'")
    } else {
        message
    }
}

fn command_substitution_is_arithmetic(bytes: &[u8], dollar_offset: usize) -> bool {
    dollar_offset + 2 < bytes.len()
        && bytes[dollar_offset] == b'$'
        && bytes[dollar_offset + 1] == b'('
        && bytes[dollar_offset + 2] == b'('
        && skip_arithmetic_substitution_for_parse(bytes, dollar_offset + 3).is_some()
}

fn skip_arithmetic_substitution_for_parse(bytes: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
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
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                if depth == 1 {
                    return (i + 1 < bytes.len() && bytes[i + 1] == b')').then_some(i + 2);
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn extract_command_substitution_for_parse(
    input_text: &str,
    mut i: usize,
) -> Option<(usize, usize, String, usize)> {
    let bytes = input_text.as_bytes();
    let body_start = i;
    let mut depth = 1usize;
    let mut case_depth = 0usize;
    let mut heredocs: std::collections::VecDeque<(String, bool)> =
        std::collections::VecDeque::new();
    let mut at_line_start = false;
    let mut comment_ok = true;
    let mut body = String::new();

    while i < bytes.len() {
        if at_line_start {
            if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                let line_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    body.push(bytes[i] as char);
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
                    body.push('\n');
                    i += 1;
                }
                comment_ok = true;
                at_line_start = true;
                continue;
            }
        }

        let b = bytes[i];
        if b == b'\n' {
            body.push('\n');
            i += 1;
            comment_ok = true;
            at_line_start = true;
            continue;
        }
        at_line_start = false;

        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                body.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            body.push('\\');
            body.push(bytes[i + 1] as char);
            i += 2;
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            let start = i;
            i += 2;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            body.push_str(&input_text[start..i.min(input_text.len())]);
            comment_ok = false;
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            let start = i;
            let Some(end) = skip_quoted_for_parse(bytes, i) else {
                body.push_str(&input_text[start..]);
                return None;
            };
            body.push_str(&input_text[start..end]);
            i = end;
            comment_ok = false;
            continue;
        }
        if b == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
            let before = i;
            skip_heredoc_redirect_for_probe(bytes, &mut i, &mut heredocs);
            body.push_str(&input_text[before..i.min(input_text.len())]);
            comment_ok = false;
            continue;
        }
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if command_substitution_is_arithmetic(bytes, i) {
                let end = skip_arithmetic_substitution_for_parse(bytes, i + 3)?;
                body.push_str(&input_text[i..end]);
                i = end;
                comment_ok = false;
                continue;
            }
            depth = depth.saturating_add(1);
            body.push('$');
            body.push('(');
            i += 2;
            comment_ok = true;
            continue;
        }
        if is_probe_name_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_probe_name_byte(bytes[i]) {
                i += 1;
            }
            let previous = previous_significant_byte(bytes, start);
            match &bytes[start..i] {
                b"case" => case_depth = case_depth.saturating_add(1),
                b"esac" if !matches!(previous, Some(b'|' | b'(')) => {
                    case_depth = case_depth.saturating_sub(1)
                }
                _ => {}
            }
            body.push_str(&input_text[start..i]);
            comment_ok = false;
            continue;
        }
        if b == b'(' {
            depth = depth.saturating_add(1);
            body.push('(');
            i += 1;
            comment_ok = true;
            continue;
        }
        if b == b')' {
            if case_depth > 0 && depth == 1 {
                body.push(')');
                i += 1;
                comment_ok = true;
                continue;
            }
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some((i + 1, body_start, body, i));
            }
            body.push(')');
            i += 1;
            comment_ok = true;
            continue;
        }
        body.push(b as char);
        i += 1;
        comment_ok = b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>');
    }
    None
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

fn skip_quoted_for_parse(bytes: &[u8], mut i: usize) -> Option<usize> {
    let quote = bytes[i];
    if quote == b'"' {
        return skip_double_quoted_for_parse(bytes, i + 1);
    }
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

fn skip_double_quoted_for_parse(bytes: &[u8], mut i: usize) -> Option<usize> {
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if command_substitution_is_arithmetic(bytes, i) {
                i = skip_arithmetic_substitution_for_parse(bytes, i + 3)?;
            } else {
                i = skip_command_substitution_for_probe(bytes, i + 2)?;
            }
            continue;
        }
        if bytes[i] == b'`' {
            i = skip_backtick_body(bytes, i)?;
            continue;
        }
        if bytes[i] == b'"' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}
