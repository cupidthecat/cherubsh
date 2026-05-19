use std::cell::Cell;
use std::rc::Rc;

use cherubsh_common::{
    AliasTable, Span, CMD_SUBST_HEREDOC_WARN_MARKER, W_ARRAYREF, W_ASSIGNMENT, W_COMPASSIGN,
    W_HASDOLLAR, W_QUOTED,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    End,
    Newline,
    Word,
    AssignmentWord,
    RedirWord,
    Number,
    ArithCmd,
    ArithForExprs,
    CondCmd,
    If,
    Then,
    Else,
    Elif,
    Fi,
    Case,
    Esac,
    For,
    Select,
    While,
    Until,
    Do,
    Done,
    Function,
    Coproc,
    In,
    Bang,
    Time,
    TimeOpt,
    TimeIgn,
    AndAnd,
    OrOr,
    Pipe,
    BarAnd,
    Semicolon,
    Ampersand,
    Less,
    Greater,
    LessLess,
    LessLessMinus,
    LessLessLess,
    GreaterGreater,
    GreaterBar,
    LessAnd,
    GreaterAnd,
    AndGreater,
    AndGreaterGreater,
    LessGreater,
    LParen,
    RParen,
    LBrace,
    RBrace,
    DblLParen,
    DblRParen,
    DblLBracket,
    DblRBracket,
    DblSemicolon,
    SemiAmp,
    DblSemiAmp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenValue {
    None,
    Text(String),
    Number { value: i64, raw: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub value: TokenValue,
    pub span: Span,
    pub word_flags: u32,
}

pub struct Lexer<'a> {
    input: &'a str,
    offset: usize,
    source: u32,
    done: bool,
    last_kind: Option<TokenKind>,
    last_reserved: bool,
    parser_state: Option<Rc<Cell<u32>>>,
    aliases: Option<&'a dyn AliasTable>,
    peek_word_after_time: bool,
    extglob_patterns: bool,
    posix_mode: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self::with_source(input, 0)
    }

    pub fn with_source(input: &'a str, source: u32) -> Self {
        Self {
            input,
            offset: 0,
            source,
            done: false,
            last_kind: None,
            last_reserved: false,
            parser_state: None,
            aliases: None,
            peek_word_after_time: false,
            extglob_patterns: false,
            posix_mode: false,
        }
    }

    pub fn with_aliases(input: &'a str, source: u32, aliases: &'a dyn AliasTable) -> Self {
        let mut lexer = Self::with_source(input, source);
        lexer.aliases = Some(aliases);
        lexer
    }

    pub fn set_parser_state(&mut self, state: Rc<Cell<u32>>) {
        self.parser_state = Some(state);
    }

    pub fn set_extglob_patterns(&mut self, enabled: bool) {
        self.extglob_patterns = enabled;
    }

    pub fn set_posix_mode(&mut self, enabled: bool) {
        self.posix_mode = enabled;
    }

    fn pst(&self) -> u32 {
        self.parser_state.as_ref().map(|s| s.get()).unwrap_or(0)
    }

    pub fn next_token(&mut self) -> Option<Token> {
        if self.done {
            return None;
        }

        loop {
            self.skip_whitespace_and_comments();
            if self.offset >= self.input.len() {
                self.done = true;
                return Some(self.emit_simple(TokenKind::End, self.offset, self.offset));
            }
            let ch = self.peek_byte();
            if ch == b'\\'
                && self.offset + 1 < self.input.len()
                && self.input.as_bytes()[self.offset + 1] == b'\n'
            {
                self.offset += 2;
                continue;
            }
            break;
        }

        let ch = self.peek_byte();
        if ch == b'\n' {
            let start = self.offset;
            self.offset += 1;
            return Some(self.emit_simple(TokenKind::Newline, start, self.offset));
        }

        if self.starts_with_proc_subst() {
            return Some(self.lex_word_or_number());
        }

        if self.starts_with_redir_word() {
            return Some(self.lex_word_or_number());
        }

        if let Some(token) = self.lex_operator() {
            return Some(token);
        }

        Some(self.lex_word_or_number())
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            let mut saw_blank = false;
            while self.offset < self.input.len() {
                let ch = self.peek_byte();
                if ch == b' ' || ch == b'\t' || ch == b'\r' {
                    saw_blank = true;
                    self.offset += 1;
                    continue;
                }
                break;
            }
            if self.offset < self.input.len()
                && self.peek_byte() == b'#'
                && (saw_blank || self.at_word_start())
            {
                while self.offset < self.input.len() && self.peek_byte() != b'\n' {
                    self.offset += 1;
                }
                continue;
            }
            break;
        }
    }

    fn at_word_start(&self) -> bool {
        if self.last_reserved {
            return true;
        }
        match self.last_kind {
            None
            | Some(TokenKind::Newline)
            | Some(TokenKind::Semicolon)
            | Some(TokenKind::Ampersand)
            | Some(TokenKind::AndAnd)
            | Some(TokenKind::OrOr)
            | Some(TokenKind::Pipe)
            | Some(TokenKind::BarAnd)
            | Some(TokenKind::LParen)
            | Some(TokenKind::RParen)
            | Some(TokenKind::LBrace)
            | Some(TokenKind::RBrace)
            | Some(TokenKind::DblSemicolon)
            | Some(TokenKind::SemiAmp)
            | Some(TokenKind::DblSemiAmp)
            | Some(TokenKind::DblLParen)
            | Some(TokenKind::DblRParen)
            | Some(TokenKind::DblRBracket)
            | Some(TokenKind::Then)
            | Some(TokenKind::Else)
            | Some(TokenKind::Elif)
            | Some(TokenKind::Do)
            | Some(TokenKind::Done)
            | Some(TokenKind::Fi)
            | Some(TokenKind::Esac)
            | Some(TokenKind::Bang)
            | Some(TokenKind::Time)
            | Some(TokenKind::TimeOpt)
            | Some(TokenKind::TimeIgn) => true,
            _ => false,
        }
    }

    fn peek_byte(&self) -> u8 {
        self.input.as_bytes()[self.offset]
    }

    fn take_char(&mut self) -> char {
        let ch = self.input[self.offset..]
            .chars()
            .next()
            .expect("offset within input");
        self.offset += ch.len_utf8();
        ch
    }

    fn peek_byte_at(&self, off: usize) -> Option<u8> {
        self.input.as_bytes().get(off).copied()
    }

    fn emit_simple(&mut self, kind: TokenKind, start: usize, end: usize) -> Token {
        self.last_kind = Some(kind.clone());
        self.last_reserved = false;
        Token {
            kind,
            value: TokenValue::None,
            span: Span::new(self.source, start, end),
            word_flags: 0,
        }
    }

    fn lex_operator(&mut self) -> Option<Token> {
        let start = self.offset;
        let rest = &self.input[self.offset..];
        let (kind, len) = if rest.starts_with("((") {
            (TokenKind::DblLParen, 2)
        } else if rest.starts_with("))") {
            (TokenKind::DblRParen, 2)
        } else if rest.starts_with("[[") && self.at_word_start() {
            (TokenKind::DblLBracket, 2)
        } else if rest.starts_with("]]") {
            (TokenKind::DblRBracket, 2)
        } else if rest.starts_with(";;&") {
            (TokenKind::DblSemiAmp, 3)
        } else if rest.starts_with(";;") {
            (TokenKind::DblSemicolon, 2)
        } else if rest.starts_with(";&") {
            (TokenKind::SemiAmp, 2)
        } else if rest.starts_with("&&") {
            (TokenKind::AndAnd, 2)
        } else if rest.starts_with("||") {
            (TokenKind::OrOr, 2)
        } else if rest.starts_with("|&") {
            (TokenKind::BarAnd, 2)
        } else if rest.starts_with("<<-") {
            (TokenKind::LessLessMinus, 3)
        } else if rest.starts_with("<<<") {
            (TokenKind::LessLessLess, 3)
        } else if rest.starts_with("<<") {
            (TokenKind::LessLess, 2)
        } else if rest.starts_with(">>") {
            (TokenKind::GreaterGreater, 2)
        } else if rest.starts_with(">|") {
            (TokenKind::GreaterBar, 2)
        } else if rest.starts_with("<&") {
            (TokenKind::LessAnd, 2)
        } else if rest.starts_with(">&") {
            (TokenKind::GreaterAnd, 2)
        } else if rest.starts_with("&>>") {
            (TokenKind::AndGreaterGreater, 3)
        } else if rest.starts_with("&>") {
            (TokenKind::AndGreater, 2)
        } else if rest.starts_with("<>") {
            (TokenKind::LessGreater, 2)
        } else if rest.starts_with("(") {
            (TokenKind::LParen, 1)
        } else if rest.starts_with(")") {
            (TokenKind::RParen, 1)
        } else if rest.starts_with("{") && self.at_word_start() {
            (TokenKind::LBrace, 1)
        } else if rest.starts_with("}") && self.brace_can_close() {
            (TokenKind::RBrace, 1)
        } else if rest.starts_with("|") {
            (TokenKind::Pipe, 1)
        } else if rest.starts_with(";") {
            (TokenKind::Semicolon, 1)
        } else if rest.starts_with("&") {
            (TokenKind::Ampersand, 1)
        } else if rest.starts_with("<") {
            (TokenKind::Less, 1)
        } else if rest.starts_with(">") {
            (TokenKind::Greater, 1)
        } else {
            return None;
        };

        self.offset += len;
        Some(self.emit_simple(kind, start, self.offset))
    }

    fn brace_can_close(&self) -> bool {
        matches!(
            self.last_kind,
            None | Some(TokenKind::Newline)
                | Some(TokenKind::Semicolon)
                | Some(TokenKind::Ampersand)
                | Some(TokenKind::RParen)
                | Some(TokenKind::DblRParen)
        )
    }

    fn lex_word_or_number(&mut self) -> Token {
        let start = self.offset;
        let mut text = String::new();
        let mut flags: u32 = 0;
        let mut saw_eq = false;
        let mut eq_idx: Option<usize> = None;
        let mut name_prefix_ok = false;
        let mut name_prefix_len: usize = 0;

        while self.offset < self.input.len() {
            let ch = self.peek_byte();
            if self.starts_with_extglob_pattern() {
                text.push(ch as char);
                self.offset += 1;
                text.push('(');
                self.offset += 1;
                self.scan_balanced_pair(&mut text, b'(', b')');
                if self.offset < self.input.len() && self.peek_byte() == b')' {
                    text.push(')');
                    self.offset += 1;
                }
                continue;
            }
            if (ch == b'<' || ch == b'>') && self.starts_with_proc_subst() {
                flags |= W_HASDOLLAR;
                text.push(ch as char);
                self.offset += 1;
                text.push('(');
                self.offset += 1;
                self.scan_balanced_pair(&mut text, b'(', b')');
                if self.offset < self.input.len() && self.peek_byte() == b')' {
                    text.push(')');
                    self.offset += 1;
                }
                continue;
            }
            if is_word_metacharacter(ch) {
                break;
            }
            match ch {
                b'\\' => {
                    if self.offset + 1 < self.input.len()
                        && self.input.as_bytes()[self.offset + 1] == b'\n'
                    {
                        self.offset += 2;
                        continue;
                    }
                    flags |= W_QUOTED;
                    text.push('\\');
                    self.offset += 1;
                    if self.offset < self.input.len() {
                        text.push(self.take_char());
                    }
                }
                b'\'' => {
                    flags |= W_QUOTED;
                    text.push('\'');
                    self.offset += 1;
                    while self.offset < self.input.len() && self.peek_byte() != b'\'' {
                        text.push(self.take_char());
                    }
                    if self.offset < self.input.len() {
                        text.push('\'');
                        self.offset += 1;
                    }
                }
                b'"' => {
                    flags |= W_QUOTED;
                    text.push('"');
                    self.offset += 1;
                    self.scan_double_quoted(&mut text, &mut flags);
                    if self.offset < self.input.len() && self.peek_byte() == b'"' {
                        text.push('"');
                        self.offset += 1;
                    }
                }
                b'$' => {
                    flags |= W_HASDOLLAR;
                    text.push('$');
                    self.offset += 1;
                    self.scan_dollar(&mut text, &mut flags, false);
                }
                b'`' => {
                    flags |= W_HASDOLLAR;
                    text.push('`');
                    self.offset += 1;
                    self.scan_balanced_pair(&mut text, b'`', b'`');
                    if self.offset < self.input.len() && self.peek_byte() == b'`' {
                        text.push('`');
                        self.offset += 1;
                    }
                }
                b'+' if self.offset + 1 < self.input.len()
                    && self.input.as_bytes()[self.offset + 1] == b'='
                    && !saw_eq
                    && (is_valid_name(&text) || (name_prefix_ok && (flags & W_ARRAYREF) != 0)) =>
                {
                    saw_eq = true;
                    eq_idx = Some(text.len() + 1);
                    text.push('+');
                    text.push('=');
                    self.offset += 2;
                    if self.offset < self.input.len() && self.peek_byte() == b'(' {
                        text.push('(');
                        self.offset += 1;
                        self.scan_balanced_pair(&mut text, b'(', b')');
                        if self.offset < self.input.len() && self.peek_byte() == b')' {
                            text.push(')');
                            self.offset += 1;
                        }
                        flags |= W_COMPASSIGN | W_ASSIGNMENT;
                    }
                }
                b'=' => {
                    let assign_ok = !saw_eq
                        && (is_valid_name(&text) || (name_prefix_ok && (flags & W_ARRAYREF) != 0));
                    if assign_ok {
                        saw_eq = true;
                        eq_idx = Some(text.len());
                        text.push('=');
                        self.offset += 1;
                        if self.offset < self.input.len()
                            && self.peek_byte() == b'+'
                            && eq_idx.is_some()
                        {
                            // += compound assignment
                        }
                        if self.offset < self.input.len() && self.peek_byte() == b'(' {
                            text.push('(');
                            self.offset += 1;
                            self.scan_balanced_pair(&mut text, b'(', b')');
                            if self.offset < self.input.len() && self.peek_byte() == b')' {
                                text.push(')');
                                self.offset += 1;
                            }
                            flags |= W_COMPASSIGN | W_ASSIGNMENT;
                        }
                    } else {
                        text.push('=');
                        self.offset += 1;
                    }
                }
                b'[' if !saw_eq && is_valid_name(&text) => {
                    name_prefix_ok = true;
                    name_prefix_len = text.len();
                    let subscript_start = text.len();
                    text.push('[');
                    self.offset += 1;
                    self.scan_balanced_pair(&mut text, b'[', b']');
                    if self.offset < self.input.len() && self.peek_byte() == b']' {
                        text.push(']');
                        self.offset += 1;
                    }
                    let subscript_text = &text[subscript_start..];
                    if subscript_text.contains(['\'', '"', '\\']) {
                        flags |= W_QUOTED;
                    }
                    flags |= W_ARRAYREF;
                }
                _ => {
                    text.push(self.take_char());
                }
            }
        }

        let span = Span::new(self.source, start, self.offset);

        if text.is_empty() {
            self.offset += 1;
            let tok = Token {
                kind: TokenKind::Word,
                value: TokenValue::Text(String::new()),
                span,
                word_flags: flags,
            };
            self.last_kind = Some(TokenKind::Word);
            return tok;
        }

        if let Some(eq) = eq_idx {
            let prefix_ok = if name_prefix_ok && name_prefix_len > 0 {
                is_valid_name(&text[..name_prefix_len])
            } else {
                eq > 0 && is_valid_name(&text[..eq])
            };
            if prefix_ok {
                flags |= W_ASSIGNMENT;
                let tok = Token {
                    kind: TokenKind::AssignmentWord,
                    value: TokenValue::Text(text),
                    span,
                    word_flags: flags,
                };
                self.last_kind = Some(TokenKind::AssignmentWord);
                return tok;
            }
        }

        if flags == 0
            && is_all_digits(&text)
            && is_number_boundary(self.input.as_bytes(), self.offset)
        {
            if let Ok(value) = text.parse::<i32>() {
                let tok = Token {
                    kind: TokenKind::Number,
                    value: TokenValue::Number {
                        value: i64::from(value),
                        raw: text,
                    },
                    span,
                    word_flags: 0,
                };
                self.last_kind = Some(TokenKind::Number);
                return tok;
            }
        }

        if flags == 0 && text == "!" && self.at_word_start() {
            let tok = self.emit_simple(TokenKind::Bang, start, self.offset);
            return tok;
        }

        if flags == 0 && text == "time" && self.at_word_start() {
            self.peek_word_after_time = true;
            let tok = self.emit_simple(TokenKind::Time, start, self.offset);
            return tok;
        }

        if flags == 0 && self.peek_word_after_time {
            self.peek_word_after_time = false;
            if text == "-p" {
                let tok = self.emit_simple(TokenKind::TimeOpt, start, self.offset);
                return tok;
            }
            if text == "--" {
                let tok = self.emit_simple(TokenKind::TimeIgn, start, self.offset);
                return tok;
            }
        } else {
            self.peek_word_after_time = false;
        }

        if flags == 0 && self.at_word_start() {
            if let Some(aliases) = self.aliases {
                if aliases.expansion_enabled() {
                    let _ = aliases.lookup(&text);
                }
            }
            if is_reserved_word_text(&text) {
                self.last_reserved = true;
                self.last_kind = Some(TokenKind::Word);
                return Token {
                    kind: TokenKind::Word,
                    value: TokenValue::Text(text),
                    span,
                    word_flags: 0,
                };
            }
        }

        let tok = Token {
            kind: TokenKind::Word,
            value: TokenValue::Text(text),
            span,
            word_flags: flags,
        };
        self.last_kind = Some(TokenKind::Word);
        self.last_reserved = false;
        tok
    }

    fn scan_double_quoted(&mut self, out: &mut String, flags: &mut u32) {
        while self.offset < self.input.len() {
            let ch = self.peek_byte();
            match ch {
                b'"' => return,
                b'\\' => {
                    out.push('\\');
                    self.offset += 1;
                    if self.offset < self.input.len() {
                        out.push(self.take_char());
                    }
                }
                b'$' => {
                    *flags |= W_HASDOLLAR;
                    out.push('$');
                    self.offset += 1;
                    self.scan_dollar(out, flags, true);
                }
                b'`' => {
                    *flags |= W_HASDOLLAR;
                    out.push('`');
                    self.offset += 1;
                    self.scan_balanced_pair(out, b'`', b'`');
                    if self.offset < self.input.len() && self.peek_byte() == b'`' {
                        out.push('`');
                        self.offset += 1;
                    }
                }
                _ => {
                    out.push(self.take_char());
                }
            }
        }
    }

    fn scan_dollar(&mut self, out: &mut String, flags: &mut u32, in_double_quotes: bool) {
        if self.offset >= self.input.len() {
            return;
        }
        let ch = self.peek_byte();
        match ch {
            b'(' => {
                out.push('(');
                self.offset += 1;
                if self.offset < self.input.len() && self.peek_byte() == b'(' {
                    match arith_dparen_scan(self.input.as_bytes(), self.offset + 1) {
                        DParenScan::Arithmetic(end) => {
                            self.push_range(out, self.offset, end);
                            self.offset = end;
                        }
                        DParenScan::CommandSubstitution => {
                            self.scan_command_substitution(out);
                            if self.offset < self.input.len() && self.peek_byte() == b')' {
                                out.push(')');
                                self.offset += 1;
                            }
                        }
                        DParenScan::Missing => {
                            out.push('(');
                            self.offset += 1;
                            self.scan_dparen(out);
                            if self.offset < self.input.len() && self.peek_byte() == b')' {
                                out.push(')');
                                self.offset += 1;
                            }
                            if self.offset < self.input.len() && self.peek_byte() == b')' {
                                out.push(')');
                                self.offset += 1;
                            }
                        }
                    }
                } else {
                    self.scan_command_substitution(out);
                    if self.offset < self.input.len() && self.peek_byte() == b')' {
                        out.push(')');
                        self.offset += 1;
                    }
                }
            }
            b'{' => {
                out.push('{');
                self.offset += 1;
                self.scan_parameter_brace(out, flags, in_double_quotes);
                if self.offset < self.input.len() && self.peek_byte() == b'}' {
                    out.push('}');
                    self.offset += 1;
                }
            }
            b'[' => {
                out.push('[');
                self.offset += 1;
                self.scan_legacy_arith(out);
                if self.offset < self.input.len() && self.peek_byte() == b']' {
                    out.push(']');
                    self.offset += 1;
                }
            }
            b'\'' => {
                *flags |= W_QUOTED;
                out.push('\'');
                self.offset += 1;
                while self.offset < self.input.len() && self.peek_byte() != b'\'' {
                    if self.peek_byte() == b'\\' && self.offset + 1 < self.input.len() {
                        out.push('\\');
                        self.offset += 1;
                    }
                    out.push(self.take_char());
                }
                if self.offset < self.input.len() {
                    out.push('\'');
                    self.offset += 1;
                }
            }
            b'"' if !in_double_quotes => {
                *flags |= W_QUOTED;
                out.push('"');
                self.offset += 1;
                self.scan_double_quoted(out, flags);
                if self.offset < self.input.len() && self.peek_byte() == b'"' {
                    out.push('"');
                    self.offset += 1;
                }
            }
            _ => {}
        }
    }

    fn scan_legacy_arith(&mut self, out: &mut String) {
        let mut depth = 1usize;
        while self.offset < self.input.len() && depth > 0 {
            let ch = self.peek_byte();
            match ch {
                b'[' => {
                    depth += 1;
                    out.push('[');
                    self.offset += 1;
                }
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                    out.push(']');
                    self.offset += 1;
                }
                b'\\' => {
                    out.push('\\');
                    self.offset += 1;
                    if self.offset < self.input.len() {
                        out.push(self.take_char());
                    }
                }
                b'\'' | b'"' | b'`' => {
                    let q = ch;
                    out.push(q as char);
                    self.offset += 1;
                    while self.offset < self.input.len() && self.peek_byte() != q {
                        if self.peek_byte() == b'\\' && self.offset + 1 < self.input.len() {
                            out.push('\\');
                            self.offset += 1;
                        }
                        out.push(self.take_char());
                    }
                    if self.offset < self.input.len() {
                        out.push(q as char);
                        self.offset += 1;
                    }
                }
                _ => {
                    out.push(self.take_char());
                }
            }
        }
    }

    fn push_range(&self, out: &mut String, start: usize, end: usize) {
        for b in &self.input.as_bytes()[start..end] {
            out.push(*b as char);
        }
    }

    fn scan_parameter_brace(&mut self, out: &mut String, flags: &mut u32, in_double_quotes: bool) {
        let mut depth = 1;
        while self.offset < self.input.len() && depth > 0 {
            let ch = self.peek_byte();
            if ch == b'}' {
                depth -= 1;
                if depth == 0 {
                    return;
                }
                out.push('}');
                self.offset += 1;
            } else if ch == b'\\' {
                out.push('\\');
                self.offset += 1;
                if self.offset < self.input.len() {
                    out.push(self.take_char());
                }
            } else if ch == b'$' {
                out.push('$');
                self.offset += 1;
                self.scan_dollar(out, flags, in_double_quotes);
            } else if ch == b'\'' && !(in_double_quotes && self.posix_mode) {
                let mut j = self.offset + 1;
                let mut saw_escaped_quote = false;
                let mut closes_before_quote = false;
                while j < self.input.len() && self.input.as_bytes()[j] != b'\'' {
                    if self.input.as_bytes()[j] == b'\\' && j + 1 < self.input.len() {
                        if self.input.as_bytes()[j + 1] == b'\'' {
                            saw_escaped_quote = true;
                        }
                        j += 2;
                        continue;
                    }
                    if self.input.as_bytes()[j] == b'}' && saw_escaped_quote {
                        closes_before_quote = true;
                        break;
                    }
                    j += 1;
                }
                if closes_before_quote {
                    out.push(self.take_char());
                    continue;
                }
                out.push('\'');
                self.offset += 1;
                while self.offset < self.input.len() && self.peek_byte() != b'\'' {
                    if self.peek_byte() == b'\\' && self.offset + 1 < self.input.len() {
                        out.push('\\');
                        self.offset += 1;
                    }
                    out.push(self.take_char());
                }
                if self.offset < self.input.len() {
                    out.push('\'');
                    self.offset += 1;
                }
            } else if ch == b'"' {
                out.push('"');
                self.offset += 1;
                while self.offset < self.input.len() && self.peek_byte() != b'"' {
                    if self.peek_byte() == b'\\' && self.offset + 1 < self.input.len() {
                        out.push('\\');
                        self.offset += 1;
                    }
                    out.push(self.take_char());
                }
                if self.offset < self.input.len() {
                    out.push('"');
                    self.offset += 1;
                }
            } else if ch == b'`' {
                out.push('`');
                self.offset += 1;
                self.scan_balanced_pair(out, b'`', b'`');
                if self.offset < self.input.len() && self.peek_byte() == b'`' {
                    out.push('`');
                    self.offset += 1;
                }
            } else {
                out.push(self.take_char());
            }
        }
    }

    fn scan_dparen(&mut self, out: &mut String) {
        let mut depth = 1;
        while self.offset < self.input.len() && depth > 0 {
            let ch = self.peek_byte();
            match ch {
                b'(' => {
                    depth += 1;
                    out.push('(');
                    self.offset += 1;
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                    out.push(')');
                    self.offset += 1;
                }
                b'\\' => {
                    out.push('\\');
                    self.offset += 1;
                    if self.offset < self.input.len() {
                        out.push(self.take_char());
                    }
                }
                b'\'' | b'"' | b'`' => {
                    let q = ch;
                    out.push(q as char);
                    self.offset += 1;
                    while self.offset < self.input.len() && self.peek_byte() != q {
                        if self.peek_byte() == b'\\' && self.offset + 1 < self.input.len() {
                            out.push('\\');
                            self.offset += 1;
                        }
                        out.push(self.take_char());
                    }
                    if self.offset < self.input.len() {
                        out.push(q as char);
                        self.offset += 1;
                    }
                }
                _ => {
                    out.push(self.take_char());
                }
            }
        }
    }

    fn scan_balanced_pair(&mut self, out: &mut String, open: u8, close: u8) {
        let mut depth = 1;
        while self.offset < self.input.len() && depth > 0 {
            let ch = self.peek_byte();
            if ch == open && open != close {
                depth += 1;
                out.push(ch as char);
                self.offset += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return;
                }
                out.push(ch as char);
                self.offset += 1;
            } else if ch == b'\\' {
                out.push('\\');
                self.offset += 1;
                if self.offset < self.input.len() {
                    out.push(self.take_char());
                }
            } else if ch == b'$'
                && self.offset + 1 < self.input.len()
                && self.input.as_bytes()[self.offset + 1] == b'('
            {
                out.push('$');
                self.offset += 1;
                self.scan_dollar_in_balanced_pair(out);
            } else if ch == b'$'
                && self.offset + 1 < self.input.len()
                && self.input.as_bytes()[self.offset + 1] == b'{'
            {
                out.push('$');
                self.offset += 1;
                out.push('{');
                self.offset += 1;
                let mut flags = 0;
                self.scan_parameter_brace(out, &mut flags, false);
                if self.offset < self.input.len() {
                    out.push(self.take_char());
                }
            } else if ch == b'\'' {
                out.push('\'');
                self.offset += 1;
                while self.offset < self.input.len() && self.peek_byte() != b'\'' {
                    out.push(self.take_char());
                }
                if self.offset < self.input.len() {
                    out.push('\'');
                    self.offset += 1;
                }
            } else if ch == b'"' {
                out.push('"');
                self.offset += 1;
                self.scan_double_quoted_in_balanced_pair(out);
                if self.offset < self.input.len() && self.peek_byte() == b'"' {
                    out.push('"');
                    self.offset += 1;
                }
            } else {
                out.push(self.take_char());
            }
        }
    }

    fn scan_double_quoted_in_balanced_pair(&mut self, out: &mut String) {
        while self.offset < self.input.len() {
            let ch = self.peek_byte();
            match ch {
                b'"' => return,
                b'\\' => {
                    out.push('\\');
                    self.offset += 1;
                    if self.offset < self.input.len() {
                        out.push(self.take_char());
                    }
                }
                b'$' => {
                    out.push('$');
                    self.offset += 1;
                    if self.offset < self.input.len() && self.peek_byte() == b'{' {
                        out.push('{');
                        self.offset += 1;
                        let mut flags = 0;
                        self.scan_parameter_brace(out, &mut flags, true);
                        if self.offset < self.input.len() && self.peek_byte() == b'}' {
                            out.push('}');
                            self.offset += 1;
                        }
                    } else {
                        self.scan_dollar_in_balanced_pair(out);
                    }
                }
                b'`' => {
                    out.push('`');
                    self.offset += 1;
                    self.scan_balanced_pair(out, b'`', b'`');
                    if self.offset < self.input.len() && self.peek_byte() == b'`' {
                        out.push('`');
                        self.offset += 1;
                    }
                }
                _ => out.push(self.take_char()),
            }
        }
    }

    fn scan_dollar_in_balanced_pair(&mut self, out: &mut String) {
        if self.offset >= self.input.len() {
            return;
        }
        if self.peek_byte() == b'(' {
            out.push('(');
            self.offset += 1;
            self.scan_balanced_pair(out, b'(', b')');
            if self.offset < self.input.len() && self.peek_byte() == b')' {
                out.push(')');
                self.offset += 1;
            }
        }
    }

    fn scan_command_substitution(&mut self, out: &mut String) {
        let mut depth = 1i32;
        let mut case_depth = 0usize;
        let mut comment_ok = true;
        let mut heredocs: std::collections::VecDeque<(String, bool)> =
            std::collections::VecDeque::new();
        let mut at_line_start = false;

        while self.offset < self.input.len() && depth > 0 {
            if at_line_start {
                if let Some((delimiter, strip_tabs)) = heredocs.front().cloned() {
                    let line_start = self.offset;
                    while self.offset < self.input.len() && self.peek_byte() != b'\n' {
                        self.offset += 1;
                    }
                    let line_end = self.offset;
                    let mut candidate_start = line_start;
                    let mut candidate = &self.input.as_bytes()[candidate_start..line_end];
                    if strip_tabs {
                        while candidate.first() == Some(&b'\t') {
                            candidate_start += 1;
                            candidate = &candidate[1..];
                        }
                    }
                    if depth == 1 {
                        if let Some(close_offset) =
                            heredoc_delimiter_closing_paren(candidate, delimiter.as_bytes())
                        {
                            out.push_str(&self.input[line_start..candidate_start]);
                            out.push_str(&delimiter);
                            heredocs.pop_front();
                            self.offset = candidate_start + close_offset + 1;
                            out.push(')');
                            return;
                        }
                    }
                    out.push_str(&self.input[line_start..line_end]);
                    if candidate == delimiter.as_bytes() {
                        heredocs.pop_front();
                    }
                    if self.offset < self.input.len() && self.peek_byte() == b'\n' {
                        out.push(self.take_char());
                    }
                    comment_ok = true;
                    at_line_start = true;
                    continue;
                }
            }

            let ch = self.peek_byte();
            if ch == b'\n' {
                out.push(self.take_char());
                comment_ok = true;
                at_line_start = true;
                continue;
            }
            at_line_start = false;

            if ch == b'#' && comment_ok {
                while self.offset < self.input.len() && self.peek_byte() != b'\n' {
                    out.push(self.take_char());
                }
                continue;
            }

            if ch == b'\\' {
                out.push('\\');
                self.offset += 1;
                if self.offset < self.input.len() {
                    out.push(self.take_char());
                }
                comment_ok = false;
                continue;
            }

            if ch == b'\'' || ch == b'"' || ch == b'`' {
                self.scan_quoted_bytes(out, ch);
                comment_ok = false;
                continue;
            }

            if ch == b'$'
                && self.offset + 1 < self.input.len()
                && self.input.as_bytes().get(self.offset + 1) == Some(&b'\'')
            {
                out.push('$');
                self.offset += 1;
                self.scan_quoted_bytes(out, b'\'');
                comment_ok = false;
                continue;
            }

            if ch == b'<'
                && self.offset + 1 < self.input.len()
                && self.input.as_bytes().get(self.offset + 1) == Some(&b'<')
            {
                self.scan_heredoc_redirect(out, &mut heredocs);
                comment_ok = false;
                continue;
            }

            if is_name_start(ch) {
                let start = self.offset;
                out.push(self.take_char());
                while self.offset < self.input.len() && is_name_byte(self.peek_byte()) {
                    out.push(self.take_char());
                }
                let word = &self.input[start..self.offset];
                let previous = previous_significant_byte(self.input.as_bytes(), start);
                match word {
                    "case" => case_depth = case_depth.saturating_add(1),
                    "esac" if !matches!(previous, Some(b'|' | b'(')) => {
                        case_depth = case_depth.saturating_sub(1);
                    }
                    _ => {}
                }
                comment_ok = false;
                continue;
            }

            if ch == b'(' {
                depth += 1;
                out.push('(');
                self.offset += 1;
                comment_ok = true;
                continue;
            }
            if ch == b')' {
                if case_depth > 0 && depth == 1 {
                    out.push(')');
                    self.offset += 1;
                    comment_ok = true;
                    continue;
                }
                if depth == 1 && !heredocs.is_empty() {
                    self.offset += 1;
                    self.scan_command_substitution_trailing_heredocs(out, &mut heredocs);
                    out.push(')');
                    return;
                }
                depth -= 1;
                if depth == 0 {
                    return;
                }
                out.push(')');
                self.offset += 1;
                comment_ok = true;
                continue;
            }

            out.push(self.take_char());
            comment_ok = matches!(
                ch,
                b' ' | b'\t' | b'\r' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>'
            );
        }
    }

    fn scan_command_substitution_trailing_heredocs(
        &mut self,
        out: &mut String,
        heredocs: &mut std::collections::VecDeque<(String, bool)>,
    ) {
        if self.offset < self.input.len() && self.peek_byte() == b'\n' {
            out.push(self.take_char());
        }
        out.push_str(CMD_SUBST_HEREDOC_WARN_MARKER);
        while self.offset < self.input.len() && !heredocs.is_empty() {
            let Some((delimiter, strip_tabs)) = heredocs.front().cloned() else {
                break;
            };
            let line_start = self.offset;
            while self.offset < self.input.len() && self.peek_byte() != b'\n' {
                self.offset += 1;
            }
            let line_end = self.offset;
            out.push_str(&self.input[line_start..line_end]);

            let mut candidate = &self.input.as_bytes()[line_start..line_end];
            if strip_tabs {
                while candidate.first() == Some(&b'\t') {
                    candidate = &candidate[1..];
                }
            }
            if candidate == delimiter.as_bytes() {
                heredocs.pop_front();
            }
            if self.offset < self.input.len() && self.peek_byte() == b'\n' {
                out.push(self.take_char());
            }
        }
    }

    fn scan_quoted_bytes(&mut self, out: &mut String, quote: u8) {
        if quote == b'"' {
            out.push('"');
            self.offset += 1;
            let mut flags = 0;
            self.scan_double_quoted(out, &mut flags);
            if self.offset < self.input.len() && self.peek_byte() == b'"' {
                out.push('"');
                self.offset += 1;
            }
            return;
        }
        out.push(quote as char);
        self.offset += 1;
        while self.offset < self.input.len() && self.peek_byte() != quote {
            if self.peek_byte() == b'\\' && self.offset + 1 < self.input.len() {
                out.push('\\');
                self.offset += 1;
            }
            out.push(self.take_char());
        }
        if self.offset < self.input.len() {
            out.push(quote as char);
            self.offset += 1;
        }
    }

    fn scan_heredoc_redirect(
        &mut self,
        out: &mut String,
        heredocs: &mut std::collections::VecDeque<(String, bool)>,
    ) {
        out.push('<');
        self.offset += 1;
        out.push('<');
        self.offset += 1;
        let strip_tabs = if self.offset < self.input.len() && self.peek_byte() == b'-' {
            out.push('-');
            self.offset += 1;
            true
        } else {
            false
        };
        while self.offset < self.input.len() && matches!(self.peek_byte(), b' ' | b'\t') {
            out.push(self.take_char());
        }
        let mut delimiter = String::new();
        let mut quote: Option<u8> = None;
        while self.offset < self.input.len() {
            let ch = self.peek_byte();
            if let Some(q) = quote {
                out.push(self.take_char());
                if ch == q {
                    quote = None;
                } else {
                    delimiter.push(ch as char);
                }
                continue;
            }
            if ch == b'\'' || ch == b'"' {
                quote = Some(ch);
                out.push(self.take_char());
                continue;
            }
            if ch == b'\\' {
                self.offset += 1;
                if self.offset < self.input.len() {
                    if self.peek_byte() == b'\n' {
                        self.offset += 1;
                        continue;
                    }
                    out.push('\\');
                    delimiter.push(self.peek_byte() as char);
                    out.push(self.take_char());
                }
                continue;
            }
            if ch.is_ascii_whitespace() || is_word_metacharacter(ch) {
                break;
            }
            delimiter.push(ch as char);
            out.push(self.take_char());
        }
        if strip_tabs {
            delimiter = delimiter.trim_start_matches('\t').to_string();
        }
        if !delimiter.is_empty() {
            heredocs.push_back((delimiter, strip_tabs));
        }
    }

    fn at_proc_subst(&self) -> bool {
        self.starts_with_proc_subst()
    }

    fn starts_with_proc_subst(&self) -> bool {
        if self.offset + 1 >= self.input.len() {
            return false;
        }
        let bytes = self.input.as_bytes();
        let ch = bytes[self.offset];
        (ch == b'<' || ch == b'>') && bytes[self.offset + 1] == b'('
    }

    fn starts_with_extglob_pattern(&self) -> bool {
        if !self.extglob_patterns || self.offset + 1 >= self.input.len() {
            return false;
        }
        let bytes = self.input.as_bytes();
        matches!(bytes[self.offset], b'@' | b'+' | b'*' | b'?' | b'!')
            && bytes[self.offset + 1] == b'('
    }

    fn starts_with_redir_word(&self) -> bool {
        let bytes = self.input.as_bytes();
        if bytes.get(self.offset) != Some(&b'{') {
            return false;
        }
        let mut cursor = self.offset + 1;
        while cursor < bytes.len()
            && (bytes[cursor] == b'_' || bytes[cursor].is_ascii_alphanumeric())
        {
            cursor += 1;
        }
        if cursor == self.offset + 1 || bytes.get(cursor) != Some(&b'}') {
            return false;
        }
        let name = &self.input[self.offset + 1..cursor];
        if !is_valid_name(name) {
            return false;
        }
        matches!(bytes.get(cursor + 1), Some(b'<') | Some(b'>'))
    }
}

fn heredoc_delimiter_closing_paren(candidate: &[u8], delimiter: &[u8]) -> Option<usize> {
    if !candidate.starts_with(delimiter) {
        return None;
    }
    let mut offset = delimiter.len();
    while offset < candidate.len() && matches!(candidate[offset], b' ' | b'\t') {
        offset += 1;
    }
    if candidate.get(offset) == Some(&b')') {
        Some(offset)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DParenScan {
    Arithmetic(usize),
    CommandSubstitution,
    Missing,
}

fn arith_dparen_scan(bytes: &[u8], start: usize) -> DParenScan {
    let mut i = start;
    let mut depth = 1i32;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                if depth == 1 {
                    if i + 1 < bytes.len() && bytes[i + 1] == b')' {
                        return DParenScan::Arithmetic(i + 2);
                    }
                    let mut j = i + 1;
                    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\r') {
                        j += 1;
                    }
                    if j >= bytes.len() || matches!(bytes[j], b')' | b'\n' | b';' | b'&' | b'|') {
                        return DParenScan::CommandSubstitution;
                    }
                    return DParenScan::Missing;
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    DParenScan::Missing
}

fn is_name_start(ch: u8) -> bool {
    ch == b'_' || ch.is_ascii_alphabetic()
}

fn is_name_byte(ch: u8) -> bool {
    is_name_start(ch) || ch.is_ascii_digit()
}

fn previous_significant_byte(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i > 0 {
        i -= 1;
        if !bytes[i].is_ascii_whitespace() {
            return Some(bytes[i]);
        }
    }
    None
}

/// Mirror of `PST_CONDCMD` from the parser so the lexer can dispatch `]]`
/// closing brackets without depending on the parser crate.
pub const PST_CONDCMD_FLAG: u32 = 1 << 7;

fn is_word_metacharacter(ch: u8) -> bool {
    matches!(
        ch,
        b' ' | b'\t' | b'\r' | b'\n' | b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')'
    )
}

fn is_number_boundary(bytes: &[u8], offset: usize) -> bool {
    if offset >= bytes.len() {
        return true;
    }
    matches!(
        bytes[offset],
        b' ' | b'\t' | b'\r' | b'\n' | b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'{' | b'}'
    )
}

fn is_all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|ch| ch.is_ascii_digit())
}

fn is_reserved_word_text(text: &str) -> bool {
    matches!(
        text,
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
            | "function"
            | "coproc"
            | "in"
    )
}

fn is_valid_name(text: &str) -> bool {
    let mut chars = text.chars();
    let first = match chars.next() {
        Some(first) => first,
        None => return false,
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<Token> {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            let stop = matches!(token.kind, TokenKind::End);
            tokens.push(token);
            if stop {
                break;
            }
        }
        tokens
    }

    fn lex_with_extglob(input: &str) -> Vec<Token> {
        let mut lexer = Lexer::new(input);
        lexer.set_extglob_patterns(true);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            let stop = matches!(token.kind, TokenKind::End);
            tokens.push(token);
            if stop {
                break;
            }
        }
        tokens
    }

    fn kinds(input: &str) -> Vec<TokenKind> {
        lex(input).into_iter().map(|t| t.kind).collect()
    }

    fn words(input: &str) -> Vec<String> {
        lex(input)
            .into_iter()
            .filter_map(|t| match t.value {
                TokenValue::Text(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn double_quoted_word_is_one_token() {
        let ws = words(r#"echo "a b""#);
        assert_eq!(ws, vec!["echo".to_string(), "\"a b\"".to_string()]);
    }

    #[test]
    fn dollar_before_closing_double_quote_is_literal() {
        let tokens = lex(r#"grep -v "^$" </dev/null | wc -l"#);
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Word,
                TokenKind::Word,
                TokenKind::Word,
                TokenKind::Less,
                TokenKind::Word,
                TokenKind::Pipe,
                TokenKind::Word,
                TokenKind::Word,
                TokenKind::End,
            ]
        );
        assert_eq!(words(r#"grep -v "^$" </dev/null | wc -l"#)[2], r#""^$""#);
    }

    #[test]
    fn command_substitution_word_is_one_token() {
        let ws = words("echo $(date +%s)");
        assert_eq!(ws, vec!["echo".to_string(), "$(date +%s)".to_string()]);
    }

    #[test]
    fn command_substitution_case_pattern_reserved_words_are_one_token() {
        let ws = words(": $(case x in in|esac) foo;; esac)");
        assert_eq!(
            ws,
            vec![
                ":".to_string(),
                "$(case x in in|esac) foo;; esac)".to_string()
            ]
        );

        let ws = words(": $(case k in else|done|time|esac) echo ok;; esac)");
        assert_eq!(
            ws,
            vec![
                ":".to_string(),
                "$(case k in else|done|time|esac) echo ok;; esac)".to_string()
            ]
        );
    }

    #[test]
    fn single_quoted_word_is_one_token() {
        let ws = words("echo 'a;b'");
        assert_eq!(ws, vec!["echo".to_string(), "'a;b'".to_string()]);
    }

    #[test]
    fn compound_assignment_is_one_token() {
        let ws = words("arr=(a b c)");
        assert_eq!(ws, vec!["arr=(a b c)".to_string()]);
    }

    #[test]
    fn array_element_assignment_is_one_token() {
        let ks = kinds("a[0]=hi");
        assert!(matches!(ks[0], TokenKind::AssignmentWord));
    }

    #[test]
    fn array_element_assignment_skips_command_subst_brackets() {
        let ws = words("myarray[$(echo ])]=def");
        assert_eq!(ws, vec!["myarray[$(echo ])]=def".to_string()]);
        let ks = kinds("myarray[$(echo ])]=def");
        assert!(matches!(ks[0], TokenKind::AssignmentWord));
    }

    #[test]
    fn array_element_assignment_single_quoted_backslash_closes() {
        let ws = words(r"dict['\']=4");
        assert_eq!(ws, vec![r"dict['\']=4".to_string()]);
        let ks = kinds(r"dict['\']=4");
        assert!(matches!(ks[0], TokenKind::AssignmentWord));
    }

    #[test]
    fn nested_double_quote_in_cmdsubst() {
        let ws = words(r#"echo "$(echo "x")""#);
        assert_eq!(
            ws,
            vec!["echo".to_string(), "\"$(echo \"x\")\"".to_string()]
        );
    }

    #[test]
    fn command_substitution_heredoc_delimiter_can_touch_closing_paren() {
        let ws = words("text=$(cat <<EOF\nhere is the text\nEOF)\necho after\n");
        assert_eq!(
            ws,
            vec![
                "text=$(cat <<EOF\nhere is the text\nEOF)".to_string(),
                "echo".to_string(),
                "after".to_string(),
            ]
        );
    }

    #[test]
    fn command_substitution_heredoc_delimiter_removes_escaped_newline() {
        let ws = words("text=$(cat <<\\EOT\\\n4\nd \\\ng\nEOT4\n)\necho after\n");
        assert_eq!(
            ws,
            vec![
                "text=$(cat <<\\EOT4\nd \\\ng\nEOT4\n)".to_string(),
                "echo".to_string(),
                "after".to_string(),
            ]
        );
    }

    #[test]
    fn command_substitution_double_quotes_track_nested_substitutions() {
        let ws = words(r#"echo $(echo "foo$(echo ")")") after"#);
        assert_eq!(
            ws,
            vec![
                "echo".to_string(),
                r#"$(echo "foo$(echo ")")")"#.to_string(),
                "after".to_string(),
            ]
        );
    }

    #[test]
    fn process_substitution_word() {
        let ws = words("cat <(echo x)");
        assert_eq!(ws, vec!["cat".to_string(), "<(echo x)".to_string()]);
    }

    #[test]
    fn extglob_word_when_enabled() {
        let ws = lex_with_extglob("echo !([*)*")
            .into_iter()
            .filter_map(|t| match t.value {
                TokenValue::Text(s) => Some(s),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ws, vec!["echo".to_string(), "!([*)*".to_string()]);
    }

    #[test]
    fn time_keyword_at_start() {
        let ks = kinds("time -p sleep 1");
        assert_eq!(ks[0], TokenKind::Time);
        assert_eq!(ks[1], TokenKind::TimeOpt);
    }

    #[test]
    fn semi_amp_terminator() {
        let ks = kinds(";&");
        assert_eq!(ks[0], TokenKind::SemiAmp);
    }

    #[test]
    fn dbl_semi_amp_terminator() {
        let ks = kinds(";;&");
        assert_eq!(ks[0], TokenKind::DblSemiAmp);
    }

    #[test]
    fn bang_at_command_position() {
        let ks = kinds("! true");
        assert_eq!(ks[0], TokenKind::Bang);
    }

    #[test]
    fn comment_consumed_to_newline() {
        let ks = kinds("echo a # rest\nb");
        assert!(ks.contains(&TokenKind::Newline));
    }

    #[test]
    fn hash_inside_word_is_literal() {
        let ws = words("echo abc#def");
        assert_eq!(ws, vec!["echo".to_string(), "abc#def".to_string()]);
    }

    #[test]
    fn right_brace_after_word_is_literal() {
        let ws = words("echo { }");
        assert_eq!(
            ws,
            vec!["echo".to_string(), "{".to_string(), "}".to_string()]
        );
    }

    #[test]
    fn arithmetic_substitution_word() {
        let ws = words("echo $((1+2))");
        assert_eq!(ws, vec!["echo".to_string(), "$((1+2))".to_string()]);
    }

    #[test]
    fn legacy_arithmetic_substitution_word() {
        let ws = words("echo $[ 13 * 2 ] after");
        assert_eq!(
            ws,
            vec![
                "echo".to_string(),
                "$[ 13 * 2 ]".to_string(),
                "after".to_string(),
            ]
        );

        let ws = words("echo $[ a[i] + 1 ]");
        assert_eq!(ws, vec!["echo".to_string(), "$[ a[i] + 1 ]".to_string()]);
    }

    #[test]
    fn parameter_brace_expansion_word() {
        let ws = words("echo ${a:-b}");
        assert_eq!(ws, vec!["echo".to_string(), "${a:-b}".to_string()]);
    }

    #[test]
    fn parameter_brace_backticks_protect_closing_brace() {
        let ws = words(r#"echo "${HOME:`echo }`}" after"#);
        assert_eq!(
            ws,
            vec![
                "echo".to_string(),
                r#""${HOME:`echo }`}""#.to_string(),
                "after".to_string(),
            ]
        );
    }

    #[test]
    fn backtick_command_substitution() {
        let ws = words("echo `date`");
        assert_eq!(ws, vec!["echo".to_string(), "`date`".to_string()]);
    }

    #[test]
    fn ansi_c_quoting() {
        let ws = words(r#"echo $'\n\t'"#);
        assert_eq!(ws, vec!["echo".to_string(), "$'\\n\\t'".to_string()]);
    }
}
