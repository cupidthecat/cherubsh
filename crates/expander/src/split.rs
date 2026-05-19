//! IFS-aware word splitting. Port of subst.c:3130 `list_string`. Recognizes
//! CTLESC-protected bytes (those don't trigger splitting even if they map to
//! an IFS char) and CTLNUL placeholders for quoted empty strings.

use crate::buf::{ExpandBuf, CTLESC, CTLNUL, CTLRAW};
use crate::ifs::IfsState;
use crate::wd::Wd;

/// Split `buf` on IFS bytes. Returns one or more `Wd` fields. Mirrors bash
/// semantics:
///  - Runs of IFS-whitespace between fields collapse to one delimiter.
///  - A single IFS-non-whitespace byte = one delimiter; leading/trailing
///    sequences produce empty fields (only for non-whitespace IFS).
///  - CTLESC-protected bytes are literal even if they map to IFS bytes.
///  - A standalone CTLNUL records a quoted empty field and is preserved.
pub fn split(buf: &ExpandBuf, ifs: &IfsState) -> Vec<Wd> {
    let bytes = buf.as_bytes();
    if ifs.is_null {
        if bytes.is_empty() {
            return Vec::new();
        }
        let mut wd = Wd::new();
        wd.buf.bytes.extend_from_slice(bytes);
        return vec![wd];
    }

    let mut fields: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut i = 0;

    while let Some((delim, next)) = unquoted_ifs_at(bytes, i, ifs) {
        if ifs.is_ifs_ws(delim) {
            i = next;
        } else {
            break;
        }
    }
    if i >= bytes.len() {
        return Vec::new();
    }

    while i < bytes.len() {
        if let Some((delim, next)) = unquoted_ifs_at(bytes, i, ifs) {
            fields.push(std::mem::take(&mut cur));
            let whitesep = ifs.is_ifs_ws(delim);
            i = next;

            while let Some((next_delim, next_i)) = unquoted_ifs_at(bytes, i, ifs) {
                if ifs.is_ifs_ws(next_delim) {
                    i = next_i;
                } else {
                    break;
                }
            }

            if whitesep {
                if let Some((next_delim, next_i)) = unquoted_ifs_at(bytes, i, ifs) {
                    if ifs.is_ifs_nonws(next_delim) {
                        i = next_i;
                        while let Some((ws, ws_i)) = unquoted_ifs_at(bytes, i, ifs) {
                            if ifs.is_ifs_ws(ws) {
                                i = ws_i;
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
            continue;
        }
        if let Some((_literal, next)) = raw_literal_at(bytes, i) {
            cur.extend_from_slice(&bytes[i..next]);
            i = next;
            continue;
        }
        let b = bytes[i];
        if b == CTLESC && i + 1 < bytes.len() {
            cur.push(CTLESC);
            cur.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        if b == CTLNUL {
            cur.push(CTLNUL);
            i += 1;
            continue;
        }
        cur.push(b);
        i += 1;
    }
    if !cur.is_empty() {
        fields.push(cur);
    } else if fields.is_empty() {
        fields.push(Vec::new());
    }

    let mut out: Vec<Wd> = Vec::with_capacity(fields.len());
    for bytes in fields {
        out.push(wd_from(bytes));
    }
    if out.is_empty() {
        // Pure-whitespace input produces no fields.
        out.push(Wd::new());
        out.clear();
        return out;
    }
    out
}

fn wd_from(bytes: Vec<u8>) -> Wd {
    let mut w = Wd::new();
    w.buf.bytes = bytes;
    w
}

fn raw_literal_at(bytes: &[u8], i: usize) -> Option<(u8, usize)> {
    if i + 2 < bytes.len() && bytes[i] == CTLESC && bytes[i + 1] == CTLRAW {
        Some((bytes[i + 2], i + 3))
    } else {
        None
    }
}

fn unquoted_ifs_at(bytes: &[u8], i: usize, ifs: &IfsState) -> Option<(u8, usize)> {
    if let Some((literal, next)) = raw_literal_at(bytes, i) {
        return if ifs.is_ifs(literal) {
            Some((literal, next))
        } else {
            None
        };
    }
    let b = *bytes.get(i)?;
    if b == CTLESC || b == CTLNUL {
        return None;
    }
    for sep in &ifs.utf8_separators {
        if bytes.get(i..i + sep.len()) == Some(sep.as_slice()) {
            return Some((sep[0], i + sep.len()));
        }
    }
    if b >= 0x80 && is_inside_valid_utf8_char(bytes, i) {
        return None;
    }
    if ifs.is_ifs(b) {
        Some((b, i + 1))
    } else {
        None
    }
}

fn is_inside_valid_utf8_char(bytes: &[u8], i: usize) -> bool {
    if !is_utf8_continuation(bytes[i]) {
        return false;
    }
    let mut start = i;
    while start > 0 && is_utf8_continuation(bytes[start]) {
        start -= 1;
    }
    if start == i {
        return false;
    }
    let width = utf8_width(bytes[start]);
    width > 1 && start + width > i && start + width <= bytes.len()
}

fn is_utf8_continuation(b: u8) -> bool {
    (b & 0b1100_0000) == 0b1000_0000
}

fn utf8_width(b: u8) -> usize {
    match b {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ifs::IfsState;

    fn buf_of(s: &[u8]) -> ExpandBuf {
        ExpandBuf {
            bytes: s.to_vec(),
            ..ExpandBuf::default()
        }
    }
    fn fields_to_str(fs: &[Wd]) -> Vec<String> {
        fs.iter()
            .map(|w| String::from_utf8_lossy(&w.buf.bytes).into_owned())
            .collect()
    }
    fn fields_to_dequoted_bytes(fs: &[Wd]) -> Vec<Vec<u8>> {
        fs.iter()
            .map(|w| crate::buf::dequote_bytes(&w.buf.bytes))
            .collect()
    }

    #[test]
    fn default_ifs_collapses_ws() {
        let ifs = IfsState::default();
        let b = buf_of(b"a  b\tc");
        assert_eq!(fields_to_str(&split(&b, &ifs)), vec!["a", "b", "c"]);
    }

    #[test]
    fn nonws_ifs_keeps_empty_fields() {
        let raw = b":".to_vec();
        let mut cmap = [false; 256];
        cmap[b':' as usize] = true;
        let ifs = IfsState {
            raw: raw.clone(),
            cmap,
            whitespace: [false; 256],
            is_default: false,
            is_unset: false,
            is_null: false,
            first_char: Some(b':'),
            utf8_separators: Vec::new(),
        };
        let b = buf_of(b"a::b");
        assert_eq!(fields_to_str(&split(&b, &ifs)), vec!["a", "", "b"]);
    }

    #[test]
    fn mixed_ifs_whitespace_and_nonws_matches_posix() {
        let raw = b": ".to_vec();
        let mut cmap = [false; 256];
        cmap[b':' as usize] = true;
        cmap[b' ' as usize] = true;
        let mut whitespace = [false; 256];
        whitespace[b' ' as usize] = true;
        let ifs = IfsState {
            raw,
            cmap,
            whitespace,
            is_default: false,
            is_unset: false,
            is_null: false,
            first_char: Some(b':'),
            utf8_separators: Vec::new(),
        };

        assert_eq!(fields_to_str(&split(&buf_of(b"::"), &ifs)), vec!["", ""]);
        assert_eq!(
            fields_to_str(&split(&buf_of(b"a :b"), &ifs)),
            vec!["a", "b"]
        );
        assert_eq!(fields_to_str(&split(&buf_of(b"a::"), &ifs)), vec!["a", ""]);
        assert_eq!(fields_to_str(&split(&buf_of(b"a:"), &ifs)), vec!["a"]);
    }

    #[test]
    fn raw_ctlesc_can_split_when_unquoted() {
        let raw = vec![CTLESC];
        let mut cmap = [false; 256];
        cmap[CTLESC as usize] = true;
        let ifs = IfsState {
            raw,
            cmap,
            whitespace: [false; 256],
            is_default: false,
            is_unset: false,
            is_null: false,
            first_char: Some(CTLESC),
            utf8_separators: Vec::new(),
        };
        let mut b = ExpandBuf::new();
        b.push_literal_slice(&[b'a', b'b', CTLESC, b'c', b'd', CTLESC, b'e', b'f']);
        assert_eq!(fields_to_str(&split(&b, &ifs)), vec!["ab", "cd", "ef"]);
    }

    #[test]
    fn quoted_ctlesc_does_not_split() {
        let raw = vec![CTLESC];
        let mut cmap = [false; 256];
        cmap[CTLESC as usize] = true;
        let ifs = IfsState {
            raw,
            cmap,
            whitespace: [false; 256],
            is_default: false,
            is_unset: false,
            is_null: false,
            first_char: Some(CTLESC),
            utf8_separators: Vec::new(),
        };
        let mut b = ExpandBuf::new();
        b.push_quoted_slice(&[b'a', b'b', CTLESC, b'c', b'd']);
        assert_eq!(
            fields_to_dequoted_bytes(&split(&b, &ifs)),
            vec![vec![b'a', b'b', CTLESC, b'c', b'd']]
        );
    }
}
