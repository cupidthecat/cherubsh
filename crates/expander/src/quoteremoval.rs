//! Final quote-removal pass. Strips CTLESC/CTLNUL sentinels from an
//! `ExpandBuf` and returns a clean byte vector. CTLNUL alone in an otherwise
//! empty buffer preserves the empty-but-present field.

use crate::buf::{dequote_bytes, is_quoted_null};
use crate::quote::bytes_to_shell_string;
use crate::wd::Wd;
use cherubsh_parser::WordDesc as PWordDesc;

pub fn dequote(mut wd: Wd) -> Wd {
    let dequoted = dequote_bytes(wd.buf.as_bytes());
    if dequoted.is_empty() && is_quoted_null(wd.buf.as_bytes()) {
        // Preserve quoted empty field: leave a zero-length buf.
        wd.buf.bytes.clear();
    } else {
        wd.buf.bytes = dequoted;
    }
    wd
}

pub fn to_parser(wd: Wd) -> PWordDesc {
    let cleaned = dequote(wd);
    PWordDesc {
        text: bytes_to_shell_string(cleaned.buf.as_bytes()),
        flags: cleaned.flags,
        span: cleaned.span,
        raw: None,
    }
}

pub fn to_string(wd: Wd) -> String {
    let cleaned = dequote(wd);
    bytes_to_shell_string(cleaned.buf.as_bytes())
}

#[cfg(test)]
mod tests {
    use cherubsh_common::Span;

    use crate::buf::{CTLESC, CTLNUL};
    use crate::wd::Wd;

    use super::to_parser;

    #[test]
    fn to_parser_dequotes_only_once_for_literal_ctl_bytes() {
        let wd = Wd::from_bytes_with_flags(
            vec![b'a', CTLESC, CTLESC, b'b', CTLESC, CTLNUL, b'c'],
            0,
            Span::dummy(),
        );
        let out = to_parser(wd);
        assert_eq!(out.text.as_bytes(), &[b'a', CTLESC, b'b', CTLNUL, b'c']);
    }
}
