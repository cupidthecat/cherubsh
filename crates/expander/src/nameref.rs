//! Nameref resolution with cycle detection.

use cherubsh_common::{Environment, VarAttrs};

/// Follow the nameref chain starting at `name`. Returns the final variable
/// name, or `name` itself if it isn't a nameref. Returns `None` if a cycle is
/// detected.
pub fn resolve(env: &dyn Environment, name: &str) -> Option<String> {
    if !env.attrs(name).contains(VarAttrs::NAMEREF) {
        // Trust the env-level resolver as a fallback (ShellState wires this up)
        // but if the var isn't a nameref the answer is just `name`.
        return Some(name.to_string());
    }
    let mut cur = name.to_string();
    let mut seen = std::collections::HashSet::new();
    seen.insert(cur.clone());
    for _ in 0..32 {
        let next = match env.resolve_nameref(&cur) {
            Some(n) => n,
            None => return None,
        };
        if next == cur {
            return Some(cur);
        }
        if !seen.insert(next.clone()) {
            return None; // cycle
        }
        cur = next;
        if !env.attrs(&cur).contains(VarAttrs::NAMEREF) {
            return Some(cur);
        }
    }
    None
}
