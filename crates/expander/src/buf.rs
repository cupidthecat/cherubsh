//! In-flight expansion buffer. Mirrors bash's `istring` accumulator in
//! `expand_word_internal` (subst.c:10934). CTLESC (0x01) marks the next byte as
//! literal (not a glob/IFS/quote metachar); CTLNUL (0x7F) marks a quoted empty
//! string so word-splitting preserves it as a field.

pub const CTLESC: u8 = 0x01;
pub const CTLRAW: u8 = 0x00;
pub const CTLFIELD: u8 = 0x1E;
pub const CTLNUL: u8 = 0x7F;

#[inline]
pub fn is_ctl(b: u8) -> bool {
    b == CTLESC || b == CTLFIELD || b == CTLNUL
}

#[derive(Debug, Default, Clone)]
pub struct ExpandBuf {
    pub(crate) bytes: Vec<u8>,
    pub(crate) param_nulls: Vec<usize>,
    pub had_quoted_null: bool,
    pub has_dollar_at: bool,
    pub quoted_dollar_at: bool,
}

impl ExpandBuf {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(n),
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Push a single byte from input, escaping CTLESC/CTLNUL collisions with a
    /// preceding CTLESC.
    pub fn push_literal(&mut self, b: u8) {
        if is_ctl(b) {
            self.bytes.push(CTLESC);
            self.bytes.push(CTLRAW);
        }
        self.bytes.push(b);
    }

    /// Push a byte that must survive word-splitting and globbing - emits
    /// `CTLESC b`. Used for characters inside quotes.
    pub fn push_quoted(&mut self, b: u8) {
        self.bytes.push(CTLESC);
        self.bytes.push(b);
    }

    /// Record a quoted empty string (e.g. `""`). CTLNUL is preserved through
    /// splitting and stripped at quote-removal.
    pub fn push_quoted_null(&mut self) {
        self.had_quoted_null = true;
        self.bytes.push(CTLNUL);
    }

    /// Record a quoted empty parameter value. This dequotes like a normal
    /// quoted null, but can be suppressed by adjacent zero-length "$@".
    pub fn push_quoted_param_null(&mut self) {
        self.had_quoted_null = true;
        self.param_nulls.push(self.bytes.len());
        self.bytes.push(CTLNUL);
    }

    /// Boundary between fields produced by quoted `"${array[@]}"` / `"$@"`.
    pub fn push_field_sep(&mut self) {
        self.bytes.push(CTLFIELD);
    }

    /// Append a slice as literal bytes.
    pub fn push_literal_slice(&mut self, s: &[u8]) {
        for b in s {
            self.push_literal(*b);
        }
    }

    /// Append a slice with every byte CTLESC-protected.
    pub fn push_quoted_slice(&mut self, s: &[u8]) {
        for b in s {
            self.push_quoted(*b);
        }
    }

    /// Append raw bytes - caller is responsible for invariants.
    pub fn push_raw(&mut self, s: &[u8]) {
        self.bytes.extend_from_slice(s);
    }

    pub fn push_raw_byte(&mut self, b: u8) {
        self.bytes.push(b);
    }

    pub fn extend_from(&mut self, other: &ExpandBuf) {
        let offset = self.bytes.len();
        self.bytes.extend_from_slice(&other.bytes);
        self.param_nulls
            .extend(other.param_nulls.iter().map(|pos| offset + pos));
        self.had_quoted_null |= other.had_quoted_null;
        self.has_dollar_at |= other.has_dollar_at;
        self.quoted_dollar_at |= other.quoted_dollar_at;
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Strip CTLESC/CTLNUL from a buffer - final quote-removal pass on a word.
/// Returns `None` if the result is a single quoted-null (an empty quoted
/// string) so callers can distinguish "field intentionally empty" from "no
/// content at all".
pub fn dequote_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let b = input[i];
        if b == CTLESC && i + 2 < input.len() && input[i + 1] == CTLRAW {
            out.push(input[i + 2]);
            i += 3;
            continue;
        }
        if b == CTLESC && i + 1 < input.len() {
            out.push(input[i + 1]);
            i += 2;
            continue;
        }
        if b == CTLNUL || b == CTLFIELD {
            // CTLNUL strips to nothing; only its presence in an otherwise
            // empty buffer signals a quoted empty field.
            i += 1;
            continue;
        }
        out.push(b);
        i += 1;
    }
    out
}

/// Is the buffer a single CTLNUL (i.e. a quoted empty string)?
pub fn is_quoted_null(input: &[u8]) -> bool {
    input.len() == 1 && input[0] == CTLNUL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_literal_escapes_ctlbytes() {
        let mut buf = ExpandBuf::new();
        buf.push_literal(b'a');
        buf.push_literal(CTLESC);
        buf.push_literal(b'b');
        assert_eq!(buf.bytes, vec![b'a', CTLESC, CTLRAW, CTLESC, b'b']);
    }

    #[test]
    fn dequote_strips_ctlesc() {
        assert_eq!(dequote_bytes(&[CTLESC, b'$', b'x']), b"$x".to_vec());
        assert_eq!(dequote_bytes(&[b'a', CTLNUL, b'b']), b"ab".to_vec());
    }

    #[test]
    fn quoted_null_preserved_in_buf() {
        let mut buf = ExpandBuf::new();
        buf.push_quoted_null();
        assert!(buf.had_quoted_null);
        assert!(is_quoted_null(&buf.bytes));
    }
}
