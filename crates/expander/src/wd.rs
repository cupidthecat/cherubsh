//! Internal word descriptor used during expansion. Distinct from
//! `parser::WordDesc` to carry quoting/state information that's only meaningful
//! during the expansion pipeline.

use cherubsh_common::Span;
use cherubsh_parser::WordDesc as PWordDesc;

use crate::buf::{dequote_bytes, ExpandBuf};
use crate::quote::bytes_to_shell_string;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotedState {
    Unquoted,
    Partially,
    Wholly,
}

impl Default for QuotedState {
    fn default() -> Self {
        QuotedState::Unquoted
    }
}

#[derive(Clone, Debug)]
pub struct Wd {
    pub buf: ExpandBuf,
    pub flags: u32,
    pub quoted_state: QuotedState,
    pub span: Span,
}

impl Wd {
    pub fn new() -> Self {
        Self {
            buf: ExpandBuf::new(),
            flags: 0,
            quoted_state: QuotedState::Unquoted,
            span: Span::dummy(),
        }
    }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            buf: ExpandBuf::with_capacity(n),
            flags: 0,
            quoted_state: QuotedState::Unquoted,
            span: Span::dummy(),
        }
    }

    pub fn from_parser(p: &PWordDesc) -> Self {
        Self {
            buf: ExpandBuf {
                bytes: p.text.as_bytes().to_vec(),
                ..ExpandBuf::default()
            },
            flags: p.flags,
            quoted_state: QuotedState::Unquoted,
            span: p.span,
        }
    }

    pub fn from_bytes_with_flags(bytes: Vec<u8>, flags: u32, span: Span) -> Self {
        Self {
            buf: ExpandBuf {
                bytes,
                ..ExpandBuf::default()
            },
            flags,
            quoted_state: QuotedState::Unquoted,
            span,
        }
    }

    /// Dequote and return as final WordDesc.
    pub fn into_parser(self) -> PWordDesc {
        let bytes = dequote_bytes(self.buf.as_bytes());
        let text = bytes_to_shell_string(&bytes);
        PWordDesc {
            text,
            flags: self.flags,
            span: self.span,
        }
    }

    /// Dequote and return as String (for callers that don't care about flags).
    pub fn into_string(self) -> String {
        let bytes = dequote_bytes(self.buf.as_bytes());
        bytes_to_shell_string(&bytes)
    }
}

impl Default for Wd {
    fn default() -> Self {
        Self::new()
    }
}
