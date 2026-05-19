//! Bash-style history expansion (`!!`, `!n`, `!string`, `^old^new^`).
//!
//! Implementation follows histexpand.c in bash-5.2.21: invoked from
//! `read_logical_command` on the raw input line *before* lexing when the
//! `histexpand` shopt is on (default for interactive shells). Returns the
//! rewritten line plus a flag indicating whether any substitution occurred.

use cherubsh_common::history::HistoryTable;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct ExpandResult {
    pub line: String,
    pub changed: bool,
    pub error: Option<String>,
    pub print_only: bool,
}

pub fn expand(input: &str, history: &HistoryTable) -> ExpandResult {
    expand_with_options(input, history, false)
}

pub fn expand_with_options(input: &str, history: &HistoryTable, posix: bool) -> ExpandResult {
    if !needs_expansion(input) {
        return ExpandResult {
            line: input.to_string(),
            changed: false,
            error: None,
            print_only: false,
        };
    }
    expand_inner(input, history, posix)
}

fn needs_expansion(line: &str) -> bool {
    let mut prev = ' ';
    for c in line.chars() {
        if c == '!' && prev != '\\' {
            return true;
        }
        prev = c;
    }
    line.starts_with('^')
}

fn expand_inner(input: &str, history: &HistoryTable, posix: bool) -> ExpandResult {
    let mut out = String::with_capacity(input.len());
    let mut changed = false;
    let mut error: Option<String> = None;
    let mut print_only = false;

    // `^old^new^` quick substitution - only valid at start of line.
    let (input_body, line_ending) = input
        .strip_suffix('\n')
        .map(|body| (body, "\n"))
        .unwrap_or((input, ""));
    if let Some(stripped) = input_body.strip_prefix('^') {
        let parts: Vec<&str> = stripped.splitn(3, '^').collect();
        if parts.len() >= 2 {
            let old = parts[0];
            let new = parts[1];
            if let Some(last) = history.last() {
                let replaced = format!("{}{}", last.line.replacen(old, new, 1), line_ending);
                return ExpandResult {
                    line: replaced,
                    changed: true,
                    error: None,
                    print_only: false,
                };
            }
            return ExpandResult {
                line: input.to_string(),
                changed: false,
                error: Some("no previous history".into()),
                print_only: false,
            };
        }
    }

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut double_cmdsub_depth = 0usize;
    let mut double_cmdsub_single = false;
    let mut double_cmdsub_double = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '#' && !in_single && is_history_comment_start(&out) {
            out.extend(chars[i..].iter());
            break;
        }
        if in_double && double_cmdsub_depth > 0 {
            if double_cmdsub_single {
                if c == '\'' {
                    double_cmdsub_single = false;
                }
                out.push(c);
                i += 1;
                continue;
            }
            if c == '\'' && !double_cmdsub_double {
                double_cmdsub_single = true;
                out.push(c);
                i += 1;
                continue;
            }
            if c == '"' {
                double_cmdsub_double = !double_cmdsub_double;
                out.push(c);
                i += 1;
                continue;
            }
            if !double_cmdsub_double {
                if c == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
                    double_cmdsub_depth += 1;
                    out.push(c);
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if c == '(' {
                    double_cmdsub_depth += 1;
                    out.push(c);
                    i += 1;
                    continue;
                }
                if c == ')' {
                    double_cmdsub_depth = double_cmdsub_depth.saturating_sub(1);
                    out.push(c);
                    i += 1;
                    continue;
                }
            }
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '$' && in_double && i + 1 < chars.len() && chars[i + 1] == '(' {
            double_cmdsub_depth = 1;
            double_cmdsub_single = false;
            double_cmdsub_double = false;
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '!' && !in_single && !(posix && in_double) {
            if in_double && double_cmdsub_depth == 0 && chars.get(i + 1) == Some(&'\'') {
                error = Some("!': event not found".into());
                out.push(c);
                i += 1;
                continue;
            }
            if is_literal_bang_context(&chars, i) {
                out.push(c);
                i += 1;
                continue;
            }
            match expand_reference(&chars, i, &out, history) {
                Some(Ok(expanded)) => {
                    out.push_str(&expanded.text);
                    if expanded.changed {
                        changed = true;
                    }
                    if expanded.print_only {
                        print_only = true;
                    }
                    i += 1 + expanded.consumed;
                }
                Some(Err(e)) => {
                    error = Some(e);
                    out.push(c);
                    i += 1;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    ExpandResult {
        line: out,
        changed,
        error,
        print_only,
    }
}

#[derive(Debug)]
enum Event {
    Bang,              // !!
    Default,           // !:N, !$, !*
    Current,           // !#
    Number(i64),       // !N, !-N
    String(String),    // !str
    Substring(String), // !?sub?
}

#[derive(Debug)]
struct ExpandedReference {
    text: String,
    consumed: usize,
    changed: bool,
    print_only: bool,
}

#[derive(Clone, Debug)]
struct LastSubstitution {
    old: String,
    new: String,
}

#[derive(Clone, Debug)]
struct EventValue {
    line: String,
    matched_word: Option<String>,
}

#[derive(Clone, Debug)]
struct Selection {
    words: Vec<String>,
}

#[derive(Clone, Debug)]
enum WordDesignator {
    Index(WordIndex),
    Range(WordIndex, WordIndex),
}

#[derive(Clone, Debug)]
enum WordIndex {
    Num(usize),
    FirstArg,
    Last,
    BeforeLast,
    Matched,
}

fn last_substitution() -> &'static Mutex<Option<LastSubstitution>> {
    static CELL: OnceLock<Mutex<Option<LastSubstitution>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn is_history_comment_start(out: &str) -> bool {
    out.is_empty()
        || out
            .chars()
            .last()
            .map(|c| c.is_whitespace() || c == ';' || c == '&' || c == '|')
            .unwrap_or(true)
}

fn is_literal_bang_context(chars: &[char], i: usize) -> bool {
    if i > 0 && chars[i - 1] == '[' {
        return true;
    }
    i >= 2 && chars[i - 1] == '{' && chars[i - 2] == '$'
}

fn expand_reference(
    chars: &[char],
    bang: usize,
    current_output: &str,
    history: &HistoryTable,
) -> Option<Result<ExpandedReference, String>> {
    let start = bang + 1;
    if start >= chars.len() {
        return None;
    }
    let (event, event_consumed) = read_event(chars, start)?;
    let default_event = matches!(event, Event::Default);
    let event_value = match resolve_event(&event, history, current_output) {
        Ok(value) => value,
        Err(e) => return Some(Err(e)),
    };
    let mut pos = start + event_consumed;
    let mut selection = Selection {
        words: vec![event_value.line.clone()],
    };

    if pos < chars.len() {
        if chars[pos] == ':' {
            if let Some((designator, consumed)) = read_word_designator(chars, pos + 1) {
                selection = apply_word_designator(&event_value, &designator);
                pos += 1 + consumed;
            }
        } else if event_consumed > 0 || default_event {
            if let Some((designator, consumed)) = read_word_designator(chars, pos) {
                selection = apply_word_designator(&event_value, &designator);
                pos += consumed;
            }
        }
    }

    let mut print_only = false;
    while pos < chars.len() && chars[pos] == ':' {
        pos += 1;
        if pos >= chars.len() {
            return Some(Err("history expansion failed".into()));
        }
        match apply_modifier(chars, pos, &mut selection) {
            Ok((consumed, modifier_print_only)) => {
                print_only |= modifier_print_only;
                pos += consumed;
            }
            Err(e) => return Some(Err(e)),
        }
    }

    Some(Ok(ExpandedReference {
        text: selection.text(),
        consumed: pos - start,
        changed: true,
        print_only,
    }))
}

fn read_event(chars: &[char], start: usize) -> Option<(Event, usize)> {
    let c = chars[start];
    if matches!(c, ':' | '^' | '$' | '*' | '%' | '-') {
        if c == '-' && start + 1 < chars.len() && chars[start + 1].is_ascii_digit() {
            let mut end = start + 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            let raw: String = chars[start..end].iter().collect();
            let n = raw.parse::<i64>().ok()?;
            return Some((Event::Number(n), end - start));
        }
        return Some((Event::Default, 0));
    }
    if c == '!' {
        return Some((Event::Bang, 1));
    }
    if c == '#' {
        return Some((Event::Current, 1));
    }
    if c == '?' {
        let mut end = start + 1;
        while end < chars.len() && chars[end] != '?' {
            end += 1;
        }
        let sub: String = chars[start + 1..end].iter().collect();
        let consumed = end - start + if end < chars.len() { 1 } else { 0 };
        return Some((Event::Substring(sub), consumed));
    }
    if c == '-' || c.is_ascii_digit() {
        let mut end = start;
        if chars[end] == '-' {
            end += 1;
        }
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
        let raw: String = chars[start..end].iter().collect();
        let n = raw.parse::<i64>().ok()?;
        return Some((Event::Number(n), end - start));
    }
    if c.is_ascii_alphabetic() || c == '_' {
        let mut end = start;
        while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
            end += 1;
        }
        let s: String = chars[start..end].iter().collect();
        return Some((Event::String(s), end - start));
    }
    None
}

fn resolve_event(
    event: &Event,
    history: &HistoryTable,
    current_output: &str,
) -> Result<EventValue, String> {
    match event {
        Event::Bang | Event::Default => history
            .last()
            .map(|e| EventValue {
                line: e.line.clone(),
                matched_word: None,
            })
            .ok_or_else(|| "no previous history".into()),
        Event::Current => Ok(EventValue {
            line: current_output.to_string(),
            matched_word: None,
        }),
        Event::Number(n) if *n > 0 => history
            .get(*n as usize)
            .map(|e| EventValue {
                line: e.line.clone(),
                matched_word: None,
            })
            .ok_or_else(|| format!("!{}: event not found", n)),
        Event::Number(n) if *n < 0 => history
            .nth_last((-n) as usize)
            .map(|e| EventValue {
                line: e.line.clone(),
                matched_word: None,
            })
            .ok_or_else(|| format!("!{}: event not found", n)),
        Event::Number(_) => Err("!0: event not found".into()),
        Event::String(s) => {
            for entry in history.iter().rev() {
                if entry.line.starts_with(s.as_str()) {
                    return Ok(EventValue {
                        line: entry.line.clone(),
                        matched_word: None,
                    });
                }
            }
            Err(format!("!{}: event not found", s))
        }
        Event::Substring(s) => {
            for entry in history.iter().rev() {
                if entry.line.contains(s.as_str()) {
                    let matched_word = history_words(&entry.line)
                        .into_iter()
                        .rev()
                        .find(|word| word.contains(s.as_str()));
                    return Ok(EventValue {
                        line: entry.line.clone(),
                        matched_word,
                    });
                }
            }
            Err(format!("!?{}?: event not found", s))
        }
    }
}

fn read_word_designator(chars: &[char], start: usize) -> Option<(WordDesignator, usize)> {
    if start >= chars.len() {
        return None;
    }
    match chars[start] {
        '^' => Some((WordDesignator::Index(WordIndex::FirstArg), 1)),
        '$' => Some((WordDesignator::Index(WordIndex::Last), 1)),
        '%' => Some((WordDesignator::Index(WordIndex::Matched), 1)),
        '*' => Some((
            WordDesignator::Range(WordIndex::FirstArg, WordIndex::Last),
            1,
        )),
        '-' => read_dash_designator(chars, start),
        c if c.is_ascii_digit() => read_number_designator(chars, start),
        _ => None,
    }
}

fn read_dash_designator(chars: &[char], start: usize) -> Option<(WordDesignator, usize)> {
    if start + 1 >= chars.len() {
        return None;
    }
    match chars[start + 1] {
        '$' => Some((WordDesignator::Range(WordIndex::Num(0), WordIndex::Last), 2)),
        c if c.is_ascii_digit() => {
            let (n, consumed) = read_usize(chars, start + 1)?;
            Some((
                WordDesignator::Range(WordIndex::Num(0), WordIndex::Num(n)),
                consumed + 1,
            ))
        }
        _ => None,
    }
}

fn read_number_designator(chars: &[char], start: usize) -> Option<(WordDesignator, usize)> {
    let (n, mut consumed) = read_usize(chars, start)?;
    let mut end = start + consumed;
    if end < chars.len() && chars[end] == '*' {
        consumed += 1;
        return Some((
            WordDesignator::Range(WordIndex::Num(n), WordIndex::Last),
            consumed,
        ));
    }
    if end < chars.len() && chars[end] == '-' {
        end += 1;
        consumed += 1;
        if end < chars.len() {
            match chars[end] {
                '$' => {
                    consumed += 1;
                    return Some((
                        WordDesignator::Range(WordIndex::Num(n), WordIndex::Last),
                        consumed,
                    ));
                }
                c if c.is_ascii_digit() => {
                    let (m, m_consumed) = read_usize(chars, end)?;
                    consumed += m_consumed;
                    return Some((
                        WordDesignator::Range(WordIndex::Num(n), WordIndex::Num(m)),
                        consumed,
                    ));
                }
                _ => {}
            }
        }
        return Some((
            WordDesignator::Range(WordIndex::Num(n), WordIndex::BeforeLast),
            consumed,
        ));
    }
    Some((WordDesignator::Index(WordIndex::Num(n)), consumed))
}

fn read_usize(chars: &[char], start: usize) -> Option<(usize, usize)> {
    let mut end = start;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    if end == start {
        return None;
    }
    let raw: String = chars[start..end].iter().collect();
    Some((raw.parse::<usize>().ok()?, end - start))
}

fn apply_word_designator(event: &EventValue, designator: &WordDesignator) -> Selection {
    let words = history_words(&event.line);
    match designator {
        WordDesignator::Index(index) => Selection {
            words: word_index(&words, event.matched_word.as_deref(), index)
                .into_iter()
                .collect(),
        },
        WordDesignator::Range(start, end) => {
            let Some(s) = word_index_number(&words, start) else {
                return Selection { words: Vec::new() };
            };
            let Some(e) = word_index_number(&words, end) else {
                return Selection { words: Vec::new() };
            };
            if s > e || s >= words.len() {
                return Selection { words: Vec::new() };
            }
            Selection {
                words: words[s..=e.min(words.len() - 1)].to_vec(),
            }
        }
    }
}

fn word_index<'a>(
    words: &'a [String],
    matched_word: Option<&'a str>,
    index: &WordIndex,
) -> Option<String> {
    match index {
        WordIndex::Matched => matched_word.map(|s| s.to_string()),
        _ => word_index_number(words, index).and_then(|idx| words.get(idx).cloned()),
    }
}

fn word_index_number(words: &[String], index: &WordIndex) -> Option<usize> {
    match index {
        WordIndex::Num(n) => Some(*n),
        WordIndex::FirstArg => Some(1),
        WordIndex::Last => words.len().checked_sub(1),
        WordIndex::BeforeLast => words.len().checked_sub(2),
        WordIndex::Matched => None,
    }
}

fn apply_modifier(
    chars: &[char],
    start: usize,
    selection: &mut Selection,
) -> Result<(usize, bool), String> {
    match chars[start] {
        'h' => {
            selection.map_words(path_head);
            Ok((1, false))
        }
        't' => {
            selection.map_words(path_tail);
            Ok((1, false))
        }
        'r' => {
            selection.map_words(path_root);
            Ok((1, false))
        }
        'e' => {
            selection.map_words(path_extension);
            Ok((1, false))
        }
        'q' => {
            let text = selection
                .words
                .iter()
                .map(|word| dequote_history_word(word))
                .collect::<Vec<_>>()
                .join(" ");
            selection.words = vec![shell_quote(&text)];
            Ok((1, false))
        }
        'x' => {
            selection.words = selection
                .words
                .iter()
                .map(|word| shell_quote(&dequote_history_word(word)))
                .collect();
            Ok((1, false))
        }
        'p' => Ok((1, true)),
        's' => {
            let (spec, consumed) = read_substitution(chars, start, false)?;
            apply_substitution(selection, &spec, false);
            remember_substitution(spec);
            Ok((consumed, false))
        }
        'g' if start + 1 < chars.len() && chars[start + 1] == 's' => {
            let (spec, consumed) = read_substitution(chars, start + 1, true)?;
            apply_substitution(selection, &spec, true);
            remember_substitution(spec);
            Ok((consumed + 1, false))
        }
        'g' if start + 1 < chars.len() && chars[start + 1] == '&' => {
            repeat_substitution(selection, true)?;
            Ok((2, false))
        }
        '&' => {
            repeat_substitution(selection, false)?;
            Ok((1, false))
        }
        other => Err(format!("!:{}: history expansion failed", other)),
    }
}

impl Selection {
    fn text(&self) -> String {
        self.words.join(" ")
    }

    fn map_words(&mut self, f: fn(&str) -> String) {
        self.words = self
            .words
            .iter()
            .map(|word| f(&dequote_history_word(word)))
            .collect();
    }
}

fn history_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            cur.push(c);
            if c == '"' {
                in_double = false;
            } else if c == '\\' {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            continue;
        }
        match c {
            '\'' => {
                cur.push(c);
                in_single = true;
            }
            '"' => {
                cur.push(c);
                in_double = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    cur.push('\\');
                    cur.push(next);
                }
            }
            c if c.is_whitespace() => flush_word(&mut words, &mut cur),
            '>' | '<' => {
                if chars.peek().copied() == Some('(') {
                    cur.push(c);
                    cur.push(chars.next().unwrap());
                    continue;
                }
                let mut op = String::new();
                if !cur.is_empty() && cur.chars().all(|ch| ch.is_ascii_digit()) {
                    op.push_str(&cur);
                    cur.clear();
                } else {
                    flush_word(&mut words, &mut cur);
                }
                op.push(c);
                if chars.peek().copied() == Some(c) {
                    op.push(chars.next().unwrap());
                }
                words.push(op);
            }
            _ => cur.push(c),
        }
    }
    flush_word(&mut words, &mut cur);
    words
}

fn flush_word(words: &mut Vec<String>, cur: &mut String) {
    if !cur.is_empty() {
        words.push(std::mem::take(cur));
    }
}

fn dequote_history_word(word: &str) -> String {
    let mut out = String::new();
    let mut chars = word.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                out.push(c);
            }
            continue;
        }
        if in_double {
            if c == '"' {
                in_double = false;
            } else if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else {
                out.push(c);
            }
            continue;
        }
        match c {
            '\'' => in_single = true,
            '"' => in_double = true,
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn path_head(s: &str) -> String {
    match s.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => s[..idx].to_string(),
        None => String::new(),
    }
}

fn path_tail(s: &str) -> String {
    s.rsplit_once('/')
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_else(|| s.to_string())
}

fn path_root(s: &str) -> String {
    let slash = s.rfind('/').map(|idx| idx + 1).unwrap_or(0);
    match s[slash..].rfind('.') {
        Some(dot) => s[..slash + dot].to_string(),
        None => s.to_string(),
    }
}

fn path_extension(s: &str) -> String {
    let slash = s.rfind('/').map(|idx| idx + 1).unwrap_or(0);
    match s[slash..].rfind('.') {
        Some(dot) => s[slash + dot..].to_string(),
        None => String::new(),
    }
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn read_substitution(
    chars: &[char],
    start: usize,
    _global: bool,
) -> Result<(LastSubstitution, usize), String> {
    let delim_pos = start + 1;
    if delim_pos >= chars.len() {
        return Err("history expansion failed".into());
    }
    let delim = chars[delim_pos];
    let (old, next) = read_until_delim(chars, delim_pos + 1, delim)?;
    let (new, end) = read_until_delim(chars, next, delim)?;
    Ok((LastSubstitution { old, new }, end - start))
}

fn read_until_delim(
    chars: &[char],
    mut pos: usize,
    delim: char,
) -> Result<(String, usize), String> {
    let mut out = String::new();
    while pos < chars.len() {
        let c = chars[pos];
        if c == delim {
            return Ok((out, pos + 1));
        }
        if c == '\\' && pos + 1 < chars.len() {
            out.push(chars[pos + 1]);
            pos += 2;
        } else {
            out.push(c);
            pos += 1;
        }
    }
    Err("history expansion failed".into())
}

fn remember_substitution(spec: LastSubstitution) {
    if let Ok(mut guard) = last_substitution().lock() {
        *guard = Some(spec);
    }
}

fn repeat_substitution(selection: &mut Selection, global: bool) -> Result<(), String> {
    let spec = last_substitution()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .ok_or_else(|| "history expansion failed".to_string())?;
    apply_substitution(selection, &spec, global);
    Ok(())
}

fn apply_substitution(selection: &mut Selection, spec: &LastSubstitution, global: bool) {
    selection.words = vec![substitute_literal(&selection.text(), spec, global)];
}

fn substitute_literal(text: &str, spec: &LastSubstitution, global: bool) -> String {
    if spec.old.is_empty() {
        return text.to_string();
    }
    let replacement = replacement_text(&spec.new, &spec.old);
    if global {
        text.replace(&spec.old, &replacement)
    } else {
        text.replacen(&spec.old, &replacement, 1)
    }
}

fn replacement_text(raw: &str, old: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else if c == '&' {
            out.push_str(old);
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherubsh_common::history::HistControl;

    #[test]
    fn quick_substitution_keeps_trailing_newline_out_of_replacement() {
        let mut history = HistoryTable::new(10);
        history.add("echo line 2 for history", HistControl::empty());

        let expanded = expand("^2^8\n", &history);

        assert_eq!(expanded.line, "echo line 8 for history\n");
        assert!(expanded.changed);
    }

    #[test]
    fn word_designators_and_path_modifiers_match_bash_cases() {
        let mut history = HistoryTable::new(10);
        history.add("/bin/sh -c 'echo this is $0'", HistControl::empty());
        history.add("echo a b c d e", HistControl::empty());
        history.add("echo file.c", HistControl::empty());

        assert_eq!(expand("echo !-3:0:t\n", &history).line, "echo sh\n");
        assert_eq!(expand("echo !-3:0:h\n", &history).line, "echo /bin\n");
        assert_eq!(expand("echo !-2:2-$\n", &history).line, "echo b c d e\n");
        assert_eq!(expand("echo !!:$:r:q\n", &history).line, "echo 'file'\n");
    }

    #[test]
    fn substitution_modifiers_support_global_repeat_and_print_only() {
        let mut history = HistoryTable::new(10);
        history.add("echo foo.c foo.o", HistControl::empty());

        let first = expand("!!:gs/foo/bar/\n", &history);
        assert_eq!(first.line, "echo bar.c bar.o\n");

        history.add(first.line.trim_end(), HistControl::empty());
        let second = expand("!!:gs/bar/x&/:p\n", &history);
        assert_eq!(second.line, "echo xbar.c xbar.o\n");
        assert!(second.print_only);
    }

    #[test]
    fn special_bang_contexts_are_not_history_expanded() {
        let mut history = HistoryTable::new(10);
        history.add("echo old", HistControl::empty());

        assert_eq!(expand("echo ${!var}\n", &history).line, "echo ${!var}\n");
        assert_eq!(
            expand("case p in [!A-Z]) echo ok;; esac\n", &history).line,
            "case p in [!A-Z]) echo ok;; esac\n"
        );
        assert_eq!(
            expand("echo ok # !1200\n", &history).line,
            "echo ok # !1200\n"
        );
    }
}
