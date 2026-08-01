use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use bitflags::bitflags;

pub mod completion;
pub mod histexpand;
pub mod history;
pub mod jobs;
pub mod keymap;
pub mod signals;
pub mod target;

pub use completion::{CompAction, CompOpts, CompSlot, CompSpec};
pub use history::{HistControl, HistoryEntry, HistoryTable};
pub use jobs::{
    decode_status as decode_job_status, Job, JobFlags, JobId, JobLookupErr, JobSpec, JobState,
    JobTable, Process as JobProcess,
};
pub use keymap::{canonicalise_keyseq, EditAction, Keymap};
pub use signals::{sigsuspend_empty, SignalMaskGuard, TrapAction, TrapKind, NSIG};

pub type SourceId = u32;

pub const SOURCE_NONE: SourceId = 0;

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
pub type FastHashSet<T> = HashSet<T, BuildHasherDefault<FastHasher>>;

#[derive(Default)]
pub struct FastHasher(u64);

impl Hasher for FastHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut hash = if self.0 == 0 {
            0xcbf29ce484222325
        } else {
            self.0
        };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        self.0 = hash;
    }

    fn finish(&self) -> u64 {
        if self.0 == 0 {
            0xcbf29ce484222325
        } else {
            self.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub source: SourceId,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(source: SourceId, start: usize, end: usize) -> Self {
        Self { source, start, end }
    }

    pub fn dummy() -> Self {
        Self {
            source: SOURCE_NONE,
            start: 0,
            end: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub id: SourceId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub content: Arc<str>,
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    next_id: SourceId,
    entries: HashMap<SourceId, SourceEntry>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            entries: HashMap::new(),
        }
    }

    pub fn insert(&mut self, name: String, path: Option<PathBuf>, content: Arc<str>) -> SourceId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.insert(
            id,
            SourceEntry {
                id,
                name,
                path,
                content,
            },
        );
        id
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceEntry> {
        self.entries.get(&id)
    }

    pub fn name(&self, id: SourceId) -> Option<&str> {
        self.entries.get(&id).map(|entry| entry.name.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputKind {
    None,
    Stdin,
    String { name: String },
    File(PathBuf),
    Stream { name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellJump {
    NotJumped,
    ForceEof,
    ExitProg(i32),
    ExitBltin(i32),
    ErrExit,
    Discard,
    SigExit(i32),
}

pub type ShellResult<T> = Result<T, ShellJump>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitStatus(pub i32);

impl ExitStatus {
    pub const SUCCESS: ExitStatus = ExitStatus(0);
    pub const FAILURE: ExitStatus = ExitStatus(1);

    pub fn success() -> Self {
        Self::SUCCESS
    }

    pub fn from_wait_status(raw: i32) -> Self {
        if libc::WIFEXITED(raw) {
            Self(libc::WEXITSTATUS(raw))
        } else if libc::WIFSIGNALED(raw) {
            Self(128 + libc::WTERMSIG(raw))
        } else if libc::WIFSTOPPED(raw) {
            Self(128 + libc::WSTOPSIG(raw))
        } else {
            Self::FAILURE
        }
    }

    pub fn signaled(sig: i32) -> Self {
        Self(128 + sig)
    }

    pub fn code(&self) -> i32 {
        self.0
    }

    pub fn is_success(&self) -> bool {
        self.0 == 0
    }
}

// Command flag bits mirroring bash-5.2.21 command.h CMD_* values.
pub const CMD_WANT_SUBSHELL: u32 = 1 << 0;
pub const CMD_FORCE_SUBSHELL: u32 = 1 << 1;
pub const CMD_INVERT_RETURN: u32 = 1 << 2;
pub const CMD_IGNORE_RETURN: u32 = 1 << 3;
pub const CMD_NO_FUNCTIONS: u32 = 1 << 4;
pub const CMD_INHIBIT_EXPANSION: u32 = 1 << 5;
pub const CMD_NO_FORK: u32 = 1 << 6;
pub const CMD_TIME_PIPELINE: u32 = 1 << 7;
pub const CMD_TIME_POSIX: u32 = 1 << 8;
pub const CMD_AMPERSAND: u32 = 1 << 9;
pub const CMD_STDIN_REDIR: u32 = 1 << 10;
pub const CMD_COMMAND_BUILTIN: u32 = 1 << 11;
pub const CMD_COPROC_SUBSHELL: u32 = 1 << 12;
pub const CMD_LASTPIPE: u32 = 1 << 13;
pub const CMD_STDPATH: u32 = 1 << 14;
pub const CMD_TRY_OPTIMIZING: u32 = 1 << 15;

// Word descriptor flag bits mirroring bash-5.2.21 command.h W_* values.
pub const W_HASDOLLAR: u32 = 1 << 0;
pub const W_QUOTED: u32 = 1 << 1;
pub const W_ASSIGNMENT: u32 = 1 << 2;
pub const W_SPLITSPACE: u32 = 1 << 3;
pub const W_NOSPLIT: u32 = 1 << 4;
pub const W_NOGLOB: u32 = 1 << 5;
pub const W_NOSPLIT2: u32 = 1 << 6;
pub const W_TILDEEXP: u32 = 1 << 7;
pub const W_ARRAYREF: u32 = 1 << 8;
pub const W_NOCOMSUB: u32 = 1 << 9;
pub const W_ASSIGNRHS: u32 = 1 << 10;
pub const W_NOTILDE: u32 = 1 << 11;
pub const W_NOASSNTILDE: u32 = 1 << 12;
pub const W_EXPANDRHS: u32 = 1 << 13;
pub const W_COMPASSIGN: u32 = 1 << 14;
pub const W_ASSNBLTIN: u32 = 1 << 15;
pub const W_ASSIGNARG: u32 = 1 << 16;
pub const W_HASQUOTEDNULL: u32 = 1 << 17;
pub const W_NOPROCSUB: u32 = 1 << 18;
pub const W_SAWQUOTEDNULL: u32 = 1 << 19;
pub const W_ASSIGNASSOC: u32 = 1 << 20;
pub const W_ASSIGNARRAY: u32 = 1 << 21;
pub const W_ARRAYIND: u32 = 1 << 22;
pub const W_ASSNGLOBAL: u32 = 1 << 23;
pub const W_NOBRACE: u32 = 1 << 24;
pub const W_CHKLOCAL: u32 = 1 << 25;
pub const W_FORCELOCAL: u32 = 1 << 26;

// Internal marker inserted by the lexer when `$(...)` closes before the body of
// a heredoc it opened. The expander strips this before executing the
// substitution and emits Bash's compatibility warning.
pub const CMD_SUBST_HEREDOC_WARN_MARKER: &str = "\u{1d}cherubsh-cmdsubst-heredoc-warn\u{1d}";

// Pattern-list flags from command.h.
pub const CASEPAT_FALLTHROUGH: u32 = 0x01;
pub const CASEPAT_TESTNEXT: u32 = 0x02;

/// Variable kind discriminator for `Environment::kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    Unset,
    Scalar,
    Indexed,
    Assoc,
    Nameref,
}

bitflags! {
    /// Variable attribute bits mirroring bash-5.2.21 variables.h att_* flags.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct VarAttrs: u32 {
        const EXPORT    = 1 << 0;
        const READONLY  = 1 << 1;
        const INTEGER   = 1 << 2;
        const ARRAY     = 1 << 3;
        const ASSOC     = 1 << 4;
        const UPPERCASE = 1 << 5;
        const LOWERCASE = 1 << 6;
        const CAPCASE   = 1 << 7;
        const TRACE     = 1 << 8;
        const NAMEREF   = 1 << 9;
        const LOCAL     = 1 << 10;
    }
}

bitflags! {
    /// Quote-context bits mirroring bash-5.2.21 subst.h Q_* flags. Expansion
    /// stages thread these through to know when word-splitting, glob, and
    /// CTLESC insertion should fire.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct QFlags: u32 {
        const DOUBLE_QUOTES = 1 << 0;
        const HERE_DOCUMENT = 1 << 1;
        const NOQUOTE       = 1 << 2;
        const PATQUOTE      = 1 << 3;
        const QUOTED        = 1 << 4;
        const ADDEDQUOTES   = 1 << 5;
        const ADDEDESC      = 1 << 6;
        const DOLBRACE      = 1 << 7;
        const ARITH         = 1 << 8;
        const ARRAYSUB      = 1 << 9;
    }
}

// Redirect flag from command.h.
pub const REDIR_VARASSIGN: u32 = 0x01;

/// Plumbing for alias expansion at lex time.
pub trait AliasTable {
    fn lookup(&self, name: &str) -> Option<String>;
    fn expansion_enabled(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoAliases;

impl AliasTable for NoAliases {
    fn lookup(&self, _name: &str) -> Option<String> {
        None
    }
    fn expansion_enabled(&self) -> bool {
        false
    }
}

/// Expand aliases before parsing, following bash's command-position rule.
///
/// This intentionally lives with `Environment` rather than in the shell binary
/// so recursive parse paths such as `eval` and `source` do not silently skip
/// alias expansion.
pub fn expand_aliases_for_parse(input: &str, env: &dyn Environment) -> String {
    if !env.aliases_enabled() {
        return input.to_string();
    }

    let expanded = expand_aliases_once(input, env).0;
    if env.option("posix") && expanded.contains("$(") {
        expand_command_substitution_aliases(&expanded, env)
    } else {
        expanded
    }
}

fn expand_command_substitution_aliases(input: &str, env: &dyn Environment) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut double = false;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                out.push_str("$(");
                i += 2;
                continue;
            }
            if let Some(scan) = scan_alias_command_substitution(input, i + 2, env) {
                if scan.changed {
                    out.push_str("$(");
                    out.push_str(&scan.body);
                    out.push(')');
                    i = scan.end;
                    continue;
                }
            }
        }

        if !double && b == b'\'' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            out.push_str(&input[start..i.min(input.len())]);
            continue;
        }

        if b == b'`' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'`' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(&input[start..i.min(input.len())]);
            continue;
        }

        if b == b'\\' {
            out.push('\\');
            i += 1;
            if i < bytes.len() {
                out.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }

        if b == b'"' {
            double = !double;
        }
        out.push(b as char);
        i += 1;
    }

    out
}

struct AliasComsubScan {
    end: usize,
    body: String,
    changed: bool,
}

#[derive(Default)]
struct AliasComsubState {
    depth: usize,
    case_depth: usize,
    command_position: bool,
    alias_blank_next: bool,
    changed: bool,
    body: String,
}

fn scan_alias_command_substitution(
    input: &str,
    mut i: usize,
    env: &dyn Environment,
) -> Option<AliasComsubScan> {
    let bytes = input.as_bytes();
    let mut state = AliasComsubState {
        depth: 1,
        command_position: true,
        ..Default::default()
    };
    let mut comment_ok = true;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            state.body.push('\n');
            i += 1;
            state.command_position = true;
            state.alias_blank_next = false;
            comment_ok = true;
            continue;
        }

        if b == b'#' && comment_ok {
            while i < bytes.len() && bytes[i] != b'\n' {
                state.body.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }

        if b == b'\\' {
            state.body.push('\\');
            i += 1;
            if i < bytes.len() {
                state.body.push(bytes[i] as char);
                i += 1;
            }
            state.command_position = false;
            state.alias_blank_next = false;
            comment_ok = false;
            continue;
        }

        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
            let start = i;
            i += 2;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            state.body.push_str(&input[start..i.min(input.len())]);
            state.command_position = false;
            state.alias_blank_next = false;
            comment_ok = false;
            continue;
        }

        if matches!(b, b'\'' | b'"' | b'`') {
            let start = i;
            let quote = b;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            state.body.push_str(&input[start..i.min(input.len())]);
            state.command_position = false;
            state.alias_blank_next = false;
            comment_ok = false;
            continue;
        }

        if b == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if let Some(nested) = scan_alias_command_substitution(input, i + 2, env) {
                state.body.push_str("$(");
                state.body.push_str(&nested.body);
                state.body.push(')');
                state.changed |= nested.changed;
                i = nested.end;
                state.command_position = false;
                state.alias_blank_next = false;
                comment_ok = false;
                continue;
            }
        }

        if is_alias_word_start(b as char) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_alias_word_char(bytes[i] as char) {
                i += 1;
            }
            let word = &input[start..i];
            let eligible = state.command_position || state.alias_blank_next;
            if eligible && !matches!(bytes.get(i), Some(b'=')) {
                if let Some(alias) = expand_alias_chain(word, env) {
                    state.changed = true;
                    if feed_alias_comsub_text(&alias, &mut state) {
                        return Some(AliasComsubScan {
                            end: i,
                            body: state.body,
                            changed: true,
                        });
                    }
                    state.alias_blank_next = alias.ends_with(char::is_whitespace);
                    state.command_position = false;
                    comment_ok = false;
                    continue;
                }
            }

            state.body.push_str(word);
            feed_alias_comsub_word(word, &mut state);
            comment_ok = false;
            continue;
        }

        if feed_alias_comsub_byte(b, &mut state) {
            return Some(AliasComsubScan {
                end: i + 1,
                body: state.body,
                changed: state.changed,
            });
        }
        i += 1;
        comment_ok = b.is_ascii_whitespace() || matches!(b, b'|' | b'&' | b';' | b'<' | b'>');
    }

    None
}

fn feed_alias_comsub_text(text: &str, state: &mut AliasComsubState) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            state.body.push('\\');
            i += 1;
            if i < bytes.len() {
                state.body.push(bytes[i] as char);
                i += 1;
            }
            state.command_position = false;
            continue;
        }
        if matches!(b, b'\'' | b'"' | b'`') {
            let quote = b;
            state.body.push(quote as char);
            i += 1;
            while i < bytes.len() {
                state.body.push(bytes[i] as char);
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                    state.body.push(bytes[i] as char);
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            state.command_position = false;
            continue;
        }
        if is_alias_word_start(b as char) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_alias_word_char(bytes[i] as char) {
                i += 1;
            }
            let word = &text[start..i];
            state.body.push_str(word);
            feed_alias_comsub_word(word, state);
            continue;
        }
        if feed_alias_comsub_byte(b, state) {
            return true;
        }
        i += 1;
    }
    false
}

fn feed_alias_comsub_word(word: &str, state: &mut AliasComsubState) {
    if word == "case" {
        state.case_depth = state.case_depth.saturating_add(1);
    } else if word == "esac" && !previous_body_byte_is(state, |b| matches!(b, b'|' | b'(')) {
        state.case_depth = state.case_depth.saturating_sub(1);
    }
    if state.command_position {
        state.command_position = looks_like_assignment_word(word);
    } else {
        state.command_position = false;
    }
    state.alias_blank_next = false;
}

fn feed_alias_comsub_byte(b: u8, state: &mut AliasComsubState) -> bool {
    match b {
        b'(' => {
            state.depth = state.depth.saturating_add(1);
            state.body.push('(');
            state.command_position = true;
            state.alias_blank_next = false;
        }
        b')' => {
            if state.case_depth > 0 && state.depth == 1 {
                state.body.push(')');
                state.command_position = true;
                state.alias_blank_next = false;
            } else {
                state.depth = state.depth.saturating_sub(1);
                if state.depth == 0 {
                    return true;
                }
                state.body.push(')');
                state.command_position = true;
                state.alias_blank_next = false;
            }
        }
        b'|' | b'&' | b';' | b'<' | b'>' => {
            state.body.push(b as char);
            state.command_position = true;
            state.alias_blank_next = false;
        }
        b' ' | b'\t' | b'\r' => {
            state.body.push(b as char);
        }
        _ => {
            state.body.push(b as char);
            state.command_position = false;
            state.alias_blank_next = false;
        }
    }
    false
}

fn previous_body_byte_is(state: &AliasComsubState, choices: impl Fn(u8) -> bool) -> bool {
    state
        .body
        .as_bytes()
        .iter()
        .rev()
        .copied()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(choices)
}

fn expand_aliases_once(input: &str, env: &dyn Environment) -> (String, bool) {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    let mut command_position = true;
    let mut alias_blank_next = false;
    let mut skip_redir_target = false;
    let mut case_pending_in = false;
    let mut case_patterns = false;
    let mut open_alias_quote: Option<char> = None;
    let mut changed = false;

    while let Some((idx, ch)) = chars.next() {
        if let Some(quote) = open_alias_quote {
            out.push(ch);
            if quote == '"' && ch == '\\' {
                if let Some((_, escaped)) = chars.next() {
                    out.push(escaped);
                }
                continue;
            }
            if ch == quote {
                open_alias_quote = None;
            }
            command_position = false;
            alias_blank_next = false;
            continue;
        }

        if ch.is_whitespace() {
            out.push(ch);
            if ch == '\n' {
                command_position = !case_patterns;
                skip_redir_target = false;
            }
            continue;
        }

        if ch == '#' && command_position {
            out.push_str(&input[idx..]);
            break;
        }

        if skip_redir_target {
            out.push(ch);
            copy_word_tail(&mut chars, &mut out);
            skip_redir_target = false;
            command_position = true;
            continue;
        }

        if command_position && is_redirection_start(ch) {
            out.push(ch);
            copy_redirection_operator_tail(&mut chars, &mut out);
            skip_redir_target = true;
            continue;
        }

        if is_command_separator(ch) {
            let case_clause_end = case_patterns
                && ch == ';'
                && chars
                    .peek()
                    .is_some_and(|(_, next)| matches!(*next, ';' | '&'));
            out.push(ch);
            copy_separator_tail(ch, &mut chars, &mut out);
            if case_patterns && ch == ')' {
                command_position = true;
                case_patterns = false;
            } else if case_clause_end {
                case_patterns = true;
                command_position = false;
            } else {
                command_position = !case_patterns;
            }
            skip_redir_target = false;
            alias_blank_next = false;
            continue;
        }

        if ch == '\'' || ch == '"' {
            out.push(ch);
            copy_quoted_tail(ch, &mut chars, &mut out);
            command_position = false;
            alias_blank_next = false;
            continue;
        }

        if ch == '\\' {
            out.push(ch);
            if let Some((_, next)) = chars.next() {
                out.push(next);
            }
            command_position = false;
            alias_blank_next = false;
            continue;
        }

        if !is_alias_word_start(ch) {
            out.push(ch);
            if !skip_redir_target {
                command_position = false;
            }
            alias_blank_next = false;
            continue;
        }

        let start = idx;
        let mut end = idx + ch.len_utf8();
        while let Some((next_idx, next_ch)) = chars.peek().copied() {
            if !is_alias_word_char(next_ch) {
                break;
            }
            chars.next();
            end = next_idx + next_ch.len_utf8();
        }

        let word = &input[start..end];
        let eligible = (command_position || alias_blank_next) && !case_patterns;
        if command_position
            && chars.peek().is_some_and(|(_, c)| *c == '=')
            && is_assignment_name(word)
        {
            let (_, eq) = chars.next().expect("peeked assignment");
            out.push_str(word);
            out.push(eq);
            copy_word_tail(&mut chars, &mut out);
            command_position = true;
            alias_blank_next = false;
            continue;
        }

        if eligible && !skip_redir_target && !(env.option("posix") && is_reserved_word(word)) {
            if let Some(alias) = expand_alias_chain(word, env) {
                changed = true;
                open_alias_quote = unclosed_alias_quote(&alias);
                alias_blank_next =
                    open_alias_quote.is_none() && alias.ends_with(char::is_whitespace);
                command_position = alias_leaves_command_position(&alias, command_position);
                out.push_str(&alias);
                if alias_ends_with_number(&alias)
                    && chars
                        .peek()
                        .is_some_and(|(_, next)| matches!(*next, '<' | '>'))
                {
                    out.push(' ');
                }
                continue;
            }
        }

        out.push_str(word);
        if command_position && word == "case" {
            case_pending_in = true;
        } else if command_position && matches!(word, "do" | "then" | "else" | "elif") {
            command_position = true;
            alias_blank_next = false;
            continue;
        } else if case_pending_in && word == "in" {
            case_pending_in = false;
            case_patterns = true;
            command_position = false;
            alias_blank_next = false;
            continue;
        } else if case_patterns && word == "esac" {
            case_patterns = false;
        }
        if skip_redir_target {
            skip_redir_target = false;
            command_position = true;
        } else {
            command_position = command_position && looks_like_assignment_word(word);
        }
        alias_blank_next = false;
    }

    (out, changed)
}

fn expand_alias_chain(word: &str, env: &dyn Environment) -> Option<String> {
    let mut seen = HashSet::new();
    match expand_alias_chain_inner(word, env, &mut seen) {
        AliasExpand::Expanded(alias) => Some(alias),
        AliasExpand::NoAlias | AliasExpand::Cycle => None,
    }
}

enum AliasExpand {
    Expanded(String),
    NoAlias,
    Cycle,
}

fn expand_alias_chain_inner(
    word: &str,
    env: &dyn Environment,
    seen: &mut HashSet<String>,
) -> AliasExpand {
    if !seen.insert(word.to_string()) {
        return AliasExpand::Cycle;
    }
    let Some(alias) = env.alias_get(word) else {
        return AliasExpand::NoAlias;
    };
    if let Some(first) = first_alias_word(&alias) {
        if seen.contains(&first) {
            return if first == word && seen.len() == 1 {
                AliasExpand::Expanded(alias)
            } else {
                AliasExpand::Cycle
            };
        }
        match expand_alias_chain_inner(&first, env, seen) {
            AliasExpand::Expanded(expanded_first) => {
                return AliasExpand::Expanded(replace_first_alias_word(&alias, &expanded_first));
            }
            AliasExpand::NoAlias => {}
            AliasExpand::Cycle => return AliasExpand::Cycle,
        }
    }
    AliasExpand::Expanded(alias)
}

fn replace_first_alias_word(alias: &str, replacement: &str) -> String {
    let Some((start, end)) = first_alias_word_span(alias) else {
        return alias.to_string();
    };
    let mut out = String::with_capacity(alias.len() + replacement.len());
    out.push_str(&alias[..start]);
    out.push_str(replacement);
    out.push_str(&alias[end..]);
    out
}

fn first_alias_word_span(alias: &str) -> Option<(usize, usize)> {
    let start = alias
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(idx, _)| idx)?;
    let mut end = start;
    for (idx, ch) in alias[start..].char_indices() {
        if !is_alias_word_char(ch) {
            break;
        }
        end = start + idx + ch.len_utf8();
    }
    if end == start {
        None
    } else {
        Some((start, end))
    }
}

fn copy_word_tail(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, out: &mut String) {
    let mut single = false;
    let mut double = false;
    let mut paren_depth = 0usize;
    while let Some((_, next)) = chars.peek().copied() {
        if !single && next == '"' {
            double = !double;
            chars.next();
            out.push(next);
            continue;
        }
        if !double && next == '\'' {
            single = !single;
            chars.next();
            out.push(next);
            continue;
        }
        let in_balanced_parens = paren_depth > 0;
        if !single
            && !double
            && !in_balanced_parens
            && (next.is_whitespace() || is_command_separator(next))
        {
            break;
        }
        chars.next();
        out.push(next);
        if !single && !double {
            if next == '(' {
                paren_depth = paren_depth.saturating_add(1);
            } else if next == ')' && paren_depth > 0 {
                paren_depth -= 1;
            }
        }
    }
}

fn copy_quoted_tail(
    quote: char,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    out: &mut String,
) {
    while let Some((_, next)) = chars.next() {
        out.push(next);
        if next == '\\' && quote == '"' {
            if let Some((_, escaped)) = chars.next() {
                out.push(escaped);
            }
            continue;
        }
        if next == quote {
            break;
        }
    }
}

fn first_alias_word(alias: &str) -> Option<String> {
    let trimmed = alias.trim_start();
    let mut out = String::new();
    for ch in trimmed.chars() {
        if !is_alias_word_char(ch) {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn unclosed_alias_quote(alias: &str) -> Option<char> {
    let mut single = false;
    let mut double = false;
    let mut chars = alias.chars().peekable();
    while let Some(ch) = chars.next() {
        if single {
            if ch == '\'' {
                single = false;
            }
            continue;
        }
        if double {
            if ch == '\\' {
                chars.next();
            } else if ch == '"' {
                double = false;
            }
            continue;
        }
        match ch {
            '\\' => {
                chars.next();
            }
            '\'' => single = true,
            '"' => double = true,
            _ => {}
        }
    }
    if single {
        Some('\'')
    } else if double {
        Some('"')
    } else {
        None
    }
}

fn alias_ends_with_number(alias: &str) -> bool {
    alias
        .chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .is_some_and(|ch| ch.is_ascii_digit())
}

fn alias_leaves_command_position(alias: &str, prior_command_position: bool) -> bool {
    match alias.chars().rev().find(|ch| !ch.is_whitespace()) {
        None => prior_command_position,
        Some(ch) => is_command_separator(ch),
    }
}

fn is_reserved_word(word: &str) -> bool {
    matches!(
        word,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "case"
            | "esac"
            | "for"
            | "select"
            | "while"
            | "until"
            | "do"
            | "done"
            | "in"
            | "function"
            | "time"
            | "coproc"
    )
}

fn copy_separator_tail(
    ch: char,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    out: &mut String,
) {
    if (ch == '&' || ch == '|') && chars.peek().is_some_and(|(_, c)| *c == ch) {
        let (_, next) = chars.next().expect("peeked separator");
        out.push(next);
    }
}

fn copy_redirection_operator_tail(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    out: &mut String,
) {
    while let Some((_, next)) = chars.peek().copied() {
        if matches!(next, '<' | '>' | '&' | '|' | '-') {
            chars.next();
            out.push(next);
        } else {
            break;
        }
    }
}

fn is_command_separator(ch: char) -> bool {
    matches!(ch, ';' | '&' | '|' | '(' | ')')
}

fn is_redirection_start(ch: char) -> bool {
    matches!(ch, '<' | '>')
}

fn is_alias_word_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '.' | '-' | '!' | '%' | ':' | '@')
}

fn is_alias_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '!' | '%' | ':' | '@')
}

fn looks_like_assignment_word(word: &str) -> bool {
    let Some(eq) = word.find('=') else {
        return false;
    };
    is_assignment_name(&word[..eq])
}

fn is_assignment_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Reasons an assignment can fail when routed through attribute-enforcing
/// store mutation. Surfaced by `Environment::assign` and used by builtins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignError {
    ReadOnly(String),
    InvalidInteger(String),
    InvalidName(String),
    BadArraySubscript(String),
    CircularNameReference(String),
}

impl AssignError {
    pub fn report(&self) {
        match self {
            AssignError::ReadOnly(name) => eprintln!("cherubsh: {name}: readonly variable"),
            AssignError::InvalidInteger(value) => {
                eprintln!("cherubsh: {value}: invalid integer")
            }
            AssignError::InvalidName(name) => {
                eprintln!("cherubsh: `{name}': not a valid identifier")
            }
            AssignError::BadArraySubscript(name) => {
                eprintln!("cherubsh: {name}: bad array subscript")
            }
            AssignError::CircularNameReference(_) => {}
        }
    }
}

/// Snapshot of a variable used by `declare -p`, `set`, `export -p`,
/// `readonly -p`. Stable enough to format independently of the live store.
#[derive(Clone, Debug)]
pub struct VarSnapshot {
    pub name: String,
    pub kind: VarKind,
    pub attrs: VarAttrs,
    pub scalar: Option<String>,
    pub indexed: Option<Vec<(i64, String)>>,
    pub assoc: Option<Vec<(String, String)>>,
    pub nameref_target: Option<String>,
}

/// Snapshot of a trap binding. Pseudo-signal names ("EXIT", "ERR", "RETURN",
/// "DEBUG") are stored with their text form; numeric signals use their POSIX
/// short name (e.g. "INT", "TERM").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrapEntry {
    pub signal: String,
    pub action: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcSubstDir {
    Input,
    Output,
}

#[derive(Clone, Debug)]
pub struct ProcessSubst {
    pub dir: ProcSubstDir,
    pub command_text: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HereDocFlags {
    pub stripped: bool,
    pub quoted_delim: bool,
}

#[derive(Clone, Debug)]
pub struct ShellError {
    pub message: String,
    pub code: i32,
    pub span: Option<Span>,
}

impl ShellError {
    pub fn new<S: Into<String>>(message: S) -> Self {
        Self {
            message: message.into(),
            code: 1,
            span: None,
        }
    }

    pub fn with_code<S: Into<String>>(message: S, code: i32) -> Self {
        Self {
            message: message.into(),
            code,
            span: None,
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn report(&self) {
        eprintln!("cherubsh: {}", self.message);
    }
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cherubsh: {}", self.message)
    }
}

impl std::error::Error for ShellError {}

impl From<std::io::Error> for ShellError {
    fn from(err: std::io::Error) -> Self {
        Self {
            message: err.to_string(),
            code: 1,
            span: None,
        }
    }
}

/// Abstraction over the shell variable + positional parameter store.
/// Mirrors bash's variable lookup surface so expander and exec can operate
/// without depending on the full ShellState.
pub trait Environment {
    fn get(&self, name: &str) -> Option<String>;
    fn get_cow<'a>(&'a self, name: &str) -> Option<Cow<'a, str>> {
        self.get(name).map(Cow::Owned)
    }
    fn set(&mut self, name: &str, value: String);
    fn unset(&mut self, name: &str);
    fn exported(&self, name: &str) -> bool;
    fn export(&mut self, name: &str);

    /// Positional parameter $N (0-based: index 0 == $0).
    fn positional(&self, index: usize) -> Option<String>;
    fn positional_cow<'a>(&'a self, index: usize) -> Option<Cow<'a, str>> {
        self.positional(index).map(Cow::Owned)
    }
    fn positional_count(&self) -> usize;
    fn set_positionals(&mut self, params: Vec<String>);

    fn last_status(&self) -> i32;
    fn set_last_status(&mut self, status: i32);

    fn diagnostic_source_name(&self) -> Option<String> {
        None
    }
    fn call_stack_source_name(&self) -> Option<String> {
        self.diagnostic_source_name()
    }
    fn diagnostic_line(&self) -> Option<u32> {
        None
    }
    fn push_diagnostic_line(&mut self, _line: u32) {}
    fn pop_diagnostic_line(&mut self) {}
    fn set_current_command(&mut self, _command: Option<String>) {}
    fn next_shell_input_line(&mut self) -> Option<String> {
        None
    }
    fn enter_loadable_child(&mut self) {}
    fn arithmetic_expansion_errors_exit_shell(&self) -> bool {
        false
    }
    fn is_login_shell(&self) -> bool {
        false
    }

    /// Shell option access (errexit, nounset, pipefail, lastpipe, noclobber,
    /// xtrace, restricted, monitor). Unknown names return false.
    fn option(&self, _name: &str) -> bool {
        false
    }
    fn set_option(&mut self, _name: &str, _on: bool) {}
    fn prompt_nonprinting_markers(&self) -> bool {
        false
    }
    fn prompt_command_number(&self) -> u64 {
        0
    }
    fn prompt_history_number(&self) -> u64 {
        self.get("HISTCMD")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1)
    }
    fn prompt_job_count(&self) -> usize {
        0
    }
    fn prompt_shell_name(&self) -> Option<String> {
        self.get("BASH").or_else(|| self.positional(0))
    }
    fn logical_pwd(&self) -> Option<String> {
        self.get("PWD").filter(|pwd| !pwd.is_empty())
    }
    fn set_logical_pwd(&mut self, value: String) {
        self.set("PWD", value);
    }

    /// $! - pid of last asynchronous command.
    fn last_async_pid(&self) -> Option<i32> {
        None
    }
    fn set_last_async_pid(&mut self, _pid: i32) {}
    fn queue_coproc_cleanup(&mut self, _name: String, _pid_name: Option<String>) {}
    fn take_coproc_cleanups(&mut self) -> Vec<(String, Option<String>)> {
        Vec::new()
    }
    fn prepare_nameref_target_assignment(&mut self, _source: &str, _target: &str) {}

    /// Stable parent shell pid ($$).
    fn shell_pid(&self) -> i32 {
        unsafe { libc::getpid() }
    }
    /// Per-process pid ($BASHPID).
    fn bashpid(&self) -> i32 {
        unsafe { libc::getpid() }
    }
    /// Epoch time when this shell instance started. Used by `printf %(...)T`
    /// for bash's `-2` sentinel.
    fn shell_start_epoch(&self) -> i64 {
        unsafe { libc::time(std::ptr::null_mut()) as i64 }
    }
    /// $BASH_SUBSHELL nesting depth.
    fn subshell_level(&self) -> u32 {
        0
    }
    /// Bump subshell_level and refresh BASHPID - called in forked subshell.
    fn enter_subshell(&mut self) {}
    /// Command/process substitution nesting depth for xtrace PS4 expansion.
    fn command_substitution_depth(&self) -> u32 {
        0
    }
    fn enter_command_substitution(&mut self) {}

    /// BASH_FUNCNAME / BASH_SOURCE / BASH_LINENO stacks.
    fn funcname_push(&mut self, _name: &str, _args: &[String]) {}
    fn funcname_push_with_source(&mut self, name: &str, args: &[String], _source: &str) {
        self.funcname_push(name, args);
    }
    fn funcname_pop(&mut self) {}
    fn source_frame_push(&mut self, _source_name: &str) {}
    fn source_frame_pop(&mut self) {}

    /// Snapshot positionals (including $0 at index 0) for save/restore.
    fn positionals_clone(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.positional_count() + 1);
        let mut i = 0;
        while let Some(p) = self.positional(i) {
            out.push(p);
            i += 1;
        }
        out
    }

    /// Replace positional parameters for a function call, returning the old
    /// vector for restoration. Implementors can override this to avoid cloning
    /// the current positional store through the object-safe accessor methods.
    fn push_function_positionals(&mut self, args: &[String]) -> Vec<String> {
        let saved = self.positionals_clone();
        let mut next = Vec::with_capacity(args.len() + 1);
        next.push(
            saved
                .first()
                .cloned()
                .unwrap_or_else(|| "cherubsh".to_string()),
        );
        next.extend(args.iter().cloned());
        self.set_positionals(next);
        saved
    }

    /// Restore positional parameters saved by `push_function_positionals`,
    /// preserving changes to `$0` made while the function was running.
    fn pop_function_positionals(&mut self, mut saved: Vec<String>) {
        if let Some(current_zero) = self.positional(0) {
            if saved.is_empty() {
                saved.push(current_zero);
            } else {
                saved[0] = current_zero;
            }
        }
        self.set_positionals(saved);
    }

    /// Indexed-array store. Falls back to scalar join when backend lacks
    /// array support (still useful for PIPESTATUS scalar consumers).
    fn set_array(&mut self, name: &str, values: Vec<String>) {
        self.set(name, values.join(" "));
    }
    fn get_array(&self, _name: &str) -> Option<Vec<String>> {
        None
    }

    /// Set element of an indexed array (`arr[5]=x`).
    fn set_array_indexed(&mut self, _name: &str, _index: i64, _value: String) {}
    /// Read element of an indexed array.
    fn get_array_indexed(&self, _name: &str, _index: i64) -> Option<String> {
        None
    }
    fn get_array_indexed_cow<'a>(&'a self, name: &str, index: i64) -> Option<Cow<'a, str>> {
        self.get_array_indexed(name, index).map(Cow::Owned)
    }
    /// Return all (index, value) pairs of an indexed array in index order.
    fn get_array_all(&self, _name: &str) -> Option<Vec<(i64, String)>> {
        None
    }
    /// Indexed array subscripts.
    fn array_keys(&self, _name: &str) -> Option<Vec<i64>> {
        None
    }
    /// Number of indexed-array elements.
    fn array_len(&self, _name: &str) -> usize {
        0
    }
    /// Highest assigned indexed-array subscript.
    fn array_max_index(&self, name: &str) -> Option<i64> {
        self.array_keys(name)
            .and_then(|keys| keys.into_iter().max())
    }

    /// Set associative array element (`m[key]=x`).
    fn set_array_assoc(&mut self, _name: &str, _key: &str, _value: String) {}
    /// Read associative array element.
    fn get_array_assoc(&self, _name: &str, _key: &str) -> Option<String> {
        None
    }
    fn get_array_assoc_cow<'a>(&'a self, name: &str, key: &str) -> Option<Cow<'a, str>> {
        self.get_array_assoc(name, key).map(Cow::Owned)
    }
    /// All (key, value) pairs of an associative array.
    fn assoc_all(&self, _name: &str) -> Option<Vec<(String, String)>> {
        None
    }
    /// Associative array keys.
    fn assoc_keys(&self, _name: &str) -> Option<Vec<String>> {
        None
    }
    /// Number of associative-array entries.
    fn assoc_len(&self, name: &str) -> usize {
        self.assoc_keys(name).map(|keys| keys.len()).unwrap_or(0)
    }

    /// Variable kind for the dispatch in `${...}` expansion.
    fn kind(&self, _name: &str) -> VarKind {
        VarKind::Unset
    }
    /// Bitfield of attributes (readonly, integer, exported, array, etc.).
    fn attrs(&self, _name: &str) -> VarAttrs {
        VarAttrs::empty()
    }
    /// Mutate a single attribute. Used by `declare -i`, `declare -r`, etc.
    fn set_attr(&mut self, _name: &str, _attr: VarAttrs, _on: bool) {}
    fn global_kind(&self, name: &str) -> VarKind {
        self.kind(name)
    }
    fn global_attrs(&self, name: &str) -> VarAttrs {
        self.attrs(name)
    }
    fn global_exported(&self, name: &str) -> bool {
        self.exported(name)
    }
    fn global_get(&self, name: &str) -> Option<String> {
        self.get(name)
    }
    fn preserve_assoc_order_for_next_assignment(&mut self, _name: &str) {}
    fn take_preserve_assoc_order_for_next_assignment(&mut self, _name: &str) -> bool {
        false
    }
    fn set_assoc_print_order(&mut self, _name: &str, _keys: Option<Vec<String>>) {}
    fn global_is_readonly(&self, name: &str) -> bool {
        self.is_readonly(name)
    }
    fn global_array_keys(&self, name: &str) -> Option<Vec<i64>> {
        self.array_keys(name)
    }
    fn set_global_attr(&mut self, name: &str, attr: VarAttrs, on: bool) {
        self.set_attr(name, attr, on);
    }
    fn assign_global(&mut self, name: &str, value: String) -> Result<(), AssignError> {
        self.assign(name, value)
    }
    fn unset_global(&mut self, name: &str) {
        self.unset(name);
    }
    fn export_global(&mut self, name: &str) {
        self.export(name);
    }
    fn set_global_array(&mut self, name: &str, values: Vec<String>) {
        self.set_array(name, values);
    }
    fn set_global_array_indexed(&mut self, name: &str, index: i64, value: String) {
        self.set_array_indexed(name, index, value);
    }
    fn set_global_array_assoc(&mut self, name: &str, key: &str, value: String) {
        self.set_array_assoc(name, key, value);
    }
    fn unset_global_array_elem(&mut self, name: &str, key: &str) {
        self.unset_array_elem(name, key);
    }

    /// Resolve a nameref chain. Returns the ultimate target name (or `name`
    /// itself if `name` is not a nameref). Implementations must guard against
    /// cycles and self-reference.
    fn resolve_nameref(&self, name: &str) -> Option<String> {
        Some(name.to_string())
    }

    /// Push a local-variable scope (called on function entry).
    fn push_local_scope(&mut self) {}
    /// Pop the local-variable scope, restoring prior values.
    fn pop_local_scope(&mut self) {}
    /// Set a variable in the innermost local scope (declare -p semantics for
    /// `local` builtin and `${var:=word}` when `var` is already local).
    fn set_local(&mut self, name: &str, value: String) {
        self.set(name, value);
    }
    /// Create a local variable binding without assigning a value. This is the
    /// operation bash uses before applying `declare` attributes inside a
    /// function, so caller globals do not seed local arrays.
    fn make_local(&mut self, _name: &str) -> Result<(), AssignError> {
        Ok(())
    }
    fn make_local_with_value(
        &mut self,
        name: &str,
        value: Option<String>,
    ) -> Result<(), AssignError> {
        self.make_local(name)?;
        if let Some(value) = value {
            self.assign(name, value)?;
        }
        Ok(())
    }
    /// Create a local binding initialized from the visible variable of the same
    /// name (`declare/local -I`). Implementations should not inherit NAMEREF.
    fn make_local_inherit(&mut self, name: &str) -> Result<(), AssignError> {
        self.make_local(name)
    }
    fn set_local_restore_snapshot(&mut self, _name: &str, _snapshot: Option<VarSnapshot>) {}

    /// Remove a single array element / assoc key.
    fn unset_array_elem(&mut self, _name: &str, _key: &str) {}

    /// All variable names that begin with `prefix`. Used by `${!pfx*}` / `${!pfx@}`.
    fn all_var_names_with_prefix(&self, _prefix: &str) -> Vec<String> {
        Vec::new()
    }

    /// Raw IFS value (defaults to space/tab/newline when unset).
    fn ifs_raw(&self) -> String {
        self.get("IFS").unwrap_or_else(|| " \t\n".to_string())
    }

    /// Attribute-enforcing assignment. Honors READONLY, INTEGER (RHS is
    /// arithmetic-evaluated), UPPER/LOWER/CAPCASE, and NAMEREF (assignment
    /// routes to the target). Default impl just calls `set`; concrete envs
    /// override to enforce attributes.
    fn assign(&mut self, name: &str, value: String) -> Result<(), AssignError> {
        self.set(name, value);
        Ok(())
    }

    /// True if the named variable has the READONLY attribute set.
    fn is_readonly(&self, _name: &str) -> bool {
        false
    }

    /// All variables ordered for `set` / `declare -p` consumption (caller
    /// usually sorts by name).
    fn iter_vars(&self) -> Vec<VarSnapshot> {
        Vec::new()
    }
    /// Snapshot of one visible variable. Default keeps compatibility with
    /// lightweight test environments that only implement `iter_vars`.
    fn var_snapshot(&self, name: &str) -> Option<VarSnapshot> {
        self.iter_vars().into_iter().find(|snap| snap.name == name)
    }
    /// Direct nameref target for one visible variable, without resolving the
    /// chain. An empty or missing target indicates an unresolved nameref.
    fn nameref_target(&self, name: &str) -> Option<String> {
        self.var_snapshot(name).and_then(|snap| snap.nameref_target)
    }
    /// Variables declared in the innermost active local frame.
    fn iter_local_vars(&self) -> Vec<VarSnapshot> {
        Vec::new()
    }
    /// Whether the innermost active local frame has an option snapshot from
    /// `local -`.
    fn local_options_active(&self) -> bool {
        false
    }
    fn make_options_local(&mut self) {}

    /// Defined shell functions by name.
    fn function_names(&self) -> Vec<String> {
        Vec::new()
    }

    /// True iff a function with this name is defined.
    fn function_exists(&self, _name: &str) -> bool {
        false
    }

    /// True iff a function with this name is read-only.
    fn function_is_readonly(&self, _name: &str) -> bool {
        false
    }

    /// Mark a function read-only (`readonly -f`).
    fn function_set_readonly(&mut self, _name: &str) {}

    /// Remove a function definition (`unset -f`).
    fn function_unset(&mut self, _name: &str) {}

    /// Pretty-print form of the function definition for `declare -f name`.
    /// `None` if undefined or pretty-printing isn't available.
    fn function_pretty(&self, _name: &str) -> Option<String> {
        None
    }

    // Aliases.
    fn alias_set(&mut self, _name: &str, _value: String) {}
    fn alias_get(&self, _name: &str) -> Option<String> {
        None
    }
    fn alias_unset(&mut self, _name: &str) {}
    fn alias_iter(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    // Umask (process-level boundary; concrete env wraps libc::umask).
    fn umask_get(&self) -> u32 {
        unsafe {
            let cur = libc::umask(0);
            libc::umask(cur);
            cur as u32 & 0o777
        }
    }
    fn umask_set(&mut self, mask: u32) {
        unsafe { libc::umask(mask as libc::mode_t) };
    }

    // Trap storage (string-form).
    // Dispatch via `trap_action` / `trap_set_action`.
    fn trap_set(&mut self, _signal: &str, _action: Option<String>) {}
    fn trap_get(&self, _signal: &str) -> Option<String> {
        None
    }
    fn trap_iter(&self) -> Vec<TrapEntry> {
        Vec::new()
    }

    fn trap_action(&self, _kind: signals::TrapKind) -> Option<signals::TrapAction> {
        None
    }
    fn trap_set_action(&mut self, _kind: signals::TrapKind, _action: signals::TrapAction) {}
    fn trap_clear(&mut self, _kind: signals::TrapKind) {}
    fn trap_all(&self) -> Vec<(signals::TrapKind, signals::TrapAction)> {
        Vec::new()
    }
    /// Mark an EXIT trap inherited into a subshell. Bash keeps it visible to
    /// `trap`, but does not run it when that subshell exits unless it is reset.
    fn suppress_inherited_exit_trap(&mut self) {}
    fn inherited_exit_trap_suppressed(&self) -> bool {
        false
    }
    /// True if a non-default trap is registered for `kind`.
    fn trap_is_set(&self, kind: signals::TrapKind) -> bool {
        self.trap_action(kind).is_some()
    }

    fn jobs_table(&self) -> Option<&jobs::JobTable> {
        None
    }
    fn jobs_table_mut(&mut self) -> Option<&mut jobs::JobTable> {
        None
    }

    fn history(&self) -> Option<&history::HistoryTable> {
        None
    }
    fn history_mut(&mut self) -> Option<&mut history::HistoryTable> {
        None
    }
    fn history_last_line_added(&self) -> bool {
        false
    }
    fn histcontrol(&self) -> history::HistControl {
        history::HistControl::empty()
    }
    fn histfile(&self) -> Option<PathBuf> {
        None
    }

    fn compspec_set(
        &mut self,
        _slot: completion::CompSlot,
        _key: Option<&str>,
        _spec: completion::CompSpec,
    ) {
    }
    fn compspec_get(
        &self,
        _slot: completion::CompSlot,
        _key: Option<&str>,
    ) -> Option<completion::CompSpec> {
        None
    }
    fn compspec_remove(&mut self, _slot: completion::CompSlot, _key: Option<&str>) -> bool {
        false
    }
    fn compspec_iter(&self) -> Vec<(completion::CompSlot, Option<String>, completion::CompSpec)> {
        Vec::new()
    }
    fn completion_options_update(
        &mut self,
        _set: completion::CompOpts,
        _clear: completion::CompOpts,
    ) -> bool {
        false
    }
    fn completion_options_current(&self) -> Option<(String, completion::CompOpts)> {
        None
    }

    fn keymap_active(&self) -> &str {
        "emacs"
    }
    fn keymap_set_active(&mut self, _name: &str) {}
    fn keymap_get(&self, _name: &str) -> Option<keymap::Keymap> {
        None
    }
    fn keymap_bind(&mut self, _name: &str, _seq: &str, _action: keymap::EditAction) {}
    fn keymap_bind_macro(&mut self, _name: &str, _seq: &str, _text: &str) {}
    fn keymap_bind_shell_command(&mut self, _name: &str, _seq: &str, _command: &str) {}
    fn keymap_unbind(&mut self, _name: &str, _seq: &str) -> bool {
        false
    }
    fn keymap_list(&self) -> Vec<String> {
        Vec::new()
    }

    fn pending_signal_take(&mut self, _sig: i32) -> u32 {
        0
    }
    fn acknowledge_trapped_signal(&mut self, _sig: i32) {}
    fn running_trap(&self) -> Option<i32> {
        None
    }
    fn set_running_trap(&mut self, _sig: Option<i32>) {}
    fn run_debug_trap_hook(&mut self) {}
    fn run_err_trap_hook(&mut self) {}
    fn run_return_trap_hook(&mut self) {}
    fn run_pending_traps_hook(&mut self) {}
    fn run_exit_trap_hook(&mut self) -> Option<i32> {
        None
    }
    fn shell_pgrp(&self) -> i32 {
        unsafe { libc::getpgrp() }
    }
    fn set_shell_pgrp(&mut self, _pgrp: i32) {}
    fn tty_fd(&self) -> Option<i32> {
        None
    }
    fn set_tty_fd(&mut self, _fd: Option<i32>) {}
    fn job_control_enabled(&self) -> bool {
        false
    }
    fn set_job_control_enabled(&mut self, _on: bool) {}

    // Command hash table (for `hash`).
    fn hash_set(&mut self, _name: &str, _path: std::path::PathBuf) {}
    fn hash_get(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn hash_get_with_hit(&mut self, name: &str) -> Option<std::path::PathBuf> {
        self.hash_get(name)
    }
    fn hash_remove(&mut self, _name: &str) {}
    fn hash_clear(&mut self) {}
    fn hash_iter(&self) -> Vec<(String, std::path::PathBuf)> {
        Vec::new()
    }
    fn hash_iter_with_hits(&self) -> Vec<(String, std::path::PathBuf, u64)> {
        self.hash_iter()
            .into_iter()
            .map(|(name, path)| (name, path, 0))
            .collect()
    }

    // Directory stack (for pushd/popd/dirs). Index 0 == current dir.
    fn dirs_iter(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    fn dirs_push(&mut self, _path: std::path::PathBuf) {}
    fn dirs_pop(&mut self, _index: usize) -> Option<std::path::PathBuf> {
        None
    }
    fn dirs_set_top(&mut self, _path: std::path::PathBuf) {}
    fn dirs_set_stack(&mut self, _stack: Vec<std::path::PathBuf>) {}
    fn dirs_clear(&mut self) {}

    // Per-builtin enable/disable state for `enable -n / -a / -s`.
    fn builtin_enabled(&self, _name: &str) -> bool {
        true
    }
    fn builtin_set_enabled(&mut self, _name: &str, _on: bool) {}

    // Shopt accept_aliases / expand_aliases state shortcuts (alias lookup is
    // gated on this).
    fn aliases_enabled(&self) -> bool {
        false
    }
    fn set_aliases_enabled(&mut self, _on: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct AliasEnv {
        aliases: BTreeMap<String, String>,
        posix: bool,
    }

    impl Environment for AliasEnv {
        fn get(&self, _name: &str) -> Option<String> {
            None
        }

        fn set(&mut self, _name: &str, _value: String) {}

        fn unset(&mut self, _name: &str) {}

        fn exported(&self, _name: &str) -> bool {
            false
        }

        fn export(&mut self, _name: &str) {}

        fn positional(&self, _index: usize) -> Option<String> {
            None
        }

        fn positional_count(&self) -> usize {
            0
        }

        fn set_positionals(&mut self, _params: Vec<String>) {}

        fn last_status(&self) -> i32 {
            0
        }

        fn set_last_status(&mut self, _status: i32) {}

        fn alias_get(&self, name: &str) -> Option<String> {
            self.aliases.get(name).cloned()
        }

        fn aliases_enabled(&self) -> bool {
            true
        }

        fn option(&self, name: &str) -> bool {
            name == "posix" && self.posix
        }
    }

    #[test]
    fn source_map_assigns_unique_ids() {
        let mut map = SourceMap::new();
        let a = map.insert("a".into(), None, Arc::from(""));
        let b = map.insert("b".into(), None, Arc::from(""));
        assert_ne!(a, b);
        assert_eq!(map.name(a), Some("a"));
        assert_eq!(map.name(b), Some("b"));
    }

    #[test]
    fn exit_status_signal_offset() {
        assert_eq!(ExitStatus::signaled(15).code(), 128 + 15);
        assert!(ExitStatus::success().is_success());
    }

    #[test]
    fn shell_error_io_conversion() {
        let err: ShellError = std::io::Error::new(std::io::ErrorKind::NotFound, "missing").into();
        assert_eq!(err.code, 1);
    }

    #[test]
    fn alias_expansion_keeps_case_patterns_literal() {
        let mut env = AliasEnv::default();
        env.aliases.insert("foo".into(), "oneword".into());
        let input = "case \"$foo_word\"\nin\n\tfoo) echo ok;;\nesac\n";
        assert_eq!(expand_aliases_for_parse(input, &env), input);
    }

    #[test]
    fn alias_expansion_recurses_after_blank_alias() {
        let mut env = AliasEnv::default();
        env.aliases.insert("foo".into(), "echo ".into());
        env.aliases.insert("bar".into(), "baz".into());
        env.aliases.insert("baz".into(), "quux".into());
        assert_eq!(expand_aliases_for_parse("foo bar\n", &env), "echo  quux\n");
    }

    #[test]
    fn alias_expansion_stops_recursive_cycle_at_first_repeat() {
        let mut env = AliasEnv::default();
        env.aliases.insert("qfoo".into(), "qbar".into());
        env.aliases.insert("qbar".into(), "qbaz".into());
        env.aliases.insert("qbaz".into(), "quux".into());
        env.aliases.insert("quux".into(), "qfoo".into());
        assert_eq!(expand_aliases_for_parse("qfoo\n", &env), "qfoo\n");
    }

    #[test]
    fn alias_expansion_keeps_self_alias_suffix() {
        let mut env = AliasEnv::default();
        env.aliases.insert("echo".into(), "echo a".into());
        assert_eq!(expand_aliases_for_parse("echo b\n", &env), "echo a b\n");
    }

    #[test]
    fn alias_expansion_allows_multibyte_tail() {
        let mut env = AliasEnv::default();
        env.aliases
            .insert("a1".into(), "printf \"<%s>\\n\" áa".into());
        assert_eq!(
            expand_aliases_for_parse("a1\n", &env),
            "printf \"<%s>\\n\" áa\n"
        );
    }

    #[test]
    fn alias_expansion_applies_before_pipe_operator() {
        let mut env = AliasEnv::default();
        env.aliases
            .insert("a".into(), "printf \"<%s>\\n\" \\".into());
        assert_eq!(
            expand_aliases_for_parse("a|cat\n", &env),
            "printf \"<%s>\\n\" \\|cat\n"
        );
    }

    #[test]
    fn alias_expansion_keeps_redirection_boundary_after_numeric_alias() {
        let mut env = AliasEnv::default();
        env.aliases.insert("foo".into(), "echo 0".into());
        assert_eq!(expand_aliases_for_parse("foo>&2\n", &env), "echo 0 >&2\n");
    }

    #[test]
    fn alias_expansion_keeps_compound_assignment_metachars_together() {
        let env = AliasEnv::default();
        let input = "test=(first & second)\n";
        assert_eq!(expand_aliases_for_parse(input, &env), input);
    }

    #[test]
    fn alias_expansion_does_not_expand_words_inside_alias_opened_quote() {
        let mut env = AliasEnv::default();
        env.aliases.insert("nullalias".into(), String::new());
        env.aliases.insert("foo".into(), "echo 'whoops: ".into());
        assert_eq!(
            expand_aliases_for_parse("foo nullalias'\n", &env),
            "echo 'whoops:  nullalias'\n"
        );
    }

    #[test]
    fn alias_expansion_in_quoted_command_substitution_tracks_case() {
        let mut env = AliasEnv {
            posix: true,
            ..Default::default()
        };
        env.aliases.insert("switch".into(), "case".into());
        assert_eq!(
            expand_aliases_for_parse(r#"echo "$( switch foo in foo) echo ok;; esac )""#, &env),
            r#"echo "$( case foo in foo) echo ok;; esac )""#
        );
    }

    #[test]
    fn alias_expansion_in_command_substitution_can_close_with_alias_text() {
        let mut env = AliasEnv {
            posix: true,
            ..Default::default()
        };
        env.aliases.insert("short".into(), "echo ok )".into());
        assert_eq!(
            expand_aliases_for_parse(r#"echo "$( short ""#, &env),
            r#"echo "$( echo ok ) ""#
        );
    }

    #[test]
    fn alias_expansion_separator_alias_resets_command_position() {
        let mut env = AliasEnv::default();
        env.aliases.insert("a1".into(), "echo ".into());
        env.aliases.insert("a2".into(), "a1".into());
        env.aliases.insert("foo".into(), "bar ".into());
        env.aliases.insert("c".into(), ";".into());
        env.aliases.insert("e".into(), "echo".into());

        assert_eq!(
            expand_aliases_for_parse("a2 foo c e x\n", &env),
            "echo  bar  ; echo x\n"
        );
    }

    #[test]
    fn alias_expansion_posix_keeps_reserved_words() {
        let mut env = AliasEnv {
            posix: true,
            ..Default::default()
        };
        env.aliases.insert("al".into(), " ".into());
        env.aliases.insert("for".into(), "echo".into());

        assert_eq!(
            expand_aliases_for_parse("al for foo in v\n", &env),
            "  for foo in v\n"
        );
    }
}
