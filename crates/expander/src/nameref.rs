//! Nameref resolution with cycle detection.

use cherubsh_common::{Environment, VarAttrs};

const NAMEREF_MAX_DEPTH: usize = 8;

/// Follow the nameref chain starting at `name`. Returns the final variable
/// name, or `name` itself if it isn't a nameref. Returns `None` if a cycle is
/// detected.
pub fn resolve(env: &dyn Environment, name: &str) -> Option<String> {
    if !env.attrs(name).contains(VarAttrs::NAMEREF) {
        // A regular variable already resolves to itself.
        return Some(name.to_string());
    }
    let mut cur = name.to_string();
    let mut seen = std::collections::HashSet::new();
    seen.insert(cur.clone());
    for _ in 0..NAMEREF_MAX_DEPTH {
        let next = env.resolve_nameref(&cur)?;
        if next == cur {
            return Some(cur);
        }
        if !seen.insert(next.clone()) {
            return None;
        }
        cur = next;
        if !env.attrs(&cur).contains(VarAttrs::NAMEREF) {
            return Some(cur);
        }
    }
    None
}
