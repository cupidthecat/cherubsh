//! Quote scanners and `$'...'` / `$"..."` decoders.

use crate::buf::ExpandBuf;

const RAW_BYTE_BASE: u32 = 0xF0000;

/// Decode the body of an ANSI-C `$'...'` literal. Recognized escapes mirror
/// `ansicstr` in bash lib/sh/strtrans.c: `\a \b \e \E \f \n \r \t \v \\ \' \"
/// \? \nnn (octal) \xhh \x{hexdigits} \cX \uhhhh \Uhhhhhhhh`.
pub fn ansi_c_decode(src: &[u8]) -> Vec<u8> {
    ansi_c_decode_for_locale(src, None)
}

pub fn ansi_c_decode_for_locale(src: &[u8], locale: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        let b = src[i];
        if b != b'\\' || i + 1 >= src.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let n = src[i + 1];
        match n {
            b'a' => {
                out.push(0x07);
                i += 2;
            }
            b'b' => {
                out.push(0x08);
                i += 2;
            }
            b'e' | b'E' => {
                out.push(0x1b);
                i += 2;
            }
            b'f' => {
                out.push(0x0c);
                i += 2;
            }
            b'n' => {
                out.push(b'\n');
                i += 2;
            }
            b'r' => {
                out.push(b'\r');
                i += 2;
            }
            b't' => {
                out.push(b'\t');
                i += 2;
            }
            b'v' => {
                out.push(0x0b);
                i += 2;
            }
            b'\\' => {
                out.push(b'\\');
                i += 2;
            }
            b'\'' => {
                out.push(b'\'');
                i += 2;
            }
            b'"' => {
                out.push(b'"');
                i += 2;
            }
            b'?' => {
                out.push(b'?');
                i += 2;
            }
            b'0'..=b'7' => {
                let mut j = i + 1;
                let stop = (i + 4).min(src.len());
                let mut val: u32 = 0;
                while j < stop && (b'0'..=b'7').contains(&src[j]) {
                    val = val * 8 + (src[j] - b'0') as u32;
                    j += 1;
                }
                if !push_ansi_byte(&mut out, val & 0xff) {
                    break;
                }
                i = j;
            }
            b'x' => {
                let (val, j, count) = decode_hex_escape(src, i + 2);
                if count == 0 && src.get(i + 2) != Some(&b'{') {
                    out.push(b'\\');
                    out.push(b'x');
                    i += 2;
                } else {
                    if !push_ansi_byte(&mut out, val & 0xff) {
                        break;
                    }
                    i = j;
                }
            }
            b'u' | b'U' => {
                let max = if n == b'u' { 4 } else { 8 };
                let mut j = i + 2;
                let stop = (j + max).min(src.len());
                let mut val: u32 = 0;
                let mut count = 0;
                while j < stop {
                    match hex_digit(src[j]) {
                        Some(d) => {
                            val = val * 16 + d;
                            j += 1;
                            count += 1;
                        }
                        None => break,
                    }
                }
                if count == 0 {
                    out.push(b'\\');
                    out.push(n);
                    i += 2;
                } else {
                    if !push_unicode_for_locale(&mut out, val, locale) {
                        break;
                    }
                    i = j;
                }
            }
            b'c' => {
                if i + 2 < src.len() {
                    let mut j = i + 2;
                    let c = if src[j] == b'\\' && j + 1 < src.len() {
                        j += 1;
                        src[j]
                    } else {
                        src[j]
                    };
                    let v = match c {
                        b'@' => 0u8,
                        b'a'..=b'z' => c - b'a' + 1,
                        b'A'..=b'Z' => c - b'A' + 1,
                        b'[' => 0x1b,
                        b'\\' => 0x1c,
                        b']' => 0x1d,
                        b'^' => 0x1e,
                        b'_' => 0x1f,
                        b'?' => 0x7f,
                        other => other,
                    };
                    if !push_ansi_byte(&mut out, v as u32) {
                        break;
                    }
                    i = j + 1;
                } else {
                    out.push(b'\\');
                    out.push(b'c');
                    i += 2;
                }
            }
            other => {
                out.push(b'\\');
                out.push(other);
                i += 2;
            }
        }
    }
    out
}

/// Convert shell bytes into the current `WordDesc` string representation
/// without losing bytes that are not valid UTF-8. Valid UTF-8 stays readable;
/// invalid bytes are parked in a private-use range and decoded at exec time.
pub fn bytes_to_shell_string(mut bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    let valid = std::str::from_utf8(&bytes[..valid_up_to]).unwrap_or("");
                    out.push_str(valid);
                    bytes = &bytes[valid_up_to..];
                }
                let invalid_len = err.error_len().unwrap_or(bytes.len()).min(bytes.len());
                for &b in &bytes[..invalid_len] {
                    push_raw_byte_marker(&mut out, b);
                }
                bytes = &bytes[invalid_len..];
            }
        }
    }
    out
}

/// Decode the internal shell string representation back to bytes.
pub fn shell_string_to_bytes(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if (RAW_BYTE_BASE..=RAW_BYTE_BASE + 0xff).contains(&cp) {
            out.push((cp - RAW_BYTE_BASE) as u8);
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// Decode to bytes suitable for `exec*`; Unix argv cannot carry NUL bytes.
pub fn shell_string_to_cstring_bytes(s: &str) -> Vec<u8> {
    let mut bytes = shell_string_to_bytes(s);
    if let Some(pos) = bytes.iter().position(|b| *b == 0) {
        bytes.truncate(pos);
    }
    bytes
}

fn push_raw_byte_marker(out: &mut String, byte: u8) {
    if let Some(ch) = char::from_u32(RAW_BYTE_BASE + byte as u32) {
        out.push(ch);
    }
}

fn decode_hex_escape(src: &[u8], start: usize) -> (u32, usize, usize) {
    let braced = src.get(start) == Some(&b'{');
    let mut j = if braced { start + 1 } else { start };
    let stop = if braced {
        src.len()
    } else {
        (j + 2).min(src.len())
    };
    let mut val: u32 = 0;
    let mut count = 0;
    while j < stop {
        match hex_digit(src[j]) {
            Some(d) => {
                val = val.wrapping_mul(16).wrapping_add(d);
                j += 1;
                count += 1;
            }
            None => break,
        }
    }
    if braced && j < src.len() && src[j] == b'}' {
        j += 1;
    }
    (val, j, count)
}

fn hex_digit(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

fn push_ansi_byte(out: &mut Vec<u8>, value: u32) -> bool {
    let byte = (value & 0xff) as u8;
    if byte == 0 {
        return false;
    }
    out.push(byte);
    true
}

fn push_unicode_for_locale(out: &mut Vec<u8>, cp: u32, locale: Option<&str>) -> bool {
    if cp == 0 {
        return false;
    }
    if locale.is_some_and(is_big5_hkscs_locale) && cp == 0x03b1 {
        out.extend_from_slice(&[0xa3, 0x5c]);
        return true;
    }
    if let Some(c) = char::from_u32(cp) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        out.extend_from_slice(s.as_bytes());
    }
    true
}

fn is_big5_hkscs_locale(locale: &str) -> bool {
    let lower = locale.to_ascii_lowercase();
    lower.starts_with("zh_hk.") && lower.contains("big5hkscs")
}

/// Scan a single-quoted run starting just past the opening `'`. Returns the
/// content bytes and the offset of the byte after the closing `'`.
pub fn scan_single_quoted(src: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut i = start;
    let mut out = Vec::new();
    while i < src.len() && src[i] != b'\'' {
        out.push(src[i]);
        i += 1;
    }
    let end = if i < src.len() { i + 1 } else { i };
    (out, end)
}

/// Scan an ANSI-C quoted run starting just past the opening `$'`. The returned
/// body keeps backslash escapes intact for `ansi_c_decode`; an escaped `'` does
/// not terminate the quote.
pub fn scan_ansi_c_quoted(src: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut i = start;
    let mut out = Vec::new();
    while i < src.len() {
        match src[i] {
            b'\'' => return (out, i + 1),
            b'\\' if i + 1 < src.len() => {
                out.push(src[i]);
                out.push(src[i + 1]);
                i += 2;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    (out, i)
}

/// Quote the bytes as a shell literal - emitted by `${var@Q}`. Bash prefers
/// single quotes for printable strings, including embedded single quotes and
/// backslashes, and uses `$'...'` when control bytes need escape syntax.
pub fn shell_quote(src: &[u8]) -> Vec<u8> {
    let needs_dollar = src.iter().any(|&b| b < 0x20 || b == 0x7f);
    if !needs_dollar {
        let mut out = Vec::with_capacity(src.len() + 2);
        out.push(b'\'');
        for &b in src {
            if b == b'\'' {
                out.extend_from_slice(br#"'\''"#);
            } else {
                out.push(b);
            }
        }
        out.push(b'\'');
        return out;
    }
    let mut out = Vec::with_capacity(src.len() + 4);
    out.push(b'$');
    out.push(b'\'');
    for &b in src {
        match b {
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x1b => out.extend_from_slice(b"\\E"),
            0x00..=0x1f | 0x7f => {
                out.extend_from_slice(format!("\\{:03o}", b).as_bytes());
            }
            _ => out.push(b),
        }
    }
    out.push(b'\'');
    out
}

/// Push the contents of an ANSI-C string literal (already-decoded bytes) into
/// `buf`, with CTLESC protection so it survives splitting.
pub fn push_ansi_c_quoted(buf: &mut ExpandBuf, decoded: &[u8]) {
    if decoded.is_empty() {
        buf.push_quoted_null();
    } else {
        for &b in decoded {
            buf.push_quoted(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_basic() {
        assert_eq!(ansi_c_decode(b"hi\\nthere"), b"hi\nthere".to_vec());
    }

    #[test]
    fn ansi_octal() {
        assert_eq!(ansi_c_decode(b"\\101"), b"A".to_vec());
    }

    #[test]
    fn ansi_hex() {
        assert_eq!(ansi_c_decode(b"\\x41"), b"A".to_vec());
    }

    #[test]
    fn ansi_hex_braced_consumes_hex_and_truncates_to_byte() {
        assert_eq!(ansi_c_decode(b"ab\\x{41}cd"), b"abAcd".to_vec());
        assert_eq!(ansi_c_decode(b"\\x{01234567X"), b"gX".to_vec());
        assert_eq!(ansi_c_decode(b"\\x{ab}cX"), vec![0xab, b'c', b'X']);
    }

    #[test]
    fn ansi_nul_terminates_literal_body() {
        assert_eq!(ansi_c_decode(b"ab\\x{}cd"), b"ab".to_vec());
        assert_eq!(ansi_c_decode(b"\\x00tail"), Vec::<u8>::new());
        assert_eq!(ansi_c_decode(b"\\0tail"), Vec::<u8>::new());
        assert_eq!(ansi_c_decode(b"\\c@tail"), Vec::<u8>::new());
    }

    #[test]
    fn ansi_unicode() {
        assert_eq!(ansi_c_decode(b"\\u0041"), b"A".to_vec());
    }

    #[test]
    fn ansi_unicode_honors_big5_hkscs_locale() {
        assert_eq!(
            ansi_c_decode_for_locale(b"\\u3b1", Some("zh_HK.big5hkscs")),
            vec![0xa3, 0x5c]
        );
    }

    #[test]
    fn ansi_ctrl() {
        assert_eq!(ansi_c_decode(b"\\cA"), vec![1u8]);
        assert_eq!(ansi_c_decode(b"\\c^"), vec![0x1e]);
    }

    #[test]
    fn ansi_ctrl_escaped_backslash_consumes_escape() {
        assert_eq!(
            ansi_c_decode(br#"\c[\c\\\c]\c^\c_\c?"#),
            vec![0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x7f]
        );
    }

    #[test]
    fn shell_string_preserves_invalid_raw_bytes() {
        let text = bytes_to_shell_string(&[b'a', 0xde, b'b', 0xff]);
        assert_eq!(shell_string_to_bytes(&text), vec![b'a', 0xde, b'b', 0xff]);
        assert_eq!(
            shell_string_to_cstring_bytes(&text),
            vec![b'a', 0xde, b'b', 0xff]
        );
    }

    #[test]
    fn shell_string_keeps_valid_utf8_as_utf8() {
        let text = bytes_to_shell_string("aé".as_bytes());
        assert_eq!(text, "aé");
        assert_eq!(shell_string_to_bytes(&text), "aé".as_bytes());
    }

    #[test]
    fn quote_round_special() {
        let q = shell_quote(b"a'b\nc");
        let s = String::from_utf8(q).unwrap();
        assert_eq!(s, "$'a\\'b\\nc'");
    }

    #[test]
    fn quote_plain_uses_single_quotes() {
        let q = shell_quote(b"hello world");
        assert_eq!(String::from_utf8(q).unwrap(), "'hello world'");
    }

    #[test]
    fn scan_single() {
        let (body, end) = scan_single_quoted(b"abc'rest", 0);
        assert_eq!(body, b"abc".to_vec());
        assert_eq!(end, 4);
    }

    #[test]
    fn scan_ansi_c_quoted_keeps_escaped_single_quotes() {
        let (body, end) = scan_ansi_c_quoted(br#"\'abcd\''rest"#, 0);
        assert_eq!(body, br#"\'abcd\'"#.to_vec());
        assert_eq!(end, 9);
        assert_eq!(ansi_c_decode(&body), b"'abcd'".to_vec());
    }
}
