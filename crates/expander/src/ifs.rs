//! Cached IFS state. Mirrors `ifs_value`, `ifs_cmap`, `ifs_whitespace`,
//! `ifs_firstc` from bash subst.c:11985+.

use cherubsh_common::Environment;

#[derive(Clone, Debug)]
pub struct IfsState {
    pub raw: Vec<u8>,
    pub cmap: [bool; 256],
    pub whitespace: [bool; 256],
    pub is_default: bool,
    pub is_unset: bool,
    pub is_null: bool,
    pub first_char: Option<u8>,
    pub utf8_separators: Vec<Vec<u8>>,
}

impl IfsState {
    pub fn from_env(env: &dyn Environment) -> Self {
        match env.get("IFS") {
            None => Self::build_for(b" \t\n", true, true, false),
            Some(v) if v.is_empty() => Self::build_for(b"", true, false, true),
            Some(v) => {
                let bytes = crate::quote::shell_string_to_bytes(&v);
                let is_default = bytes == b" \t\n";
                Self::build_for_owned(bytes, is_default, false, false)
            }
        }
    }

    fn build_for(raw: &[u8], is_default: bool, is_unset: bool, is_null: bool) -> Self {
        Self::build_for_owned(raw.to_vec(), is_default, is_unset, is_null)
    }

    fn build_for_owned(raw: Vec<u8>, is_default: bool, is_unset: bool, is_null: bool) -> Self {
        let mut cmap = [false; 256];
        let mut whitespace = [false; 256];
        for &b in &raw {
            cmap[b as usize] = true;
            if b == b' ' || b == b'\t' || b == b'\n' {
                whitespace[b as usize] = true;
            }
        }
        let first_char = raw.first().copied();
        let utf8_separators = std::str::from_utf8(&raw)
            .ok()
            .map(|text| {
                text.chars()
                    .filter_map(|ch| {
                        let mut buf = [0u8; 4];
                        let bytes = ch.encode_utf8(&mut buf).as_bytes();
                        (bytes.len() > 1).then(|| bytes.to_vec())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            raw,
            cmap,
            whitespace,
            is_default,
            is_unset,
            is_null,
            first_char,
            utf8_separators,
        }
    }

    pub fn refresh(&mut self, env: &dyn Environment) {
        *self = Self::from_env(env);
    }

    /// First IFS byte (used as separator for `"$*"`). Falls back to space when
    /// IFS is unset, empty when null.
    pub fn join_separator(&self) -> Option<u8> {
        if self.is_null {
            None
        } else if self.is_unset {
            Some(b' ')
        } else {
            self.first_char
        }
    }

    pub fn join_separator_bytes(&self) -> Option<Vec<u8>> {
        if self.is_null {
            None
        } else if self.is_unset {
            Some(vec![b' '])
        } else if let Ok(raw) = std::str::from_utf8(&self.raw) {
            raw.chars().next().map(|ch| {
                let mut buf = [0u8; 4];
                ch.encode_utf8(&mut buf).as_bytes().to_vec()
            })
        } else {
            self.first_char.map(|b| vec![b])
        }
    }

    #[inline]
    pub fn is_ifs(&self, b: u8) -> bool {
        self.cmap[b as usize]
    }

    #[inline]
    pub fn is_ifs_ws(&self, b: u8) -> bool {
        self.whitespace[b as usize]
    }

    #[inline]
    pub fn is_ifs_nonws(&self, b: u8) -> bool {
        self.cmap[b as usize] && !self.whitespace[b as usize]
    }
}

impl Default for IfsState {
    fn default() -> Self {
        Self::build_for(b" \t\n", true, true, false)
    }
}
