//! Brace expansion. Mirrors lib/glob/glob.c brace_expand and bash 5.2's
//! `{a,b,c}`, `{1..10}`, `{1..10..2}`, `{a..z..2}` plus zero-padded variants.
//! Runs as a pre-pass before `expand_word_internal` - no parameter, command,
//! or arithmetic expansion happens here.

/// Expand a single raw word's braces. Returns the resulting list (in order).
/// If `word` contains no brace expression, returns `[word]`.
pub fn brace_expand(word: &[u8]) -> Vec<Vec<u8>> {
    let segs = match find_top_brace(word) {
        Some(s) => s,
        None => return vec![word.to_vec()],
    };
    let (prefix, body, suffix) = segs;
    // Try sequence form first: {start..end[..step]}.
    if let Some(seq) = expand_sequence(body) {
        let tail_expansions = brace_expand(suffix);
        let mut out = Vec::new();
        for piece in seq {
            for tail in &tail_expansions {
                let mut combined = Vec::with_capacity(prefix.len() + piece.len() + tail.len());
                combined.extend_from_slice(prefix);
                combined.extend_from_slice(&piece);
                combined.extend_from_slice(tail);
                out.push(combined);
            }
        }
        return out;
    }
    // List form: top-level comma split.
    let alternatives = top_level_split(body);
    debug_assert!(alternatives.len() >= 2);
    let tail_expansions = brace_expand(suffix);
    let mut out = Vec::new();
    for alt in &alternatives {
        let alt_expansions = brace_expand(alt);
        for ae in &alt_expansions {
            for tail in &tail_expansions {
                let mut combined = Vec::with_capacity(prefix.len() + ae.len() + tail.len());
                combined.extend_from_slice(prefix);
                combined.extend_from_slice(ae);
                combined.extend_from_slice(tail);
                out.push(combined);
            }
        }
    }
    out
}

/// Locate the leftmost balanced, expandable `{...}`. Invalid brace pairs are
/// skipped so nested valid expansions like `a-{b{d,e}}-c` still fire.
fn find_top_brace(word: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let mut i = 0;
    while i < word.len() {
        let b = word[i];
        if b == b'\\' && i + 1 < word.len() {
            i += 2;
            continue;
        }
        if b == b'\'' {
            i += 1;
            while i < word.len() && word[i] != b'\'' {
                i += 1;
            }
            if i < word.len() {
                i += 1;
            }
            continue;
        }
        if b == b'"' {
            i += 1;
            while i < word.len() && word[i] != b'"' {
                if word[i] == b'\\' && i + 1 < word.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < word.len() {
                i += 1;
            }
            continue;
        }
        // `$(`, `$((`, `${`: these constructs are NOT brace expansion. Skip
        // past their matching close so we don't mistake their `{` / `(` for a
        // brace-expansion opener.
        if b == b'$' && i + 1 < word.len() {
            let n = word[i + 1];
            if n == b'{' {
                let end = skip_balanced(word, i + 1, b'{', b'}');
                i = end;
                continue;
            }
            if n == b'(' {
                let end = skip_balanced(word, i + 1, b'(', b')');
                i = end;
                continue;
            }
        }
        if b == b'`' {
            i += 1;
            while i < word.len() && word[i] != b'`' {
                if word[i] == b'\\' && i + 1 < word.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < word.len() {
                i += 1;
            }
            continue;
        }
        if b == b'{' {
            // Find matching close
            let body_start = i + 1;
            let mut depth: i32 = 1;
            let mut j = body_start;
            while j < word.len() && depth > 0 {
                let c = word[j];
                if c == b'\\' && j + 1 < word.len() {
                    j += 2;
                    continue;
                }
                if c == b'\'' {
                    j += 1;
                    while j < word.len() && word[j] != b'\'' {
                        j += 1;
                    }
                    if j < word.len() {
                        j += 1;
                    }
                    continue;
                }
                if c == b'"' {
                    j += 1;
                    while j < word.len() && word[j] != b'"' {
                        if word[j] == b'\\' && j + 1 < word.len() {
                            j += 2;
                        } else {
                            j += 1;
                        }
                    }
                    if j < word.len() {
                        j += 1;
                    }
                    continue;
                }
                if c == b'{' {
                    depth += 1;
                } else if c == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        let prefix = &word[..i];
                        let body = &word[body_start..j];
                        let suffix = &word[j + 1..];
                        if is_expandable_body(body) {
                            return Some((prefix, body, suffix));
                        }
                        break;
                    }
                }
                j += 1;
            }
            i = body_start;
            continue;
        }
        i += 1;
    }
    None
}

fn is_expandable_body(body: &[u8]) -> bool {
    expand_sequence(body).is_some() || top_level_split(body).len() >= 2
}

fn skip_balanced(word: &[u8], start: usize, open: u8, close: u8) -> usize {
    if start >= word.len() || word[start] != open {
        return start + 1;
    }
    let mut i = start + 1;
    let mut depth = 1;
    while i < word.len() && depth > 0 {
        let c = word[i];
        if c == b'\\' && i + 1 < word.len() {
            i += 2;
            continue;
        }
        if c == b'\'' {
            i += 1;
            while i < word.len() && word[i] != b'\'' {
                i += 1;
            }
            if i < word.len() {
                i += 1;
            }
            continue;
        }
        if c == b'"' {
            i += 1;
            while i < word.len() && word[i] != b'"' {
                if word[i] == b'\\' && i + 1 < word.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < word.len() {
                i += 1;
            }
            continue;
        }
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
        }
        i += 1;
    }
    i
}

/// Split brace body on top-level `,`. Returns each alternative.
fn top_level_split(body: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut i = 0;
    let mut depth: i32 = 0;
    while i < body.len() {
        let b = body[i];
        if b == b'\\' && i + 1 < body.len() {
            cur.push(b);
            cur.push(body[i + 1]);
            i += 2;
            continue;
        }
        if b == b'\'' {
            cur.push(b);
            i += 1;
            while i < body.len() && body[i] != b'\'' {
                cur.push(body[i]);
                i += 1;
            }
            if i < body.len() {
                cur.push(body[i]);
                i += 1;
            }
            continue;
        }
        if b == b'"' {
            cur.push(b);
            i += 1;
            while i < body.len() && body[i] != b'"' {
                if body[i] == b'\\' && i + 1 < body.len() {
                    cur.push(body[i]);
                    cur.push(body[i + 1]);
                    i += 2;
                } else {
                    cur.push(body[i]);
                    i += 1;
                }
            }
            if i < body.len() {
                cur.push(body[i]);
                i += 1;
            }
            continue;
        }
        if b == b'{' {
            depth += 1;
            cur.push(b);
            i += 1;
            continue;
        }
        if b == b'}' {
            depth -= 1;
            cur.push(b);
            i += 1;
            continue;
        }
        if b == b',' && depth == 0 {
            out.push(std::mem::take(&mut cur));
            i += 1;
            continue;
        }
        cur.push(b);
        i += 1;
    }
    out.push(cur);
    out
}

/// Try to interpret the body as a sequence expression. Returns the list of
/// expanded byte-strings or `None` if not a sequence.
fn expand_sequence(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    // Find ".." at top level (no nesting).
    let dd = find_dotdot(body)?;
    let (start_part, after) = (&body[..dd], &body[dd + 2..]);
    let (end_part, step_part) = match find_dotdot(after) {
        Some(p) => (&after[..p], Some(&after[p + 2..])),
        None => (after, None),
    };
    if start_part.is_empty() || end_part.is_empty() {
        return None;
    }
    // Numeric sequence
    if let (Some(start), Some(end)) = (parse_seq_num(start_part), parse_seq_num(end_part)) {
        let raw_step = match step_part {
            Some(part) => parse_seq_num(part)?.0,
            None => 1,
        };
        let raw_step = if raw_step == 0 { 1 } else { raw_step };
        let abs_step = raw_step.unsigned_abs() as i64;
        let going_up = start.0 <= end.0;
        let width = compute_width(start_part, end_part, start.1, end.1);
        let mut out = Vec::new();
        let (mut cur, end_val) = (start.0, end.0);
        loop {
            let s = if width > 0 {
                let mut s = format!("{:0width$}", cur.abs(), width = width);
                if cur < 0 {
                    s.insert(0, '-');
                }
                s
            } else {
                format!("{}", cur)
            };
            out.push(s.into_bytes());
            if going_up {
                if cur > end_val - abs_step && cur >= end_val {
                    break;
                }
                let next = cur.checked_add(abs_step)?;
                if next > end_val {
                    break;
                }
                cur = next;
            } else {
                if cur < end_val + abs_step && cur <= end_val {
                    break;
                }
                let next = cur.checked_sub(abs_step)?;
                if next < end_val {
                    break;
                }
                cur = next;
            }
            if out.len() > 1_000_000 {
                return None;
            }
        }
        return Some(out);
    }
    // Alphabetic sequence: endpoints must both be letters, but the emitted
    // byte range includes intervening ASCII punctuation just like bash.
    if start_part.len() == 1
        && end_part.len() == 1
        && start_part[0].is_ascii_alphabetic()
        && end_part[0].is_ascii_alphabetic()
    {
        let s = start_part[0];
        let e = end_part[0];
        let raw_step = match step_part {
            Some(part) => parse_seq_num(part)?.0,
            None => 1,
        };
        let raw_step = if raw_step == 0 { 1 } else { raw_step };
        let mut out = Vec::new();
        let mut cur = s as i32;
        let going_up = s <= e;
        let abs_step = raw_step.unsigned_abs() as i32;
        loop {
            out.push(if cur == b'\\' as i32 {
                Vec::new()
            } else {
                vec![cur as u8]
            });
            if going_up {
                if cur >= e as i32 {
                    break;
                }
                cur += abs_step;
                if cur > e as i32 {
                    break;
                }
            } else {
                if cur <= e as i32 {
                    break;
                }
                cur -= abs_step;
                if cur < e as i32 {
                    break;
                }
            }
            if out.len() > 1_000_000 {
                return None;
            }
        }
        return Some(out);
    }
    None
}

fn find_dotdot(body: &[u8]) -> Option<usize> {
    let mut i = 0;
    let mut depth = 0;
    while i + 1 < body.len() {
        let b = body[i];
        if b == b'\\' && i + 1 < body.len() {
            i += 2;
            continue;
        }
        if b == b'{' {
            depth += 1;
            i += 1;
            continue;
        }
        if b == b'}' {
            depth -= 1;
            i += 1;
            continue;
        }
        if depth == 0 && b == b'.' && body[i + 1] == b'.' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a numeric token. Returns (value, original-width-if-zero-padded).
fn parse_seq_num(s: &[u8]) -> Option<(i64, bool)> {
    let str_s = std::str::from_utf8(s).ok()?;
    let trimmed = str_s.trim();
    let mut bytes = trimmed.as_bytes();
    let negative = if bytes.first() == Some(&b'-') {
        bytes = &bytes[1..];
        true
    } else {
        false
    };
    if bytes.is_empty() {
        return None;
    }
    let leading_zero = bytes.len() > 1 && bytes[0] == b'0';
    let n: i64 = trimmed.parse().ok()?;
    let _ = negative;
    Some((n, leading_zero))
}

fn compute_width(start: &[u8], end: &[u8], start_pad: bool, end_pad: bool) -> usize {
    if !start_pad && !end_pad {
        return 0;
    }
    fn width(s: &[u8]) -> usize {
        let mut i = 0;
        if s.first() == Some(&b'-') {
            i = 1;
        }
        s.len() - i
    }
    width(start).max(width(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[Vec<u8>]) -> Vec<String> {
        v.iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect()
    }

    #[test]
    fn no_brace() {
        assert_eq!(s(&brace_expand(b"abc")), vec!["abc"]);
    }

    #[test]
    fn list_basic() {
        assert_eq!(s(&brace_expand(b"a{b,c,d}e")), vec!["abe", "ace", "ade"]);
    }

    #[test]
    fn seq_numeric() {
        assert_eq!(s(&brace_expand(b"{1..5}")), vec!["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn seq_step() {
        assert_eq!(
            s(&brace_expand(b"{1..10..2}")),
            vec!["1", "3", "5", "7", "9"]
        );
    }

    #[test]
    fn seq_step_direction_follows_endpoints() {
        assert_eq!(
            s(&brace_expand(b"{10..1..2}")),
            vec!["10", "8", "6", "4", "2"]
        );
        assert_eq!(
            s(&brace_expand(b"{-1..-10..2}")),
            vec!["-1", "-3", "-5", "-7", "-9"]
        );
    }

    #[test]
    fn seq_zero_step_uses_default_step() {
        assert_eq!(
            s(&brace_expand(b"{10..2..0}")),
            vec!["10", "9", "8", "7", "6", "5", "4", "3", "2"]
        );
        assert_eq!(
            s(&brace_expand(b"{2..9..0}")),
            vec!["2", "3", "4", "5", "6", "7", "8", "9"]
        );
        assert_eq!(
            s(&brace_expand(b"{a..f..0}")),
            vec!["a", "b", "c", "d", "e", "f"]
        );
    }

    #[test]
    fn seq_descending() {
        assert_eq!(s(&brace_expand(b"{5..1}")), vec!["5", "4", "3", "2", "1"]);
    }

    #[test]
    fn seq_alpha() {
        assert_eq!(s(&brace_expand(b"{a..e}")), vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn seq_alpha_skips_backslash() {
        assert_eq!(
            s(&brace_expand(b"{Z..a}")),
            vec!["Z", "[", "", "]", "^", "_", "`", "a"]
        );
    }

    #[test]
    fn mixed_alpha_numeric_sequences_are_literal() {
        assert_eq!(s(&brace_expand(b"{1..f}")), vec!["{1..f}"]);
        assert_eq!(s(&brace_expand(b"{f..1}")), vec!["{f..1}"]);
    }

    #[test]
    fn invalid_sequence_step_is_literal() {
        assert_eq!(s(&brace_expand(b"{1..10..ff}")), vec!["{1..10..ff}"]);
        assert_eq!(s(&brace_expand(b"{1..20..2f}")), vec!["{1..20..2f}"]);
    }

    #[test]
    fn seq_zero_pad() {
        assert_eq!(
            s(&brace_expand(b"{01..05}")),
            vec!["01", "02", "03", "04", "05"]
        );
    }

    #[test]
    fn nested() {
        assert_eq!(s(&brace_expand(b"a{b,{c,d}}")), vec!["ab", "ac", "ad"]);
    }

    #[test]
    fn invalid_outer_brace_does_not_block_nested_expansion() {
        assert_eq!(
            s(&brace_expand(b"a-{b{d,e}}-c")),
            vec!["a-{bd}-c", "a-{be}-c"]
        );
        assert_eq!(
            s(&brace_expand(br#"{"klklkl"}{1,2,3}"#)),
            vec![r#"{"klklkl"}1"#, r#"{"klklkl"}2"#, r#"{"klklkl"}3"#]
        );
    }

    #[test]
    fn cartesian() {
        assert_eq!(
            s(&brace_expand(b"{a,b}{1,2}")),
            vec!["a1", "a2", "b1", "b2"]
        );
    }

    #[test]
    fn solitary_brace_kept() {
        assert_eq!(s(&brace_expand(b"{abc}")), vec!["{abc}"]);
    }
}
