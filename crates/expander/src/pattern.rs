//! Bash pattern matcher. Implements POSIX shell glob plus bash extglob
//! (`?(p) *(p) +(p) @(p) !(p)`). Used by both `glob.rs` (pathname expansion +
//! `case`) and `param.rs` (`${var#pat}`, `${var%pat}`, `${var/pat/rep}`,
//! `${var^^pat}`).
//!
//! Patterns may carry CTLESC bytes - those mark the following byte as literal
//! (no meta interpretation). Used to preserve quoting through expansion.

use crate::buf::{dequote_bytes, CTLESC, CTLNUL, CTLRAW};

/// Glob options influencing matching (case-folding, extglob, etc.).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobOpts {
    pub nocaseglob: bool,
    pub extglob: bool,
    pub globasciiranges: bool,
}

#[derive(Clone, Debug)]
pub enum Tok {
    Lit(u8),
    Star,
    Question,
    Class {
        negate: bool,
        items: Vec<ClassItem>,
    },
    Ext {
        kind: ExtKind,
        branches: Vec<Vec<Tok>>,
    },
}

#[derive(Clone, Debug)]
pub enum ClassItem {
    Single(u8),
    Range(u8, u8),
    Named(NamedClass),
}

#[derive(Clone, Debug)]
enum ClassAtom {
    Item(ClassItem),
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedClass {
    Alpha,
    Alnum,
    Digit,
    Lower,
    Upper,
    Space,
    Blank,
    Print,
    Punct,
    Xdigit,
    Cntrl,
    Graph,
    Ascii,
    Word,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtKind {
    Q,    // ?(p) - zero or one
    Star, // *(p) - zero or more
    Plus, // +(p) - one or more
    At,   // @(p) - exactly one
    Bang, // !(p) - anything not matching
}

pub fn parse(pat: &[u8], opts: GlobOpts) -> Vec<Tok> {
    let mut i = 0;
    let mut invalid_class = false;
    parse_inner_checked(pat, &mut i, opts, false, &mut invalid_class)
}

fn parse_inner_checked(
    pat: &[u8],
    i: &mut usize,
    opts: GlobOpts,
    in_paren: bool,
    invalid_class: &mut bool,
) -> Vec<Tok> {
    let mut out = Vec::new();
    while *i < pat.len() {
        let b = pat[*i];
        if in_paren && (b == b'|' || b == b')') {
            return out;
        }
        if b == CTLESC && *i + 2 < pat.len() && pat[*i + 1] == CTLRAW {
            out.push(Tok::Lit(pat[*i + 2]));
            *i += 3;
            continue;
        }
        if b == CTLESC && *i + 1 < pat.len() {
            out.push(Tok::Lit(pat[*i + 1]));
            *i += 2;
            continue;
        }
        if b == CTLNUL {
            *i += 1;
            continue;
        }
        match b {
            b'*' => {
                if opts.extglob {
                    if let Some(ext) = try_parse_ext(pat, i, opts, ExtKind::Star) {
                        out.push(ext);
                        continue;
                    }
                }
                out.push(Tok::Star);
                *i += 1;
            }
            b'?' => {
                if opts.extglob {
                    if let Some(ext) = try_parse_ext(pat, i, opts, ExtKind::Q) {
                        out.push(ext);
                        continue;
                    }
                }
                out.push(Tok::Question);
                *i += 1;
            }
            b'+' if opts.extglob => {
                if let Some(ext) = try_parse_ext(pat, i, opts, ExtKind::Plus) {
                    out.push(ext);
                } else {
                    out.push(Tok::Lit(b'+'));
                    *i += 1;
                }
            }
            b'@' if opts.extglob => {
                if let Some(ext) = try_parse_ext(pat, i, opts, ExtKind::At) {
                    out.push(ext);
                } else {
                    out.push(Tok::Lit(b'@'));
                    *i += 1;
                }
            }
            b'!' if opts.extglob => {
                if let Some(ext) = try_parse_ext(pat, i, opts, ExtKind::Bang) {
                    out.push(ext);
                } else {
                    out.push(Tok::Lit(b'!'));
                    *i += 1;
                }
            }
            b'[' => {
                if let Some((cls, end)) = try_parse_class(pat, *i) {
                    out.push(cls);
                    *i = end;
                } else {
                    *invalid_class = true;
                    out.push(Tok::Lit(b'['));
                    *i += 1;
                }
            }
            b'\\' if *i + 1 < pat.len() => {
                if *i + 3 < pat.len() && pat[*i + 1] == CTLESC && pat[*i + 2] == CTLRAW {
                    out.push(Tok::Lit(pat[*i + 3]));
                    *i += 4;
                } else if pat[*i + 1] == CTLESC {
                    out.push(Tok::Lit(b'\\'));
                    *i += 1;
                } else {
                    out.push(Tok::Lit(pat[*i + 1]));
                    *i += 2;
                }
            }
            other => {
                out.push(Tok::Lit(other));
                *i += 1;
            }
        }
    }
    out
}

fn try_parse_ext(pat: &[u8], i: &mut usize, opts: GlobOpts, kind: ExtKind) -> Option<Tok> {
    if *i + 1 >= pat.len() || pat[*i + 1] != b'(' {
        return None;
    }
    let mut j = *i + 2;
    let mut branches = Vec::new();
    let mut invalid_class = false;
    loop {
        let branch = parse_inner_checked(pat, &mut j, opts, true, &mut invalid_class);
        if invalid_class {
            return None;
        }
        branches.push(branch);
        if j < pat.len() && pat[j] == b'|' {
            j += 1;
            continue;
        }
        if j < pat.len() && pat[j] == b')' {
            j += 1;
            *i = j;
            return Some(Tok::Ext { kind, branches });
        }
        return None;
    }
}

fn try_parse_class(pat: &[u8], start: usize) -> Option<(Tok, usize)> {
    let mut j = start + 1;
    if j >= pat.len() {
        return None;
    }
    let negate = matches!(pat[j], b'!' | b'^');
    if negate {
        j += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    while j < pat.len() {
        if pat[j] == b']' && !first {
            return Some((Tok::Class { negate, items }, j + 1));
        }

        let lo = parse_class_atom(pat, &mut j);
        if j + 1 < pat.len() && pat[j] == b'-' && pat[j + 1] != b']' {
            j += 1;
            let hi = parse_class_atom(pat, &mut j);
            if let (
                ClassAtom::Item(ClassItem::Single(lo)),
                ClassAtom::Item(ClassItem::Single(hi)),
            ) = (lo, hi)
            {
                items.push(ClassItem::Range(lo, hi));
            }
        } else if let ClassAtom::Item(item) = lo {
            items.push(item);
        }
        first = false;
    }
    None
}

fn parse_class_atom(pat: &[u8], j: &mut usize) -> ClassAtom {
    let c = pat[*j];
    if c == b'[' && *j + 1 < pat.len() {
        match pat[*j + 1] {
            b':' => {
                let name_start = *j + 2;
                if let Some(end) = find_bracket_expr_end(pat, name_start, b':') {
                    *j = end + 2;
                    let name = dequote_bytes(&pat[name_start..end]);
                    return parse_named_class(&name)
                        .map(ClassItem::Named)
                        .map(ClassAtom::Item)
                        .unwrap_or(ClassAtom::Invalid);
                }
            }
            b'.' => {
                let name_start = *j + 2;
                if let Some(end) = find_bracket_expr_end(pat, name_start, b'.') {
                    *j = end + 2;
                    let name = dequote_bytes(&pat[name_start..end]);
                    return parse_collating_symbol(&name)
                        .map(ClassItem::Single)
                        .map(ClassAtom::Item)
                        .unwrap_or(ClassAtom::Invalid);
                }
            }
            b'=' => {
                let name_start = *j + 2;
                if let Some(end) = find_bracket_expr_end(pat, name_start, b'=') {
                    *j = end + 2;
                    let name = dequote_bytes(&pat[name_start..end]);
                    return parse_collating_symbol(&name)
                        .map(ClassItem::Single)
                        .map(ClassAtom::Item)
                        .unwrap_or(ClassAtom::Invalid);
                }
            }
            _ => {}
        }
    }

    let lit = if c == b'\\' && *j + 1 < pat.len() {
        if *j + 3 < pat.len() && pat[*j + 1] == CTLESC && pat[*j + 2] == CTLRAW {
            *j += 4;
            pat[*j - 1]
        } else if pat[*j + 1] == CTLESC {
            *j += 1;
            b'\\'
        } else {
            *j += 2;
            pat[*j - 1]
        }
    } else if c == CTLESC && *j + 2 < pat.len() && pat[*j + 1] == CTLRAW {
        *j += 3;
        pat[*j - 1]
    } else if c == CTLESC && *j + 1 < pat.len() {
        *j += 2;
        pat[*j - 1]
    } else {
        *j += 1;
        c
    };
    ClassAtom::Item(ClassItem::Single(lit))
}

fn find_bracket_expr_end(pat: &[u8], start: usize, marker: u8) -> Option<usize> {
    (start..pat.len()).find(|&k| pat[k] == marker && k + 1 < pat.len() && pat[k + 1] == b']')
}

fn parse_named_class(name: &[u8]) -> Option<NamedClass> {
    Some(match name {
        b"alpha" => NamedClass::Alpha,
        b"alnum" => NamedClass::Alnum,
        b"digit" => NamedClass::Digit,
        b"lower" => NamedClass::Lower,
        b"upper" => NamedClass::Upper,
        b"space" => NamedClass::Space,
        b"blank" => NamedClass::Blank,
        b"print" => NamedClass::Print,
        b"punct" => NamedClass::Punct,
        b"xdigit" => NamedClass::Xdigit,
        b"cntrl" => NamedClass::Cntrl,
        b"graph" => NamedClass::Graph,
        b"ascii" => NamedClass::Ascii,
        b"word" => NamedClass::Word,
        _ => return None,
    })
}

fn parse_collating_symbol(name: &[u8]) -> Option<u8> {
    if name.len() == 1 {
        return Some(name[0]);
    }
    Some(match name {
        b"hyphen" => b'-',
        b"space" => b' ',
        b"tab" => b'\t',
        b"newline" => b'\n',
        b"grave-accent" => b'`',
        _ => return None,
    })
}

fn named_matches(cls: NamedClass, b: u8) -> bool {
    match cls {
        NamedClass::Alpha => b.is_ascii_alphabetic(),
        NamedClass::Alnum => b.is_ascii_alphanumeric(),
        NamedClass::Digit => b.is_ascii_digit(),
        NamedClass::Lower => b.is_ascii_lowercase(),
        NamedClass::Upper => b.is_ascii_uppercase(),
        NamedClass::Space => b.is_ascii_whitespace(),
        NamedClass::Blank => matches!(b, b' ' | b'\t'),
        NamedClass::Print => (0x20..=0x7e).contains(&b),
        NamedClass::Punct => b.is_ascii_punctuation(),
        NamedClass::Xdigit => b.is_ascii_hexdigit(),
        NamedClass::Cntrl => b.is_ascii_control(),
        NamedClass::Graph => b.is_ascii_graphic(),
        NamedClass::Ascii => b < 128,
        NamedClass::Word => b.is_ascii_alphanumeric() || b == b'_',
    }
}

fn class_matches(items: &[ClassItem], b: u8, opts: GlobOpts) -> bool {
    let fold = |c: u8| -> u8 {
        if opts.nocaseglob {
            c.to_ascii_lowercase()
        } else {
            c
        }
    };
    let target = fold(b);
    for item in items {
        match *item {
            ClassItem::Single(s) => {
                if fold(s) == target {
                    return true;
                }
            }
            ClassItem::Range(lo, hi) => {
                let (l, h) = (fold(lo), fold(hi));
                let (lo2, hi2) = if l <= h { (l, h) } else { (h, l) };
                if target >= lo2 && target <= hi2 {
                    return true;
                }
            }
            ClassItem::Named(n) => {
                if named_matches(n, b) {
                    return true;
                }
            }
        }
    }
    false
}

#[inline]
fn lit_eq(a: u8, b: u8, opts: GlobOpts) -> bool {
    if opts.nocaseglob {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

/// Anchored match: does `pat` match all of `text`?
pub fn fnmatch(pat: &[u8], text: &[u8], opts: GlobOpts) -> bool {
    let toks = parse(pat, opts);
    match_tokens(&toks, text, opts)
}

/// Whether a pathname pattern contains an explicit leading `.` match.
///
/// Bash allows hidden directory entries without `dotglob` only when the
/// leading dot is matched explicitly. For extglob, `@(.foo)` and `*(x).foo`
/// count, while `*`, `?`, bracket expressions, and `!(.foo)` do not.
pub fn explicitly_matches_leading_dot(pat: &[u8], opts: GlobOpts) -> bool {
    let toks = parse(pat, opts);
    tokens_explicitly_match_leading_dot(&toks)
}

pub fn explicitly_matches_dot_name(pat: &[u8], text: &[u8], opts: GlobOpts) -> bool {
    if text.first() != Some(&b'.') {
        return false;
    }
    let toks = parse(pat, opts);
    let mut ends = Vec::new();
    collect_explicit_dot_ends(&toks, text, 0, opts, &mut ends);
    ends.contains(&text.len())
}

fn tokens_explicitly_match_leading_dot(toks: &[Tok]) -> bool {
    let Some((first, rest)) = toks.split_first() else {
        return false;
    };
    match first {
        Tok::Lit(b'.') => true,
        Tok::Ext { kind, branches } => {
            if matches!(
                kind,
                ExtKind::At | ExtKind::Q | ExtKind::Star | ExtKind::Plus
            ) && branches
                .iter()
                .any(|branch| tokens_explicitly_match_leading_dot(branch))
            {
                return true;
            }
            matches!(kind, ExtKind::Q | ExtKind::Star) && tokens_explicitly_match_leading_dot(rest)
        }
        Tok::Lit(_) | Tok::Star | Tok::Question | Tok::Class { .. } => false,
    }
}

fn collect_explicit_dot_ends(
    toks: &[Tok],
    text: &[u8],
    si: usize,
    opts: GlobOpts,
    out: &mut Vec<usize>,
) {
    if si >= text.len() || text[si] != b'.' {
        return;
    }
    let Some((first, rest)) = toks.split_first() else {
        return;
    };
    match first {
        Tok::Lit(b'.') => collect_match_ends(rest, 0, text, si + 1, opts, out),
        Tok::Ext { kind, branches } => {
            if matches!(kind, ExtKind::Q | ExtKind::Star) {
                collect_explicit_dot_ends(rest, text, si, opts, out);
            }
            if matches!(
                kind,
                ExtKind::At | ExtKind::Q | ExtKind::Star | ExtKind::Plus
            ) {
                let mut branch_ends = Vec::new();
                for branch in branches {
                    collect_explicit_dot_ends(branch, text, si, opts, &mut branch_ends);
                }
                branch_ends.sort_unstable();
                branch_ends.dedup();
                for end in branch_ends {
                    collect_match_ends(rest, 0, text, end, opts, out);
                }
            }
        }
        Tok::Lit(_) | Tok::Star | Tok::Question | Tok::Class { .. } => {}
    }
}

fn match_tokens(toks: &[Tok], text: &[u8], opts: GlobOpts) -> bool {
    let mut ends = Vec::new();
    collect_match_ends(toks, 0, text, 0, opts, &mut ends);
    ends.iter().any(|&e| e == text.len())
}

/// Try every match length and return the first end-of-text that consumes all
/// `toks`. Returns the offset in `text` after the match.
fn match_at(toks: &[Tok], ti: usize, text: &[u8], si: usize, opts: GlobOpts) -> Option<usize> {
    if ti >= toks.len() {
        return Some(si);
    }
    match &toks[ti] {
        Tok::Lit(b) => {
            if si < text.len() && lit_eq(*b, text[si], opts) {
                match_at(toks, ti + 1, text, si + 1, opts)
            } else {
                None
            }
        }
        Tok::Question => {
            if si < text.len() {
                match_at(toks, ti + 1, text, si + 1, opts)
            } else {
                None
            }
        }
        Tok::Class { negate, items } => {
            if si < text.len() {
                let is_in = class_matches(items, text[si], opts);
                if is_in != *negate {
                    match_at(toks, ti + 1, text, si + 1, opts)
                } else {
                    None
                }
            } else {
                None
            }
        }
        Tok::Star => {
            // Try matching 0..=remaining chars.
            let mut k = si;
            loop {
                if let Some(e) = match_at(toks, ti + 1, text, k, opts) {
                    return Some(e);
                }
                if k >= text.len() {
                    return None;
                }
                k += 1;
            }
        }
        Tok::Ext { kind, branches } => match_ext(*kind, branches, toks, ti, text, si, opts),
    }
}

fn match_ext(
    kind: ExtKind,
    branches: &[Vec<Tok>],
    toks: &[Tok],
    ti: usize,
    text: &[u8],
    si: usize,
    opts: GlobOpts,
) -> Option<usize> {
    let try_branch = |start: usize| -> Vec<usize> {
        let mut ends = Vec::new();
        for br in branches {
            collect_match_ends(br, 0, text, start, opts, &mut ends);
        }
        ends.sort_unstable();
        ends.dedup();
        ends
    };

    match kind {
        ExtKind::At => {
            for end in try_branch(si) {
                if let Some(e) = match_at(toks, ti + 1, text, end, opts) {
                    return Some(e);
                }
            }
            None
        }
        ExtKind::Q => {
            // Zero matches: skip
            if let Some(e) = match_at(toks, ti + 1, text, si, opts) {
                return Some(e);
            }
            for end in try_branch(si) {
                if let Some(e) = match_at(toks, ti + 1, text, end, opts) {
                    return Some(e);
                }
            }
            None
        }
        ExtKind::Plus | ExtKind::Star => {
            // One or more (Plus) / zero or more (Star) repetitions of any branch.
            let mut stack: Vec<usize> = vec![si];
            let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut tried_zero = false;
            if matches!(kind, ExtKind::Star) {
                if let Some(e) = match_at(toks, ti + 1, text, si, opts) {
                    return Some(e);
                }
                tried_zero = true;
            }
            let _ = tried_zero;
            let mut frontier: Vec<usize> = Vec::new();
            for s in &stack {
                for end in try_branch(*s) {
                    if end > *s && visited.insert(end) {
                        frontier.push(end);
                    }
                }
            }
            while let Some(cur) = frontier.pop() {
                if let Some(e) = match_at(toks, ti + 1, text, cur, opts) {
                    return Some(e);
                }
                for end in try_branch(cur) {
                    if end > cur && visited.insert(end) {
                        frontier.push(end);
                    }
                }
            }
            None
        }
        ExtKind::Bang => {
            // Any prefix that is NOT matched by branches, followed by the rest.
            let mut k = si;
            loop {
                // Check whether ANY branch matches all of text[si..k]. If not,
                // text[si..k] is a candidate for `!(p)`.
                let mut branch_matches = false;
                for br in branches {
                    let mut ends = Vec::new();
                    collect_match_ends(br, 0, text, si, opts, &mut ends);
                    if ends.contains(&k) {
                        branch_matches = true;
                        break;
                    }
                }
                if !branch_matches {
                    if let Some(e) = match_at(toks, ti + 1, text, k, opts) {
                        return Some(e);
                    }
                }
                if k >= text.len() {
                    return None;
                }
                k += 1;
            }
        }
    }
}

fn collect_match_ends(
    toks: &[Tok],
    ti: usize,
    text: &[u8],
    si: usize,
    opts: GlobOpts,
    out: &mut Vec<usize>,
) {
    if ti >= toks.len() {
        out.push(si);
        return;
    }
    match &toks[ti] {
        Tok::Lit(b) => {
            if si < text.len() && lit_eq(*b, text[si], opts) {
                collect_match_ends(toks, ti + 1, text, si + 1, opts, out);
            }
        }
        Tok::Question => {
            if si < text.len() {
                collect_match_ends(toks, ti + 1, text, si + 1, opts, out);
            }
        }
        Tok::Class { negate, items } => {
            if si < text.len() {
                let inside = class_matches(items, text[si], opts);
                if inside != *negate {
                    collect_match_ends(toks, ti + 1, text, si + 1, opts, out);
                }
            }
        }
        Tok::Star => {
            for k in si..=text.len() {
                collect_match_ends(toks, ti + 1, text, k, opts, out);
            }
        }
        Tok::Ext { kind, branches } => {
            // For collection mode, try every possible end the ext-pattern would match.
            let candidates = enumerate_ext_ends(*kind, branches, text, si, opts);
            for end in candidates {
                collect_match_ends(toks, ti + 1, text, end, opts, out);
            }
        }
    }
}

fn enumerate_ext_ends(
    kind: ExtKind,
    branches: &[Vec<Tok>],
    text: &[u8],
    si: usize,
    opts: GlobOpts,
) -> Vec<usize> {
    let mut ends = std::collections::HashSet::new();
    match kind {
        ExtKind::At | ExtKind::Plus | ExtKind::Star | ExtKind::Q | ExtKind::Bang => {}
    }
    let one_step = |from: usize| -> Vec<usize> {
        let mut r = Vec::new();
        for br in branches {
            collect_match_ends(br, 0, text, from, opts, &mut r);
        }
        r
    };
    match kind {
        ExtKind::At => one_step(si),
        ExtKind::Q => {
            ends.insert(si);
            for e in one_step(si) {
                ends.insert(e);
            }
            ends.into_iter().collect()
        }
        ExtKind::Star | ExtKind::Plus => {
            let mut frontier: Vec<usize> = Vec::new();
            let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
            if matches!(kind, ExtKind::Star) {
                ends.insert(si);
            }
            for e in one_step(si) {
                if e > si && seen.insert(e) {
                    frontier.push(e);
                }
            }
            while let Some(c) = frontier.pop() {
                ends.insert(c);
                for e in one_step(c) {
                    if e > c && seen.insert(e) {
                        frontier.push(e);
                    }
                }
            }
            ends.into_iter().collect()
        }
        ExtKind::Bang => {
            let mut result = Vec::new();
            for k in si..=text.len() {
                let mut br_matched = false;
                for br in branches {
                    let mut r = Vec::new();
                    collect_match_ends(br, 0, text, si, opts, &mut r);
                    if r.contains(&k) {
                        br_matched = true;
                        break;
                    }
                }
                if !br_matched {
                    result.push(k);
                }
            }
            result
        }
    }
}

/// Strip the matching prefix/suffix from `s`. `prefix=true` removes leading
/// match; `prefix=false` removes trailing match. `longest` picks the greedy
/// vs the shortest match (`##` / `%%` vs `#` / `%`).
pub fn remove_pattern(s: &[u8], pat: &[u8], prefix: bool, longest: bool) -> Vec<u8> {
    remove_pattern_with_opts(s, pat, prefix, longest, GlobOpts::default())
}

pub fn remove_pattern_with_opts(
    s: &[u8],
    pat: &[u8],
    prefix: bool,
    longest: bool,
    opts: GlobOpts,
) -> Vec<u8> {
    let toks = parse(pat, opts);
    if toks.is_empty() {
        return s.to_vec();
    }
    if prefix {
        let mut hits = Vec::new();
        collect_match_ends(&toks, 0, s, 0, opts, &mut hits);
        if hits.is_empty() {
            return s.to_vec();
        }
        let end = if longest {
            *hits.iter().max().unwrap()
        } else {
            *hits.iter().min().unwrap()
        };
        s[end..].to_vec()
    } else {
        // Suffix: find a starting offset k where toks fully matches s[k..].
        let mut best: Option<usize> = None;
        for k in 0..=s.len() {
            let mut ends = Vec::new();
            collect_match_ends(&toks, 0, s, k, opts, &mut ends);
            if ends.iter().any(|e| *e == s.len()) {
                match best {
                    None => best = Some(k),
                    Some(b) => {
                        if longest {
                            best = Some(b.min(k));
                        } else {
                            best = Some(b.max(k));
                        }
                    }
                }
            }
        }
        match best {
            Some(k) => s[..k].to_vec(),
            None => s.to_vec(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatSubMode {
    First,
    All,
    PrefixAnchored,
    SuffixAnchored,
}

/// Replace occurrences of `pat` in `s` with `rep`. `mode` controls scope.
pub fn pat_subst(s: &[u8], pat: &[u8], rep: &[u8], mode: PatSubMode) -> Vec<u8> {
    pat_subst_with_opts(s, pat, rep, mode, GlobOpts::default())
}

pub fn pat_subst_with_opts(
    s: &[u8],
    pat: &[u8],
    rep: &[u8],
    mode: PatSubMode,
    opts: GlobOpts,
) -> Vec<u8> {
    pat_subst_with_replacer(s, pat, mode, opts, |_| rep.to_vec())
}

pub fn pat_subst_with_replacer(
    s: &[u8],
    pat: &[u8],
    mode: PatSubMode,
    opts: GlobOpts,
    mut replacement: impl FnMut(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    let toks = parse(pat, opts);
    if toks.is_empty() {
        return match mode {
            PatSubMode::PrefixAnchored => {
                let rep = replacement(b"");
                let mut out = Vec::with_capacity(rep.len() + s.len());
                out.extend_from_slice(&rep);
                out.extend_from_slice(s);
                out
            }
            PatSubMode::SuffixAnchored => {
                let rep = replacement(b"");
                let mut out = Vec::with_capacity(s.len() + rep.len());
                out.extend_from_slice(s);
                out.extend_from_slice(&rep);
                out
            }
            _ => s.to_vec(),
        };
    }
    if s.is_empty()
        && matches!(mode, PatSubMode::First | PatSubMode::All)
        && star_pattern_matches_empty_for_substitution(&toks)
    {
        return replacement(b"");
    }
    let mut out = Vec::with_capacity(s.len());
    match mode {
        PatSubMode::PrefixAnchored => {
            let mut ends = Vec::new();
            collect_match_ends(&toks, 0, s, 0, opts, &mut ends);
            if let Some(e) = ends.iter().copied().max() {
                let rep = replacement(&s[..e]);
                out.extend_from_slice(&rep);
                out.extend_from_slice(&s[e..]);
            } else {
                out.extend_from_slice(s);
            }
            return out;
        }
        PatSubMode::SuffixAnchored => {
            let mut best: Option<usize> = None;
            for k in 0..=s.len() {
                let mut ends = Vec::new();
                collect_match_ends(&toks, 0, s, k, opts, &mut ends);
                if ends.iter().any(|e| *e == s.len()) {
                    match best {
                        None => best = Some(k),
                        Some(b) => best = Some(b.min(k)),
                    }
                }
            }
            match best {
                Some(k) => {
                    out.extend_from_slice(&s[..k]);
                    let rep = replacement(&s[k..]);
                    out.extend_from_slice(&rep);
                }
                None => out.extend_from_slice(s),
            }
            return out;
        }
        _ => {}
    }
    let mut i = 0;
    while i <= s.len() {
        let mut ends = Vec::new();
        collect_match_ends(&toks, 0, s, i, opts, &mut ends);
        // Prefer longest non-zero match starting here.
        let chosen = ends.iter().copied().filter(|e| *e > i).max();
        match chosen {
            Some(e) => {
                let rep = replacement(&s[i..e]);
                out.extend_from_slice(&rep);
                i = e;
                if matches!(mode, PatSubMode::First) {
                    out.extend_from_slice(&s[i..]);
                    return out;
                }
            }
            None => {
                if i < s.len() {
                    out.push(s[i]);
                }
                i += 1;
            }
        }
    }
    out
}

fn star_pattern_matches_empty_for_substitution(toks: &[Tok]) -> bool {
    !toks.is_empty()
        && toks.iter().all(|tok| {
            matches!(
                tok,
                Tok::Star
                    | Tok::Ext {
                        kind: ExtKind::Star,
                        ..
                    }
            )
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseModMode {
    UpperFirst,
    UpperAll,
    LowerFirst,
    LowerAll,
    ToggleFirst,
    ToggleAll,
}

/// `${var^pat}`, `${var^^pat}`, `${var,pat}`, `${var,,pat}`, `${var~pat}`,
/// `${var~~pat}`. `pat` is optional (None = match any char).
pub fn casemod(s: &[u8], pat: Option<&[u8]>, mode: CaseModMode) -> Vec<u8> {
    casemod_with_opts(s, pat, mode, GlobOpts::default())
}

pub fn casemod_with_opts(
    s: &[u8],
    pat: Option<&[u8]>,
    mode: CaseModMode,
    opts: GlobOpts,
) -> Vec<u8> {
    let toks = pat.map(|p| parse(p, opts));
    let mut out = Vec::with_capacity(s.len());
    let first_mode = matches!(
        mode,
        CaseModMode::UpperFirst | CaseModMode::LowerFirst | CaseModMode::ToggleFirst
    );
    let mut i = 0;
    while i < s.len() {
        let matched = match &toks {
            None => true,
            Some(t) => {
                let mut ends = Vec::new();
                collect_match_ends(t, 0, s, i, opts, &mut ends);
                ends.iter().any(|e| *e == i + 1)
            }
        };
        let b = s[i];
        let new_b = if matched && (!first_mode || i == 0) {
            match mode {
                CaseModMode::UpperFirst | CaseModMode::UpperAll => b.to_ascii_uppercase(),
                CaseModMode::LowerFirst | CaseModMode::LowerAll => b.to_ascii_lowercase(),
                CaseModMode::ToggleFirst | CaseModMode::ToggleAll => {
                    if b.is_ascii_lowercase() {
                        b.to_ascii_uppercase()
                    } else if b.is_ascii_uppercase() {
                        b.to_ascii_lowercase()
                    } else {
                        b
                    }
                }
            }
        } else {
            b
        };
        out.push(new_b);
        i += 1;
    }
    out
}

/// Quick syntactic check: does the byte slice contain any glob metacharacters?
pub fn has_glob_meta(s: &[u8], opts: GlobOpts) -> bool {
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        if b == CTLESC && i + 2 < s.len() && s[i + 1] == CTLRAW {
            i += 3;
            continue;
        }
        if b == CTLESC {
            i += 2;
            continue;
        }
        if b == b'\\' && i + 1 < s.len() {
            if s[i + 1] == CTLESC {
                i += 1;
            } else {
                i += 2;
            }
            continue;
        }
        if matches!(b, b'*' | b'?') {
            return true;
        }
        if b == b'[' && try_parse_pathname_class_end(s, i).is_some() {
            return true;
        }
        if opts.extglob {
            if matches!(b, b'@' | b'+' | b'!') && i + 1 < s.len() && s[i + 1] == b'(' {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn try_parse_pathname_class_end(pat: &[u8], start: usize) -> Option<usize> {
    let mut j = start + 1;
    if j >= pat.len() {
        return None;
    }
    if matches!(pat[j], b'!' | b'^') {
        j += 1;
    }
    let mut first = true;
    while j < pat.len() {
        if pat[j] == b'/' {
            return None;
        }
        if pat[j] == b']' && !first {
            return Some(j + 1);
        }

        let lo = parse_class_atom(pat, &mut j);
        if j + 1 < pat.len() && pat[j] == b'-' && pat[j + 1] != b']' {
            if pat[j + 1] == b'/' {
                return None;
            }
            j += 1;
            let _ = parse_class_atom(pat, &mut j);
        } else {
            let _ = lo;
        }
        first = false;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        fnmatch(p.as_bytes(), t.as_bytes(), GlobOpts::default())
    }

    #[test]
    fn star_matches_anything() {
        assert!(m("*", "abc"));
        assert!(m("a*c", "abxxxc"));
    }

    #[test]
    fn encoded_control_bytes_survive_next_to_backslash() {
        assert!(fnmatch(
            &[b'\\', CTLESC, CTLESC],
            &[b'\\', CTLESC],
            GlobOpts::default()
        ));
        assert!(fnmatch(
            &[b'\\', CTLESC, CTLNUL],
            &[b'\\', CTLNUL],
            GlobOpts::default()
        ));
        assert!(fnmatch(
            &[b'\\', CTLESC, b'x'],
            &[b'\\', b'x'],
            GlobOpts::default()
        ));
        assert!(fnmatch(
            &[CTLESC, CTLRAW, CTLNUL],
            &[CTLNUL],
            GlobOpts::default()
        ));
    }

    #[test]
    fn raw_backslash_does_not_escape_quoted_glob_meta() {
        let pat = [b'a', b'\\', CTLESC, b'*', b'b', b'*'];
        assert!(fnmatch(&pat, br"a\*b", GlobOpts::default()));
        assert!(!fnmatch(&pat, b"a*b", GlobOpts::default()));
    }

    #[test]
    fn raw_backslash_escapes_encoded_control_byte() {
        assert!(fnmatch(
            &[b'\\', CTLESC, CTLRAW, CTLESC],
            &[CTLESC],
            GlobOpts::default()
        ));
        assert!(!fnmatch(
            &[b'\\', CTLESC, CTLRAW, CTLESC],
            &[b'\\', CTLESC],
            GlobOpts::default()
        ));
        assert!(fnmatch(
            &[b'a', b'\\', CTLESC, CTLRAW, CTLNUL, b'b'],
            &[b'a', CTLNUL, b'b'],
            GlobOpts::default()
        ));
    }

    #[test]
    fn has_glob_meta_respects_pathname_bracket_and_backslash_rules() {
        let opts = GlobOpts::default();
        assert!(!has_glob_meta(br"[abc", opts));
        assert!(!has_glob_meta(br"[abc/def]", opts));
        assert!(has_glob_meta(br"[abc\/def]", opts));
        assert!(!has_glob_meta(br"a\?", opts));
        assert!(has_glob_meta(br"a\?*", opts));
        assert!(has_glob_meta(br"[[:alpha:]]", opts));
        assert!(has_glob_meta(br"[[:alpha:]", opts));
    }

    #[test]
    fn question_one_char() {
        assert!(m("a?c", "abc"));
        assert!(!m("a?c", "abbc"));
    }

    #[test]
    fn class_basic() {
        assert!(m("[abc]", "b"));
        assert!(!m("[abc]", "d"));
    }

    #[test]
    fn quoted_posix_class_names_are_dequoted() {
        let pat = [
            b'[', b'[', b':', CTLESC, b'a', CTLESC, b'l', CTLESC, b'p', CTLESC, b'h', CTLESC, b'a',
            b':', b']', b']',
        ];
        assert!(fnmatch(&pat, b"p", GlobOpts::default()));
        assert!(!fnmatch(&pat, b"1", GlobOpts::default()));
    }

    #[test]
    fn class_negate() {
        assert!(m("[!abc]", "d"));
        assert!(!m("[!abc]", "a"));
    }

    #[test]
    fn class_range() {
        assert!(m("[a-z]", "m"));
        assert!(!m("[a-z]", "1"));
    }

    #[test]
    fn named_class() {
        assert!(m("[[:digit:]]", "5"));
        assert!(!m("[[:digit:]]", "x"));
    }

    #[test]
    fn posix_collating_symbols_in_classes() {
        assert!(m("[[.a.]]", "a"));
        assert!(m("[[.a.]-[.z.]]", "m"));
        assert!(m("[[.hyphen.]-9]", "-"));
        assert!(m("[[.-.]]", "-"));
        assert!(m("[[.space.]]", " "));
        assert!(m("[[.space.][.tab.][.newline.]]", "\t"));
        assert!(!m("[[.grave-accent.]]", " "));
    }

    #[test]
    fn invalid_posix_bracket_expressions_do_not_match_as_literals() {
        assert!(!m("[[:al:])", "a"));
        assert!(!m("[[.yyz.]-[.z.]]", "c"));
        assert!(m("[[.yyz.][.a.]-z]", "c"));
        assert!(m("[[.yyz.]cde]", "c"));
        assert!(m("[[.a.]-[.zz.]p]", "p"));
        assert!(m("[[.aa.]-[.z.]p]", "p"));
    }

    #[test]
    fn posix_equivalence_classes_match_single_collating_symbol() {
        assert!(m("[[=b=]]", "b"));
        assert!(!m("[[=B=]]", "b"));
        assert!(!m("[[=b=])", "a"));
    }

    #[test]
    fn extglob_alt() {
        let opts = GlobOpts {
            extglob: true,
            ..Default::default()
        };
        assert!(fnmatch(b"@(abc|xyz)", b"xyz", opts));
        assert!(!fnmatch(b"@(abc|xyz)", b"def", opts));
    }

    #[test]
    fn extglob_zero_or_more() {
        let opts = GlobOpts {
            extglob: true,
            ..Default::default()
        };
        assert!(fnmatch(b"*(ab)", b"ababab", opts));
        assert!(fnmatch(b"*(ab)", b"", opts));
        assert!(!fnmatch(b"*(ab)", b"abx", opts));
    }

    #[test]
    fn extglob_negate() {
        let opts = GlobOpts {
            extglob: true,
            ..Default::default()
        };
        assert!(fnmatch(b"!(*.txt)", b"file.bak", opts));
        assert!(!fnmatch(b"!(*.txt)", b"file.txt", opts));
    }

    #[test]
    fn remove_prefix_basic() {
        assert_eq!(
            remove_pattern(b"foo.bar.baz", b"*.", true, false),
            b"bar.baz".to_vec()
        );
        assert_eq!(
            remove_pattern(b"foo.bar.baz", b"*.", true, true),
            b"baz".to_vec()
        );
    }

    #[test]
    fn remove_suffix_basic() {
        assert_eq!(
            remove_pattern(b"foo.bar.baz", b".*", false, false),
            b"foo.bar".to_vec()
        );
        assert_eq!(
            remove_pattern(b"foo.bar.baz", b".*", false, true),
            b"foo".to_vec()
        );
    }

    #[test]
    fn patsub_first_and_all() {
        assert_eq!(
            pat_subst(b"aaabbb", b"a", b"X", PatSubMode::First),
            b"Xaabbb".to_vec()
        );
        assert_eq!(
            pat_subst(b"aaabbb", b"a", b"X", PatSubMode::All),
            b"XXXbbb".to_vec()
        );
        assert_eq!(pat_subst(b"", b"*", b"X", PatSubMode::First), b"X".to_vec());
        assert_eq!(pat_subst(b"", b"*", b"X", PatSubMode::All), b"X".to_vec());
        assert_eq!(pat_subst(b"", b"?", b"X", PatSubMode::All), b"".to_vec());
    }

    #[test]
    fn patsub_anchored() {
        assert_eq!(
            pat_subst(b"aaabbb", b"a*", b"X", PatSubMode::PrefixAnchored),
            b"X".to_vec()
        );
        assert_eq!(
            pat_subst(b"aaabbb", b"b*", b"X", PatSubMode::SuffixAnchored),
            b"aaaX".to_vec()
        );
    }

    #[test]
    fn casemod_all() {
        assert_eq!(
            casemod(b"hello", None, CaseModMode::UpperAll),
            b"HELLO".to_vec()
        );
        assert_eq!(
            casemod(b"HELLO", None, CaseModMode::LowerAll),
            b"hello".to_vec()
        );
        assert_eq!(
            casemod(b"hello", None, CaseModMode::UpperFirst),
            b"Hello".to_vec()
        );
    }
}
