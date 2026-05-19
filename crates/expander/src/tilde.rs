//! Tilde expansion. Mirrors lib/tilde/tilde.c semantics plus the bash
//! extensions `~+`, `~-`, `~N`, `~+N`, `~-N` (dirstack).

use std::ffi::CStr;

use cherubsh_common::Environment;

/// Expand a single tilde-prefix token. Returns `Some((replacement, consumed))`
/// or `None` if the prefix is not tilde-eligible. The caller decides where the
/// prefix ends (typically at `/` or end-of-word).
pub fn try_expand(prefix: &[u8], env: &dyn Environment) -> Option<Vec<u8>> {
    if prefix.is_empty() || prefix[0] != b'~' {
        return None;
    }
    let rest = &prefix[1..];
    if rest.iter().any(|b| matches!(*b, b'\\' | b'\'' | b'"')) {
        return None;
    }
    if rest.is_empty() {
        return env.get("HOME").map(|h| h.into_bytes());
    }
    if rest == b"+" {
        return env.get("PWD").map(|p| p.into_bytes());
    }
    if rest == b"-" {
        return env.get("OLDPWD").map(|p| p.into_bytes());
    }
    // ~+N or ~-N or ~N - DIRSTACK index.
    if rest[0] == b'+' || rest[0] == b'-' {
        let sign = rest[0];
        let num = std::str::from_utf8(&rest[1..]).ok()?;
        let n: usize = num.parse().ok()?;
        let dirs = collect_dirstack(env);
        let index = if sign == b'+' {
            n
        } else {
            dirs.len().checked_sub(1 + n)?
        };
        return dirs.get(index).cloned().map(|s| s.into_bytes());
    }
    if rest.iter().all(|b| b.is_ascii_digit()) {
        let num = std::str::from_utf8(rest).ok()?;
        let n: usize = num.parse().ok()?;
        let dirs = collect_dirstack(env);
        return dirs.get(n).cloned().map(|s| s.into_bytes());
    }
    // ~user - getpwnam.
    lookup_user(rest)
}

fn collect_dirstack(env: &dyn Environment) -> Vec<String> {
    if let Some(vals) = env.get_array("DIRSTACK") {
        return vals;
    }
    if let Some(p) = env.get("PWD") {
        return vec![p];
    }
    Vec::new()
}

fn lookup_user(name_bytes: &[u8]) -> Option<Vec<u8>> {
    let cname = std::ffi::CString::new(name_bytes).ok()?;
    unsafe {
        let pw = libc::getpwnam(cname.as_ptr());
        if pw.is_null() {
            return None;
        }
        let dir = (*pw).pw_dir;
        if dir.is_null() {
            return None;
        }
        Some(CStr::from_ptr(dir).to_bytes().to_vec())
    }
}

/// Identify the tilde-prefix at the start of `word` (returns its length within
/// `word`). Stops at `/` or end-of-string. Returns 0 if `word` doesn't begin
/// with `~`.
pub fn prefix_len(word: &[u8]) -> usize {
    if word.is_empty() || word[0] != b'~' {
        return 0;
    }
    let mut i = 1;
    while i < word.len() && word[i] != b'/' && word[i] != b':' {
        i += 1;
    }
    i
}

/// Find boundaries within an assignment-RHS where tilde expansion should
/// trigger: start, and any position immediately following an unquoted `:`.
pub fn assignment_tilde_positions(rhs: &[u8]) -> Vec<usize> {
    let mut out = vec![0];
    for (i, b) in rhs.iter().enumerate() {
        if *b == b':' && i + 1 < rhs.len() && rhs[i + 1] == b'~' {
            out.push(i + 1);
        }
    }
    out
}
