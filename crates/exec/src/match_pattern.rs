/// bash-style glob match: `*`, `?`, `[set]`, `[!set]`, `[^set]`.
pub(crate) fn matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    match_at(&p, 0, &t, 0)
}

fn match_at(p: &[char], pi: usize, t: &[char], ti: usize) -> bool {
    if pi >= p.len() {
        return ti >= t.len();
    }
    match p[pi] {
        '*' => {
            // collapse runs of *
            let mut npi = pi;
            while npi < p.len() && p[npi] == '*' {
                npi += 1;
            }
            if npi >= p.len() {
                return true;
            }
            let mut k = ti;
            loop {
                if match_at(p, npi, t, k) {
                    return true;
                }
                if k >= t.len() {
                    return false;
                }
                k += 1;
            }
        }
        '?' => {
            if ti >= t.len() {
                return false;
            }
            match_at(p, pi + 1, t, ti + 1)
        }
        '[' => {
            if ti >= t.len() {
                return false;
            }
            let (matched, consumed) = match_class(&p[pi..], t[ti]);
            if matched {
                match_at(p, pi + consumed, t, ti + 1)
            } else {
                false
            }
        }
        '\\' => {
            if pi + 1 >= p.len() {
                return false;
            }
            if ti >= t.len() || t[ti] != p[pi + 1] {
                return false;
            }
            match_at(p, pi + 2, t, ti + 1)
        }
        c => {
            if ti >= t.len() || t[ti] != c {
                return false;
            }
            match_at(p, pi + 1, t, ti + 1)
        }
    }
}

/// pattern starts at p[0]='['. Returns (matched, characters_consumed_in_pattern).
/// If no closing ']' found, treats the '[' as a literal.
fn match_class(p: &[char], ch: char) -> (bool, usize) {
    // find closing ]
    let mut end = 1;
    let mut negate = false;
    if end < p.len() && (p[end] == '!' || p[end] == '^') {
        negate = true;
        end += 1;
    }
    let body_start = end;
    if end < p.len() && p[end] == ']' {
        // ']' as first char is literal in glob
        end += 1;
    }
    while end < p.len() && p[end] != ']' {
        end += 1;
    }
    if end >= p.len() {
        // no closing bracket - treat '[' literally
        return (ch == '[', 1);
    }
    let body = &p[body_start..end];
    let mut found = false;
    let mut i = 0;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == '-' {
            if ch >= body[i] && ch <= body[i + 2] {
                found = true;
            }
            i += 3;
        } else {
            if ch == body[i] {
                found = true;
            }
            i += 1;
        }
    }
    let matched = if negate { !found } else { found };
    (matched, end + 1)
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn literal() {
        assert!(matches("foo", "foo"));
        assert!(!matches("foo", "bar"));
    }
    #[test]
    fn star() {
        assert!(matches("f*", "foo"));
        assert!(matches("*o", "foo"));
        assert!(matches("*", ""));
        assert!(matches("a*b*c", "abc"));
        assert!(matches("a*b*c", "axxbyyc"));
        assert!(!matches("a*b", "ac"));
    }
    #[test]
    fn question() {
        assert!(matches("?oo", "foo"));
        assert!(!matches("?oo", "fo"));
    }
    #[test]
    fn class() {
        assert!(matches("[abc]", "a"));
        assert!(matches("[a-z]", "m"));
        assert!(!matches("[a-z]", "M"));
        assert!(matches("[!a-z]", "M"));
        assert!(matches("[^abc]", "d"));
    }
    #[test]
    fn escape() {
        assert!(matches("\\*", "*"));
        assert!(!matches("\\*", "a"));
    }
}
