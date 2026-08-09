use cherubsh_common::{
    Span, CMD_COPROC_SUBSHELL, CMD_INVERT_RETURN, CMD_TIME_PIPELINE, CMD_TIME_POSIX,
    CMD_WANT_SUBSHELL, W_COMPASSIGN,
};
use cherubsh_lexer::Lexer;
use cherubsh_lexer::Token;
use cherubsh_lexer::TokenKind;
use cherubsh_lexer::TokenValue;
use std::sync::Arc;

mod pretty;

pub use pretty::pretty_print;

pub const CONN_AND_AND: u32 = 1;
pub const CONN_OR_OR: u32 = 2;
pub const CONN_PIPE: u32 = 3;
pub const CONN_BAR_AND: u32 = 4;
pub const CONN_SEMI: u32 = 5;
pub const CONN_AMP: u32 = 6;
pub const CONN_NEWLINE: u32 = 7;

pub const PST_CASEPAT: u32 = 1 << 0;
pub const PST_SUBSHELL: u32 = 1 << 1;
pub const PST_CMDSUBST: u32 = 1 << 2;
pub const PST_HEREDOC: u32 = 1 << 3;
pub const PST_REDIRLIST: u32 = 1 << 4;
pub const PST_ALEXPNEXT: u32 = 1 << 5;
pub const PST_ALLOWOPNBRC: u32 = 1 << 6;
pub const PST_CONDCMD: u32 = 1 << 7;
pub const PST_CONDEXPR: u32 = 1 << 8;
pub const PST_EOFTOKEN: u32 = 1 << 9;
pub const PST_DBLPAREN: u32 = 1 << 10;
pub const PST_ARITHFOR: u32 = 1 << 11;
pub const PST_EXTPAT: u32 = 1 << 12;
pub const PST_COMPASSIGN: u32 = 1 << 13;
pub const PST_ASSIGNOK: u32 = 1 << 14;
pub const PST_REGEXP: u32 = 1 << 15;
pub const PST_REPARSE: u32 = 1 << 16;
pub const PST_COMMENT: u32 = 1 << 17;
pub const PST_ENDALIAS: u32 = 1 << 18;
pub const PST_NOEXPAND: u32 = 1 << 19;
pub const PST_NOERROR: u32 = 1 << 20;
pub const PST_STRING: u32 = 1 << 21;

const HEREDOC_MAX: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordDesc {
    pub text: String,
    pub flags: u32,
    pub span: Span,
    pub raw: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedirectInstruction {
    OutputDirection,
    InputDirection,
    InputaDirection,
    AppendingTo,
    ReadingUntil,
    ReadingString,
    DuplicatingInput,
    DuplicatingOutput,
    DeblankReadingUntil,
    CloseThis,
    ErrAndOut,
    InputOutput,
    OutputForce,
    DuplicatingInputWord,
    DuplicatingOutputWord,
    MoveInput,
    MoveOutput,
    MoveInputWord,
    MoveOutputWord,
    AppendErrAndOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redirectee {
    Fd(i32),
    Word(WordDesc),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redirector {
    Fd(i32),
    Var(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    pub redirector: Redirector,
    pub rflags: u32,
    pub flags: u32,
    pub instruction: RedirectInstruction,
    pub redirectee: Redirectee,
    pub here_doc_eof: Option<String>,
    pub here_doc_body: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CondType {
    And,
    Or,
    Unary,
    Binary,
    Term,
    Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CondCommand {
    pub cond_type: CondType,
    pub op: Option<WordDesc>,
    pub left: Option<Box<CondCommand>>,
    pub right: Option<Box<CondCommand>>,
    pub term: Option<WordDesc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimpleCommand {
    pub words: Vec<WordDesc>,
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForCommand {
    pub name: WordDesc,
    pub map_list: Option<Vec<WordDesc>>,
    pub action: Box<Command>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternList {
    pub patterns: Vec<WordDesc>,
    pub action: Option<Box<Command>>,
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseCommand {
    pub word: WordDesc,
    pub clauses: Vec<PatternList>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfCommand {
    pub test: Box<Command>,
    pub true_case: Box<Command>,
    pub false_case: Option<Box<Command>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhileCommand {
    pub test: Box<Command>,
    pub action: Box<Command>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UntilCommand {
    pub test: Box<Command>,
    pub action: Box<Command>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub first: Box<Command>,
    pub second: Box<Command>,
    pub connector: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: WordDesc,
    pub command: Arc<Command>,
    pub source_file: Option<String>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupCommand {
    pub command: Box<Command>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectCommand {
    pub name: WordDesc,
    pub map_list: Option<Vec<WordDesc>>,
    pub action: Box<Command>,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArithCommand {
    pub expression: WordDesc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArithForCommand {
    pub init: Option<WordDesc>,
    pub test: Option<WordDesc>,
    pub step: Option<WordDesc>,
    pub action: Box<Command>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubshellCommand {
    pub command: Box<Command>,
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoprocCommand {
    pub name: Option<WordDesc>,
    pub command: Box<Command>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandData {
    For(ForCommand),
    Case(CaseCommand),
    While(WhileCommand),
    Until(UntilCommand),
    If(IfCommand),
    Connection(Connection),
    Simple(SimpleCommand),
    FunctionDef(FunctionDef),
    Group(GroupCommand),
    Select(SelectCommand),
    Arith(ArithCommand),
    Cond(CondCommand),
    ArithFor(ArithForCommand),
    Subshell(SubshellCommand),
    Coproc(CoprocCommand),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub data: CommandData,
    pub flags: u32,
    pub line: u32,
    pub redirects: Vec<Redirect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ast {
    pub root: Command,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Option<Span>,
}

pub struct Parser {
    tokens: Vec<Token>,
    index: usize,
    state: u32,
    input: String,
    line_starts: Vec<usize>,
    total_lines: u32,
    here_doc_count: usize,
    here_doc_body_offset: Option<usize>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>, input: &str) -> Self {
        let line_starts = line_starts(input);
        let total_lines = line_starts.len() as u32;
        Self {
            tokens,
            index: 0,
            state: 0,
            input: input.to_string(),
            line_starts,
            total_lines,
            here_doc_count: 0,
            here_doc_body_offset: None,
        }
    }

    fn line_number_for_offset(&self, offset: usize) -> u32 {
        line_number_for_offset_cached(&self.line_starts, self.input.len(), offset)
    }

    pub fn parse(&mut self) -> Result<Ast, ParseError> {
        match self.parse_input_unit()? {
            Some(ast) => Ok(ast),
            None => Err(ParseError {
                message: "empty input".to_string(),
                span: self.peek_span().cloned(),
            }),
        }
    }

    pub fn parse_input_unit(&mut self) -> Result<Option<Ast>, ParseError> {
        self.skip_newlines();
        if self.peek_kind() == Some(TokenKind::End) {
            return Ok(None);
        }
        let root = self.parse_simple_list()?;
        self.skip_newlines();
        if self.peek_kind() != Some(TokenKind::End) {
            return Err(ParseError {
                message: format!(
                    "unexpected token '{}'",
                    self.peek()
                        .map(token_to_text_lossy)
                        .unwrap_or_else(|| "EOF".to_string())
                ),
                span: self.peek_span().cloned(),
            });
        }
        Ok(Some(Ast { root }))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn peek_n(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.index + n)
    }

    fn peek_kind(&self) -> Option<TokenKind> {
        self.peek().map(|token| token.kind.clone())
    }

    fn peek_span(&self) -> Option<&Span> {
        self.peek().map(|token| &token.span)
    }

    fn raw_for_span(&self, span: Span) -> Option<String> {
        if span.end <= self.input.len() && span.start <= span.end {
            Some(self.input[span.start..span.end].to_string())
        } else {
            None
        }
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        if token.is_some() {
            self.index += 1;
        }
        token
    }

    fn skip_newlines(&mut self) {
        while self.peek_kind() == Some(TokenKind::Newline) {
            self.bump();
        }
    }

    fn parse_simple_list(&mut self) -> Result<Command, ParseError> {
        self.parse_simple_list_until(&[])
    }

    fn parse_simple_list_until(&mut self, stop_words: &[&str]) -> Result<Command, ParseError> {
        self.skip_newlines();
        if self.is_stop_word(stop_words) {
            return Err(ParseError {
                message: "expected list".to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.reject_unexpected_reserved_command(stop_words)?;
        let mut left = self.parse_and_or()?;
        loop {
            if self.is_stop_word(stop_words) {
                break;
            }
            match self.peek_kind() {
                Some(TokenKind::Semicolon) => {
                    self.bump();
                    self.skip_newlines();
                    if self.is_stop_word(stop_words) {
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        break;
                    }
                    self.reject_unexpected_reserved_command(stop_words)?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_SEMI);
                }
                Some(TokenKind::Ampersand) => {
                    self.bump();
                    self.skip_newlines();
                    if self.is_stop_word(stop_words) {
                        left = self.connect(left, self.null_command(), CONN_AMP);
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        left = self.connect(left, self.null_command(), CONN_AMP);
                        break;
                    }
                    self.reject_unexpected_reserved_command(stop_words)?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_AMP);
                }
                Some(TokenKind::Newline) => {
                    self.skip_newlines();
                    if self.is_stop_word(stop_words) {
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        break;
                    }
                    self.reject_unexpected_reserved_command(stop_words)?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_NEWLINE);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_simple_list_until_kind(
        &mut self,
        stop_kinds: &[TokenKind],
    ) -> Result<Command, ParseError> {
        self.skip_newlines();
        if self.is_stop_kind(stop_kinds) {
            return Err(ParseError {
                message: "expected list".to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.reject_unexpected_reserved_command(&[])?;
        let mut left = self.parse_and_or()?;
        loop {
            if self.is_stop_kind(stop_kinds) {
                break;
            }
            match self.peek_kind() {
                Some(TokenKind::Semicolon) => {
                    self.bump();
                    self.skip_newlines();
                    if self.is_stop_kind(stop_kinds) {
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        break;
                    }
                    self.reject_unexpected_reserved_command(&[])?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_SEMI);
                }
                Some(TokenKind::Ampersand) => {
                    self.bump();
                    self.skip_newlines();
                    if self.is_stop_kind(stop_kinds) {
                        left = self.connect(left, self.null_command(), CONN_AMP);
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        left = self.connect(left, self.null_command(), CONN_AMP);
                        break;
                    }
                    self.reject_unexpected_reserved_command(&[])?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_AMP);
                }
                Some(TokenKind::Newline) => {
                    self.skip_newlines();
                    if self.is_stop_kind(stop_kinds) {
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        break;
                    }
                    self.reject_unexpected_reserved_command(&[])?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_NEWLINE);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_and_or(&mut self) -> Result<Command, ParseError> {
        let mut left = self.parse_pipeline()?;
        loop {
            match self.peek_kind() {
                Some(TokenKind::AndAnd) => {
                    self.bump();
                    self.skip_newlines();
                    let right = self.parse_pipeline()?;
                    left = self.connect(left, right, CONN_AND_AND);
                }
                Some(TokenKind::OrOr) => {
                    self.bump();
                    self.skip_newlines();
                    let right = self.parse_pipeline()?;
                    left = self.connect(left, right, CONN_OR_OR);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_pipeline(&mut self) -> Result<Command, ParseError> {
        let mut extra_flags: u32 = 0;
        let mut invert = false;

        loop {
            match self.peek_kind() {
                Some(TokenKind::Time) => {
                    self.bump();
                    extra_flags |= CMD_TIME_PIPELINE;
                    match self.peek_kind() {
                        Some(TokenKind::TimeOpt) => {
                            self.bump();
                            extra_flags |= CMD_TIME_POSIX;
                            if self.peek_kind() == Some(TokenKind::TimeIgn) || self.is_word("--") {
                                self.bump();
                            }
                        }
                        Some(TokenKind::TimeIgn) => {
                            self.bump();
                        }
                        _ => {}
                    }
                }
                Some(TokenKind::Bang) => {
                    self.bump();
                    invert = !invert;
                }
                _ => break,
            }
        }

        if self.at_list_terminator() {
            let mut command = self.null_command();
            if invert {
                command.flags |= CMD_INVERT_RETURN;
            }
            if extra_flags != 0 {
                command.flags |= extra_flags;
            }
            return Ok(command);
        }

        let mut left = self.parse_command()?;
        loop {
            match self.peek_kind() {
                Some(TokenKind::Pipe) => {
                    self.bump();
                    self.skip_newlines();
                    let right = self.parse_command()?;
                    left = self.connect(left, right, CONN_PIPE);
                }
                Some(TokenKind::BarAnd) => {
                    self.bump();
                    self.skip_newlines();
                    let right = self.parse_command()?;
                    add_stderr_to_stdout_redirect(&mut left);
                    left = self.connect(left, right, CONN_PIPE);
                }
                _ => break,
            }
        }
        if invert {
            left.flags |= CMD_INVERT_RETURN;
        }
        if extra_flags != 0 {
            left.flags |= extra_flags;
        }
        Ok(left)
    }

    fn at_list_terminator(&self) -> bool {
        matches!(
            self.peek_kind(),
            Some(TokenKind::End)
                | Some(TokenKind::Newline)
                | Some(TokenKind::Semicolon)
                | Some(TokenKind::Ampersand)
                | Some(TokenKind::Then)
                | Some(TokenKind::Else)
                | Some(TokenKind::Elif)
                | Some(TokenKind::Fi)
                | Some(TokenKind::Do)
                | Some(TokenKind::Done)
                | Some(TokenKind::Esac)
                | Some(TokenKind::RBrace)
                | Some(TokenKind::RParen)
        ) || self.is_word("then")
            || self.is_word("else")
            || self.is_word("elif")
            || self.is_word("fi")
            || self.is_word("do")
            || self.is_word("done")
            || self.is_word("esac")
    }

    fn null_command(&self) -> Command {
        Command {
            data: CommandData::Simple(SimpleCommand {
                words: Vec::new(),
                redirects: Vec::new(),
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        }
    }

    fn parse_command(&mut self) -> Result<Command, ParseError> {
        let command_line = self
            .peek_span()
            .map(|span| self.line_number_for_offset(span.start));
        let saved_here_doc_count = self.here_doc_count;
        let saved_here_doc_body_offset = self.here_doc_body_offset;
        self.here_doc_count = 0;
        self.here_doc_body_offset = None;
        let mut command = if self.is_word("if") {
            self.parse_if_command()?
        } else if self.is_word("while") {
            self.parse_while_command()?
        } else if self.is_word("until") {
            self.parse_until_command()?
        } else if self.is_word("for") {
            self.parse_for_command()?
        } else if self.is_word("case") {
            self.parse_case_command()?
        } else if self.is_word("select") {
            self.parse_select_command()?
        } else if self.is_word("function") || self.is_function_def_start() {
            self.parse_function_def()?
        } else if self.is_word("coproc") {
            self.parse_coproc_command()?
        } else if self.peek_kind() == Some(TokenKind::LBrace) {
            self.parse_group_command()?
        } else if self.peek_kind() == Some(TokenKind::LParen) {
            self.parse_subshell_command()?
        } else if self.peek_kind() == Some(TokenKind::DblLParen) {
            if self.dbl_lparen_starts_arith_command() {
                self.parse_arith_command()?
            } else {
                self.parse_double_subshell_command()?
            }
        } else if self.peek_kind() == Some(TokenKind::DblLBracket) {
            self.parse_cond_command()?
        } else if let Some(word) = self.unexpected_reserved_word() {
            return Err(ParseError {
                message: format!("unexpected token '{}'", word),
                span: self.peek_span().cloned(),
            });
        } else {
            self.parse_simple_command()?
        };
        self.parse_trailing_redirects(&mut command)?;
        if command.line == 0 {
            command.line = command_line.unwrap_or(0);
        }
        self.here_doc_count = saved_here_doc_count;
        self.here_doc_body_offset = saved_here_doc_body_offset;
        Ok(command)
    }

    fn parse_if_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("if")?;
        let test = self.parse_simple_list_until(&["then"])?;
        self.expect_word("then")?;
        let true_case = self.parse_simple_list_until(&["elif", "else", "fi"])?;
        let mut false_case = None;

        if self.is_word("elif") {
            let elif_command = self.parse_elif_chain()?;
            false_case = Some(Box::new(elif_command));
        } else if self.is_word("else") {
            self.expect_word("else")?;
            let else_case = self.parse_simple_list_until(&["fi"])?;
            false_case = Some(Box::new(else_case));
        }

        self.expect_word("fi")?;
        Ok(Command {
            data: CommandData::If(IfCommand {
                test: Box::new(test),
                true_case: Box::new(true_case),
                false_case,
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_elif_chain(&mut self) -> Result<Command, ParseError> {
        self.expect_word("elif")?;
        let test = self.parse_simple_list_until(&["then"])?;
        self.expect_word("then")?;
        let true_case = self.parse_simple_list_until(&["elif", "else", "fi"])?;
        let mut false_case = None;

        if self.is_word("elif") {
            let elif_command = self.parse_elif_chain()?;
            false_case = Some(Box::new(elif_command));
        } else if self.is_word("else") {
            self.expect_word("else")?;
            let else_case = self.parse_simple_list_until(&["fi"])?;
            false_case = Some(Box::new(else_case));
        }

        Ok(Command {
            data: CommandData::If(IfCommand {
                test: Box::new(test),
                true_case: Box::new(true_case),
                false_case,
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_while_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("while")?;
        let test = self.parse_simple_list_until(&["do"])?;
        self.expect_word("do")?;
        let action = self.parse_simple_list_until(&["done"])?;
        self.expect_word("done")?;
        Ok(Command {
            data: CommandData::While(WhileCommand {
                test: Box::new(test),
                action: Box::new(action),
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_until_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("until")?;
        let test = self.parse_simple_list_until(&["do"])?;
        self.expect_word("do")?;
        let action = self.parse_simple_list_until(&["done"])?;
        self.expect_word("done")?;
        Ok(Command {
            data: CommandData::Until(UntilCommand {
                test: Box::new(test),
                action: Box::new(action),
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_for_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("for")?;
        if self.peek_kind() == Some(TokenKind::DblLParen) {
            return self.parse_arith_for_command();
        }
        let name_token = match self.peek_kind() {
            Some(TokenKind::Word) | Some(TokenKind::AssignmentWord) | Some(TokenKind::Number) => {
                self.bump()
            }
            _ => None,
        }
        .ok_or(ParseError {
            message: "expected for name".to_string(),
            span: self.peek_span().cloned(),
        })?;
        let name_text = match name_token.value {
            TokenValue::Text(text) => text,
            TokenValue::Number { raw, .. } => raw,
            TokenValue::None => String::new(),
        };
        let name = WordDesc {
            text: name_text,
            flags: 0,
            span: name_token.span,
            raw: self.raw_for_span(name_token.span),
        };
        let line = self.line_number_for_offset(name.span.start);

        self.skip_newlines();
        let map_list = if self.is_word("in") {
            self.expect_word("in")?;
            self.parse_word_list_until_command_body()
        } else {
            None
        };

        self.skip_newlines();
        if self.peek_kind() == Some(TokenKind::Semicolon) {
            self.bump();
            self.skip_newlines();
        }
        let action = if self.peek_kind() == Some(TokenKind::LBrace) {
            self.parse_brace_body()?
        } else {
            self.expect_word("do")?;
            let action = self.parse_simple_list_until(&["done"])?;
            self.expect_word("done")?;
            action
        };

        Ok(Command {
            data: CommandData::For(ForCommand {
                name,
                map_list,
                action: Box::new(action),
                line,
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_select_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("select")?;
        let name = self.parse_word_desc()?;
        let line = self.line_number_for_offset(name.span.start);
        let map_list = if self.is_word("in") {
            self.expect_word("in")?;
            self.parse_word_list_until_command_body()
        } else {
            None
        };
        self.skip_newlines();
        if self.peek_kind() == Some(TokenKind::Semicolon) {
            self.bump();
            self.skip_newlines();
        }
        let action = if self.peek_kind() == Some(TokenKind::LBrace) {
            self.parse_brace_body()?
        } else {
            self.expect_word("do")?;
            let action = self.parse_simple_list_until(&["done"])?;
            self.expect_word("done")?;
            action
        };

        Ok(Command {
            data: CommandData::Select(SelectCommand {
                name,
                map_list,
                action: Box::new(action),
                line,
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_case_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("case")?;
        let word = self.parse_word_desc()?;
        self.skip_newlines();
        self.expect_word("in")?;
        self.skip_newlines();

        let mut clauses: Vec<PatternList> = Vec::new();

        while !self.is_word("esac") {
            self.skip_newlines();
            if self.is_word("esac") {
                break;
            }
            if self.peek_kind() == Some(TokenKind::LParen) {
                self.bump();
            }

            let (patterns, consumed_close) = self.parse_case_patterns()?;
            if !consumed_close {
                self.expect_kind(TokenKind::RParen, "expected ')' after case pattern")?;
            }

            self.skip_newlines();
            let stop_kinds = [
                TokenKind::DblSemicolon,
                TokenKind::SemiAmp,
                TokenKind::DblSemiAmp,
            ];
            let action = if self.is_stop_kind(&stop_kinds) || self.is_word("esac") {
                None
            } else {
                Some(Box::new(self.parse_simple_list_until_words_or_kinds(
                    &["esac"],
                    &stop_kinds,
                )?))
            };

            let flags = match self.peek_kind() {
                Some(TokenKind::DblSemicolon) => {
                    self.bump();
                    0
                }
                Some(TokenKind::SemiAmp) => {
                    self.bump();
                    cherubsh_common::CASEPAT_FALLTHROUGH
                }
                Some(TokenKind::DblSemiAmp) => {
                    self.bump();
                    cherubsh_common::CASEPAT_TESTNEXT
                }
                _ => 0,
            };

            clauses.push(PatternList {
                patterns,
                action,
                flags,
            });
        }

        self.expect_word("esac")?;
        Ok(Command {
            data: CommandData::Case(CaseCommand {
                word,
                clauses,
                line: 0,
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_case_patterns(&mut self) -> Result<(Vec<WordDesc>, bool), ParseError> {
        let mut patterns: Vec<WordDesc> = Vec::new();
        let mut consumed_close = false;
        loop {
            match self.peek_kind() {
                Some(TokenKind::Pipe) => {
                    self.bump();
                }
                Some(TokenKind::RParen) => break,
                Some(TokenKind::Newline) => self.skip_newlines(),
                Some(TokenKind::End) | None => break,
                Some(kind) if !is_case_pattern_separator(&kind) => {
                    let (word, closed) = self.collect_pattern_word(PatternCollectContext::Case)?;
                    patterns.push(word);
                    if closed {
                        consumed_close = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        Ok((patterns, consumed_close))
    }

    fn parse_group_command(&mut self) -> Result<Command, ParseError> {
        self.expect_kind(TokenKind::LBrace, "expected '{'")?;
        let body = self.parse_simple_list_until_kind(&[TokenKind::RBrace])?;
        if self.peek_kind() == Some(TokenKind::End) {
            return Err(self.unexpected_eof_error());
        }
        if !self.previous_token_terminated_list() {
            return Err(ParseError {
                message: "expected ';' or newline before '}'".to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.expect_kind(TokenKind::RBrace, "expected '}'")?;
        Ok(Command {
            data: CommandData::Group(GroupCommand {
                command: Box::new(body),
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_subshell_command(&mut self) -> Result<Command, ParseError> {
        self.expect_kind(TokenKind::LParen, "expected '('")?;
        let body = self.parse_simple_list_until_kind(&[TokenKind::RParen])?;
        self.expect_kind(TokenKind::RParen, "expected ')'")?;
        Ok(Command {
            data: CommandData::Subshell(SubshellCommand {
                command: Box::new(body),
                flags: 0,
            }),
            flags: CMD_WANT_SUBSHELL,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_double_subshell_command(&mut self) -> Result<Command, ParseError> {
        self.expect_kind(TokenKind::DblLParen, "expected '(('")?;

        let inner = self.parse_simple_list_until_kind(&[TokenKind::RParen])?;
        self.expect_kind(TokenKind::RParen, "expected ')'")?;

        let mut body = Command {
            data: CommandData::Subshell(SubshellCommand {
                command: Box::new(inner),
                flags: CMD_WANT_SUBSHELL,
            }),
            flags: CMD_WANT_SUBSHELL,
            line: 0,
            redirects: Vec::new(),
        };

        loop {
            match self.peek_kind() {
                Some(TokenKind::Semicolon) => {
                    self.bump();
                    self.skip_newlines();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break;
                    }
                    let right = self.parse_and_or()?;
                    body = self.connect(body, right, CONN_SEMI);
                }
                Some(TokenKind::Ampersand) => {
                    self.bump();
                    self.skip_newlines();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        body = self.connect(body, self.null_command(), CONN_AMP);
                        break;
                    }
                    let right = self.parse_and_or()?;
                    body = self.connect(body, right, CONN_AMP);
                }
                Some(TokenKind::Newline) => {
                    self.skip_newlines();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        break;
                    }
                    let right = self.parse_and_or()?;
                    body = self.connect(body, right, CONN_NEWLINE);
                }
                _ => break,
            }
        }

        self.expect_kind(TokenKind::RParen, "expected ')'")?;
        Ok(Command {
            data: CommandData::Subshell(SubshellCommand {
                command: Box::new(body),
                flags: CMD_WANT_SUBSHELL,
            }),
            flags: CMD_WANT_SUBSHELL,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_function_def(&mut self) -> Result<Command, ParseError> {
        let saw_function_kw = self.is_word("function");
        let name = if saw_function_kw {
            self.expect_word("function")?;
            self.parse_word_desc()?
        } else {
            let token = self.bump().ok_or(ParseError {
                message: "expected function name".to_string(),
                span: self.peek_span().cloned(),
            })?;
            let text = match token.value {
                TokenValue::Text(text) => text,
                TokenValue::Number { raw, .. } => raw,
                TokenValue::None => String::new(),
            };
            WordDesc {
                text,
                flags: 0,
                span: token.span,
                raw: self.raw_for_span(token.span),
            }
        };

        if self.peek_kind() == Some(TokenKind::LParen) {
            self.bump();
            self.expect_kind(TokenKind::RParen, "expected ')' in function")?;
        } else if !saw_function_kw {
            return Err(ParseError {
                message: "expected '()' after function name".to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.skip_newlines();

        let first_body_line = self
            .compound_body_first_command_line()
            .unwrap_or_else(|| self.line_number_for_offset(name.span.start));
        let command = self.parse_compound_body()?;
        // Body must be a compound command per bash; reject simple commands.
        if let CommandData::Simple(_) = command.data {
            return Err(ParseError {
                message: "function body must be a compound command".to_string(),
                span: None,
            });
        }
        Ok(Command {
            data: CommandData::FunctionDef(FunctionDef {
                name,
                command: Arc::new(command),
                source_file: None,
                line: self
                    .total_lines
                    .saturating_sub(first_body_line)
                    .saturating_add(1),
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_coproc_command(&mut self) -> Result<Command, ParseError> {
        self.expect_word("coproc")?;
        let (name, command) = if self.peek_is_compound_opener() {
            (None, self.parse_compound_body()?)
        } else if self.peek_kind() == Some(TokenKind::Word) && self.peek_n_is_compound_opener(1) {
            (Some(self.parse_word_desc()?), self.parse_compound_body()?)
        } else {
            (None, self.parse_simple_command()?)
        };
        Ok(Command {
            data: CommandData::Coproc(CoprocCommand {
                name,
                command: Box::new(command),
            }),
            flags: CMD_WANT_SUBSHELL | CMD_COPROC_SUBSHELL,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn peek_n_is_compound_opener(&self, n: usize) -> bool {
        match self.peek_n(n) {
            Some(token) => {
                matches!(
                    token.kind,
                    TokenKind::LBrace
                        | TokenKind::LParen
                        | TokenKind::DblLParen
                        | TokenKind::DblLBracket
                ) || matches!(
                    &token.value,
                    TokenValue::Text(text) if text == "{" || text == "(" || text == "if" || text == "while" || text == "until" || text == "for" || text == "case" || text == "select"
                )
            }
            None => false,
        }
    }

    fn peek_is_compound_opener(&self) -> bool {
        self.peek_n_is_compound_opener(0)
    }

    fn compound_body_first_command_line(&self) -> Option<u32> {
        let mut idx = self.index;
        match self.tokens.get(idx) {
            Some(token) if token.kind == TokenKind::LBrace => idx += 1,
            Some(token) if token.kind == TokenKind::Word => match &token.value {
                TokenValue::Text(text) if text == "{" => idx += 1,
                _ => {}
            },
            _ => {}
        }
        while matches!(
            self.tokens.get(idx).map(|token| token.kind.clone()),
            Some(TokenKind::Newline)
        ) {
            idx += 1;
        }
        self.tokens
            .get(idx)
            .map(|token| self.line_number_for_offset(token.span.start))
    }

    fn parse_compound_body(&mut self) -> Result<Command, ParseError> {
        // After `function NAME` or `coproc NAME`, the body starts with a compound
        // command. The lexer may have emitted Word("{") rather than LBrace because
        // a Word token preceded it; promote it here.
        if let Some(token) = self.peek() {
            if matches!(token.kind, TokenKind::Word) {
                if let TokenValue::Text(text) = &token.value {
                    if text == "{" {
                        self.bump();
                        let body = self
                            .parse_simple_list_until_words_or_kinds(&["}"], &[TokenKind::RBrace])?;
                        if self.peek_kind() == Some(TokenKind::End) {
                            return Err(self.unexpected_eof_error());
                        }
                        // accept either `}` token or Word("}")
                        if self.peek_kind() == Some(TokenKind::RBrace) {
                            self.bump();
                        } else if let Some(t) = self.peek() {
                            if matches!(t.kind, TokenKind::Word) {
                                if let TokenValue::Text(t2) = &t.value {
                                    if t2 == "}" {
                                        self.bump();
                                    }
                                }
                            }
                        }
                        return Ok(Command {
                            data: CommandData::Group(GroupCommand {
                                command: Box::new(body),
                            }),
                            flags: 0,
                            line: 0,
                            redirects: Vec::new(),
                        });
                    }
                }
            }
        }
        self.parse_command()
    }

    fn parse_arith_command(&mut self) -> Result<Command, ParseError> {
        let open_end = self
            .peek()
            .map(|token| token.span.end)
            .unwrap_or(self.input.len());
        self.expect_kind(TokenKind::DblLParen, "expected '(('")?;
        let expr_text = self.collect_arith_command_expr(open_end)?;
        let expr = WordDesc {
            text: expr_text,
            flags: 0,
            span: self.peek_span().cloned().unwrap_or(Span::new(0, 0, 0)),
            raw: None,
        };
        Ok(Command {
            data: CommandData::Arith(ArithCommand { expression: expr }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn dbl_lparen_starts_arith_command(&self) -> bool {
        let mut depth = 0usize;
        let mut idx = self.index + 1;
        while let Some(token) = self.tokens.get(idx) {
            match token.kind {
                TokenKind::DblLParen => depth += 2,
                TokenKind::DblRParen if depth == 0 => return true,
                TokenKind::DblRParen if depth >= 2 => depth -= 2,
                TokenKind::DblRParen => {
                    return self
                        .tokens
                        .get(idx + 1)
                        .is_some_and(|next| next.kind == TokenKind::RParen);
                }
                TokenKind::Semicolon => return false,
                TokenKind::End => return true,
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            idx += 1;
        }
        false
    }

    fn parse_cond_command(&mut self) -> Result<Command, ParseError> {
        self.expect_kind(TokenKind::DblLBracket, "expected '[['")?;
        self.state |= PST_CONDCMD | PST_CONDEXPR;
        self.skip_newlines();
        let tree = self.cond_expr()?;
        self.skip_newlines();
        self.state &= !(PST_CONDCMD | PST_CONDEXPR);
        self.expect_kind(TokenKind::DblRBracket, "expected ']]'")?;
        Ok(Command {
            data: CommandData::Cond(tree),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn cond_expr(&mut self) -> Result<CondCommand, ParseError> {
        self.skip_newlines();
        let mut left = self.cond_and()?;
        self.skip_newlines();
        while self.peek_kind() == Some(TokenKind::OrOr) {
            self.bump();
            self.skip_newlines();
            let right = self.cond_and()?;
            left = CondCommand {
                cond_type: CondType::Or,
                op: None,
                left: Some(Box::new(left)),
                right: Some(Box::new(right)),
                term: None,
            };
            self.skip_newlines();
        }
        Ok(left)
    }

    fn cond_and(&mut self) -> Result<CondCommand, ParseError> {
        self.skip_newlines();
        let mut left = self.cond_term()?;
        self.skip_newlines();
        while self.peek_kind() == Some(TokenKind::AndAnd) {
            self.bump();
            self.skip_newlines();
            let right = self.cond_term()?;
            left = CondCommand {
                cond_type: CondType::And,
                op: None,
                left: Some(Box::new(left)),
                right: Some(Box::new(right)),
                term: None,
            };
            self.skip_newlines();
        }
        Ok(left)
    }

    fn cond_term(&mut self) -> Result<CondCommand, ParseError> {
        self.skip_newlines();
        // ! prefix
        if self.peek_kind() == Some(TokenKind::Bang) || self.is_word("!") {
            self.bump();
            self.skip_newlines();
            let inner = self.cond_term()?;
            return Ok(CondCommand {
                cond_type: CondType::Term,
                op: None,
                left: Some(Box::new(inner)),
                right: None,
                term: None,
            });
        }
        // ( cond_expr )
        if self.peek_kind() == Some(TokenKind::LParen) {
            self.bump();
            self.skip_newlines();
            let inner = self.cond_expr()?;
            self.skip_newlines();
            self.expect_kind(TokenKind::RParen, "expected ')' in [[ ]]")?;
            return Ok(CondCommand {
                cond_type: CondType::Expr,
                op: None,
                left: Some(Box::new(inner)),
                right: None,
                term: None,
            });
        }

        let lhs = self.cond_word()?;
        self.skip_newlines();
        // Unary op `-X lhs`
        if is_unary_test_op(&lhs.text) {
            let rhs = self.cond_word()?;
            if matches!(rhs.text.as_str(), "&" | "<") {
                return Err(ParseError {
                    message: format!(
                        "unexpected argument `{}` to conditional unary operator",
                        rhs.text
                    ),
                    span: Some(rhs.span),
                });
            }
            return Ok(CondCommand {
                cond_type: CondType::Unary,
                op: Some(lhs),
                left: Some(Box::new(cond_leaf(rhs))),
                right: None,
                term: None,
            });
        }

        // If terminator next, synthesize "-n lhs".
        self.skip_newlines();
        if self.cond_at_terminator() {
            if lhs.text == "&" {
                return Err(ParseError {
                    message: "unexpected token `&' in conditional command".to_string(),
                    span: Some(lhs.span),
                });
            }
            let op = WordDesc {
                text: "-n".to_string(),
                flags: 0,
                span: lhs.span,
                raw: Some("-n".to_string()),
            };
            return Ok(CondCommand {
                cond_type: CondType::Unary,
                op: Some(op),
                left: Some(Box::new(cond_leaf(lhs))),
                right: None,
                term: None,
            });
        }

        // Binary: lhs op rhs.
        let op = self.cond_word()?;
        self.skip_newlines();
        if !is_binary_test_op(&op.text) {
            return Err(ParseError {
                message: format!(
                    "unexpected token `{}`, conditional binary operator expected",
                    op.text
                ),
                span: Some(op.span),
            });
        }
        let rhs = if op.text == "=~" {
            self.cond_regex_rhs_word()?
        } else {
            self.cond_rhs_word()?
        };
        if rhs.text == "&" {
            return Err(ParseError {
                message: "unexpected argument `&' to conditional binary operator".to_string(),
                span: Some(rhs.span),
            });
        }
        Ok(CondCommand {
            cond_type: CondType::Binary,
            op: Some(op),
            left: Some(Box::new(cond_leaf(lhs))),
            right: Some(Box::new(cond_leaf(rhs))),
            term: None,
        })
    }

    fn cond_word(&mut self) -> Result<WordDesc, ParseError> {
        // Inside [[ ]], accept any non-terminator token as a word.
        self.skip_newlines();
        let token = self.bump().ok_or(ParseError {
            message: "expected word in [[ ]]".to_string(),
            span: self.peek_span().cloned(),
        })?;
        let flags = token.word_flags;
        let text = match &token.value {
            TokenValue::Text(text) => text.clone(),
            TokenValue::Number { raw, .. } => raw.clone(),
            TokenValue::None => token_to_text_lossy(&token),
        };
        Ok(WordDesc {
            text,
            flags,
            span: token.span,
            raw: self.raw_for_span(token.span),
        })
    }

    fn cond_rhs_word(&mut self) -> Result<WordDesc, ParseError> {
        self.skip_newlines();
        let (word, _) = self.collect_pattern_word(PatternCollectContext::Cond)?;
        Ok(word)
    }

    fn cond_regex_rhs_word(&mut self) -> Result<WordDesc, ParseError> {
        self.skip_newlines();
        let (word, _) = self.collect_pattern_word(PatternCollectContext::CondRegex)?;
        Ok(word)
    }

    fn cond_at_terminator(&self) -> bool {
        matches!(
            self.peek_kind(),
            Some(TokenKind::DblRBracket)
                | Some(TokenKind::AndAnd)
                | Some(TokenKind::OrOr)
                | Some(TokenKind::RParen)
                | None
        )
    }

    fn parse_arith_for_command(&mut self) -> Result<Command, ParseError> {
        let open_span = self.peek_span().cloned().unwrap_or_else(Span::dummy);
        self.expect_kind(TokenKind::DblLParen, "expected '(( '")?;
        let (expr_text, command_span) = self.collect_arith_for_exprs(open_span);
        let (mut parts, separators) = split_arith_for_clauses(&expr_text);
        if separators < 2 {
            return Err(ParseError {
                message: "syntax error: arithmetic expression required".to_string(),
                span: Some(command_span),
            });
        }
        if separators > 2 {
            return Err(ParseError {
                message: "syntax error near unexpected token `;'".to_string(),
                span: Some(command_span),
            });
        }
        while parts.len() < 3 {
            parts.push((String::new(), String::new()));
        }
        let init = if parts[0].0.is_empty() {
            None
        } else {
            Some(WordDesc {
                text: parts[0].0.clone(),
                flags: 0,
                span: Span::new(0, 0, 0),
                raw: Some(parts[0].1.clone()),
            })
        };
        let test = if parts[1].0.is_empty() {
            None
        } else {
            Some(WordDesc {
                text: parts[1].0.clone(),
                flags: 0,
                span: Span::new(0, 0, 0),
                raw: Some(parts[1].1.clone()),
            })
        };
        let step = if parts[2].0.is_empty() {
            None
        } else {
            Some(WordDesc {
                text: parts[2].0.clone(),
                flags: 0,
                span: Span::new(0, 0, 0),
                raw: Some(parts[2].1.clone()),
            })
        };

        self.skip_newlines();
        if self.peek_kind() == Some(TokenKind::Semicolon) {
            self.bump();
            self.skip_newlines();
        }
        let action = if self.peek_kind() == Some(TokenKind::LBrace) {
            self.parse_brace_body()?
        } else {
            self.expect_word("do")?;
            let action = self.parse_simple_list_until(&["done"])?;
            self.expect_word("done")?;
            action
        };
        Ok(Command {
            data: CommandData::ArithFor(ArithForCommand {
                init,
                test,
                step,
                action: Box::new(action),
            }),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn collect_arith_for_exprs(&mut self, open_span: Span) -> (String, Span) {
        let start = self
            .peek()
            .map(|token| token.span.start)
            .unwrap_or(self.input.len());
        let mut paren_depth = 0usize;
        while let Some(token) = self.peek() {
            match token.kind {
                TokenKind::DblRParen if paren_depth == 0 => {
                    let close_span = token.span;
                    let end = token.span.start.min(self.input.len());
                    self.bump();
                    return (
                        self.input[start.min(end)..end].to_string(),
                        Span::new(
                            open_span.source,
                            open_span.start.min(close_span.end),
                            close_span.end.min(self.input.len()),
                        ),
                    );
                }
                TokenKind::DblRParen => {
                    paren_depth = paren_depth.saturating_sub(2);
                    self.bump();
                }
                TokenKind::DblLParen => {
                    paren_depth += 2;
                    self.bump();
                }
                TokenKind::LParen => {
                    paren_depth += 1;
                    self.bump();
                }
                TokenKind::RParen => {
                    paren_depth = paren_depth.saturating_sub(1);
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
        let end = self.input.len();
        (
            self.input.get(start.min(end)..).unwrap_or("").to_string(),
            Span::new(open_span.source, open_span.start.min(end), end),
        )
    }

    fn parse_simple_command(&mut self) -> Result<Command, ParseError> {
        let mut words: Vec<WordDesc> = Vec::new();
        let mut redirects: Vec<Redirect> = Vec::new();
        let mut command_name: Option<String> = None;

        loop {
            if self.is_redir_word_redirector() {
                let redirector = self.bump().expect("redir word");
                let redirect = self.parse_redirect(Some(redirector))?;
                redirects.push(redirect);
                continue;
            }
            match self.peek_kind() {
                Some(TokenKind::Word) | Some(TokenKind::AssignmentWord) => {
                    let token = self.bump().expect("token");
                    let flags = token.word_flags;
                    let kind = token.kind.clone();
                    let span = token.span;
                    let text = match token.value {
                        TokenValue::Text(text) => text,
                        _ => token_value_to_string(&token.value),
                    };
                    let prefix_assignment =
                        matches!(kind, TokenKind::AssignmentWord) && command_name.is_none();
                    if flags & W_COMPASSIGN != 0
                        && command_name
                            .as_deref()
                            .is_some_and(|name| !assignment_builtin_accepts_compound(name))
                    {
                        return Err(ParseError {
                            message: "unexpected token '('".to_string(),
                            span: Some(span),
                        });
                    }
                    if !prefix_assignment && command_name.is_none() {
                        command_name = Some(text.clone());
                    }
                    words.push(WordDesc {
                        text,
                        flags,
                        span,
                        raw: self.raw_for_span(span),
                    });
                }
                Some(TokenKind::Number) => {
                    if self.number_is_adjacent_redirection_operator() {
                        let redirector = self.bump().expect("number");
                        let redirect = self.parse_redirect(Some(redirector))?;
                        redirects.push(redirect);
                    } else {
                        let token = self.bump().expect("number");
                        let text = match token.value {
                            TokenValue::Text(text) => text,
                            TokenValue::Number { raw, .. } => raw,
                            TokenValue::None => String::new(),
                        };
                        words.push(WordDesc {
                            text,
                            flags: 0,
                            span: token.span,
                            raw: self.raw_for_span(token.span),
                        });
                    }
                }
                Some(TokenKind::Less)
                | Some(TokenKind::Greater)
                | Some(TokenKind::LessLess)
                | Some(TokenKind::LessLessMinus)
                | Some(TokenKind::LessLessLess)
                | Some(TokenKind::GreaterGreater)
                | Some(TokenKind::GreaterBar)
                | Some(TokenKind::LessAnd)
                | Some(TokenKind::GreaterAnd)
                | Some(TokenKind::AndGreater)
                | Some(TokenKind::AndGreaterGreater)
                | Some(TokenKind::LessGreater) => {
                    let redirect = self.parse_redirect(None)?;
                    redirects.push(redirect);
                }
                _ => break,
            }
        }

        if words.is_empty() && redirects.is_empty() {
            return Err(ParseError {
                message: "expected command".to_string(),
                span: self.peek_span().cloned(),
            });
        }

        let simple = SimpleCommand { words, redirects };
        Ok(Command {
            data: CommandData::Simple(simple),
            flags: 0,
            line: 0,
            redirects: Vec::new(),
        })
    }

    fn parse_trailing_redirects(&mut self, command: &mut Command) -> Result<(), ParseError> {
        loop {
            if self.is_redir_word_redirector() {
                let redirector = self.bump().expect("redir word");
                let redirect = self.parse_redirect(Some(redirector))?;
                command.redirects.push(redirect);
                continue;
            }
            if self.current_is_redirection_operator() {
                let redirect = self.parse_redirect(None)?;
                command.redirects.push(redirect);
                continue;
            }
            if self.peek_kind() == Some(TokenKind::Number)
                && self.number_is_adjacent_redirection_operator()
            {
                let redirector = self.bump().expect("number");
                let redirect = self.parse_redirect(Some(redirector))?;
                command.redirects.push(redirect);
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_word_desc(&mut self) -> Result<WordDesc, ParseError> {
        let token = self.bump().ok_or(ParseError {
            message: "expected word".to_string(),
            span: self.peek_span().cloned(),
        })?;
        let flags = token.word_flags;
        let fallback = token_to_text_lossy(&token);
        let text = match token.value {
            TokenValue::Text(text) => text,
            TokenValue::Number { raw, .. } => raw,
            TokenValue::None => fallback,
        };
        Ok(WordDesc {
            text,
            flags,
            span: token.span,
            raw: self.raw_for_span(token.span),
        })
    }

    fn collect_pattern_word(
        &mut self,
        context: PatternCollectContext,
    ) -> Result<(WordDesc, bool), ParseError> {
        let first = self.peek().cloned().ok_or(ParseError {
            message: "expected pattern".to_string(),
            span: self.peek_span().cloned(),
        })?;
        let mut text = String::new();
        let mut flags = 0u32;
        let mut span = first.span;
        let mut depth = 0usize;
        let mut consumed_case_close = false;
        let mut previous_end: Option<usize> = None;

        while let Some(token) = self.peek().cloned() {
            if pattern_word_stops_before(&token.kind, context, depth) {
                break;
            }

            if context == PatternCollectContext::CondRegex && depth > 0 {
                if let Some(end) = previous_end {
                    if token.span.start > end {
                        if let Some(gap) = self.input.get(end..token.span.start) {
                            text.push_str(gap);
                        }
                    }
                }
            }

            if token.kind == TokenKind::DblRParen && context == PatternCollectContext::Case {
                match depth {
                    0 => break,
                    1 => {
                        self.bump();
                        text.push(')');
                        flags |= token.word_flags;
                        span.end = token.span.end;
                        consumed_case_close = true;
                        break;
                    }
                    _ => {
                        self.bump();
                        text.push_str("))");
                        flags |= token.word_flags;
                        span.end = token.span.end;
                        depth = depth.saturating_sub(2);
                        continue;
                    }
                }
            }

            self.bump();
            let piece = token_to_text_lossy(&token);
            match token.kind {
                TokenKind::LParen => {
                    if context == PatternCollectContext::CondRegex
                        || pattern_extglob_op_precedes(&text)
                    {
                        depth += 1;
                    }
                }
                TokenKind::RParen => {
                    depth = depth.saturating_sub(1);
                }
                TokenKind::DblRParen => {
                    if depth > 1 {
                        depth -= 2;
                    } else {
                        depth = 0;
                    }
                }
                _ => {}
            }
            text.push_str(&piece);
            flags |= token.word_flags;
            span.end = token.span.end;
            previous_end = Some(token.span.end);
        }

        if text.is_empty() {
            return Err(ParseError {
                message: "expected pattern".to_string(),
                span: Some(span),
            });
        }

        Ok((
            WordDesc {
                text,
                flags,
                span,
                raw: self.raw_for_span(span),
            },
            consumed_case_close,
        ))
    }

    fn parse_word_list_until_command_body(&mut self) -> Option<Vec<WordDesc>> {
        let mut words: Vec<WordDesc> = Vec::new();
        loop {
            match self.peek_kind() {
                Some(TokenKind::Word)
                | Some(TokenKind::AssignmentWord)
                | Some(TokenKind::Number) => {
                    if let Ok(word) = self.parse_word_desc() {
                        words.push(word);
                    } else {
                        break;
                    }
                }
                Some(TokenKind::Newline) => {
                    self.skip_newlines();
                    break;
                }
                Some(TokenKind::Semicolon) => {
                    self.bump();
                    self.skip_newlines();
                    break;
                }
                _ => break,
            }
        }
        if words.is_empty() {
            None
        } else {
            Some(words)
        }
    }

    fn parse_brace_body(&mut self) -> Result<Command, ParseError> {
        self.expect_kind(TokenKind::LBrace, "expected '{'")?;
        let body = self.parse_simple_list_until_kind(&[TokenKind::RBrace])?;
        if self.peek_kind() == Some(TokenKind::End) {
            return Err(self.unexpected_eof_error());
        }
        if !self.previous_token_terminated_list() {
            return Err(ParseError {
                message: "expected ';' or newline before '}'".to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.expect_kind(TokenKind::RBrace, "expected '}'")?;
        Ok(body)
    }

    fn is_word(&self, text: &str) -> bool {
        match self.peek() {
            Some(token) => match &token.value {
                TokenValue::Text(word) => word == text,
                _ => false,
            },
            None => false,
        }
    }

    fn unexpected_reserved_word(&self) -> Option<&str> {
        match self.peek() {
            Some(token)
                if matches!(
                    token.kind,
                    TokenKind::Then
                        | TokenKind::Else
                        | TokenKind::Elif
                        | TokenKind::Fi
                        | TokenKind::Do
                        | TokenKind::Done
                        | TokenKind::In
                        | TokenKind::Esac
                ) =>
            {
                match &token.value {
                    TokenValue::Text(word) => Some(word.as_str()),
                    _ => None,
                }
            }
            Some(token) if matches!(token.kind, TokenKind::Word) => match &token.value {
                TokenValue::Text(word)
                    if matches!(
                        word.as_str(),
                        "then" | "else" | "elif" | "fi" | "do" | "done" | "in" | "esac"
                    ) =>
                {
                    Some(word.as_str())
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn reject_unexpected_reserved_command(&self, stop_words: &[&str]) -> Result<(), ParseError> {
        if self.is_stop_word(stop_words) {
            return Ok(());
        }
        if let Some(word) = self.unexpected_reserved_word() {
            return Err(ParseError {
                message: format!("unexpected token '{}'", word),
                span: self.peek_span().cloned(),
            });
        }
        Ok(())
    }

    fn is_stop_word(&self, stop_words: &[&str]) -> bool {
        if stop_words.is_empty() {
            return false;
        }
        match self.peek() {
            Some(token) => match &token.value {
                TokenValue::Text(word) => stop_words.iter().any(|stop| *stop == word),
                _ => false,
            },
            None => false,
        }
    }

    fn is_stop_kind(&self, stop_kinds: &[TokenKind]) -> bool {
        if stop_kinds.is_empty() {
            return false;
        }
        match self.peek_kind() {
            Some(TokenKind::DblRParen) if stop_kinds.contains(&TokenKind::RParen) => true,
            Some(kind) => stop_kinds.contains(&kind),
            None => false,
        }
    }

    fn parse_simple_list_until_words_or_kinds(
        &mut self,
        stop_words: &[&str],
        stop_kinds: &[TokenKind],
    ) -> Result<Command, ParseError> {
        self.skip_newlines();
        if self.is_stop_word(stop_words) || self.is_stop_kind(stop_kinds) {
            return Err(ParseError {
                message: "expected list".to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.reject_unexpected_reserved_command(stop_words)?;
        let mut left = self.parse_and_or()?;
        loop {
            if self.is_stop_word(stop_words) || self.is_stop_kind(stop_kinds) {
                break;
            }
            match self.peek_kind() {
                Some(TokenKind::Semicolon) => {
                    self.bump();
                    self.skip_newlines();
                    if self.is_stop_word(stop_words) || self.is_stop_kind(stop_kinds) {
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        break;
                    }
                    self.reject_unexpected_reserved_command(stop_words)?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_SEMI);
                }
                Some(TokenKind::Ampersand) => {
                    self.bump();
                    self.skip_newlines();
                    if self.is_stop_word(stop_words) || self.is_stop_kind(stop_kinds) {
                        left = self.connect(left, self.null_command(), CONN_AMP);
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        left = self.connect(left, self.null_command(), CONN_AMP);
                        break;
                    }
                    self.reject_unexpected_reserved_command(stop_words)?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_AMP);
                }
                Some(TokenKind::Newline) => {
                    self.skip_newlines();
                    if self.is_stop_word(stop_words) || self.is_stop_kind(stop_kinds) {
                        break;
                    }
                    if self.peek_kind() == Some(TokenKind::End) {
                        break;
                    }
                    self.reject_unexpected_reserved_command(stop_words)?;
                    let right = self.parse_and_or()?;
                    left = self.connect(left, right, CONN_NEWLINE);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn expect_word(&mut self, text: &str) -> Result<(), ParseError> {
        if !self.is_word(text) {
            return Err(ParseError {
                message: format!("expected '{}'", text),
                span: self.peek_span().cloned(),
            });
        }
        self.bump();
        Ok(())
    }

    fn is_function_def_start(&self) -> bool {
        matches!(
            (self.peek_kind(), self.peek_n(1).map(|t| t.kind.clone())),
            (Some(TokenKind::Word), Some(TokenKind::LParen))
        ) && self.peek_n(2).map(|t| t.kind.clone()) == Some(TokenKind::RParen)
    }

    fn collect_arith_command_expr(&mut self, start: usize) -> Result<String, ParseError> {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        while let Some(token) = self.peek() {
            match token.kind {
                TokenKind::DblRParen if paren_depth == 0 && bracket_depth == 0 => {
                    let end = token.span.start.min(self.input.len());
                    self.bump();
                    return Ok(self.input[start.min(end)..end].to_string());
                }
                TokenKind::DblRParen if paren_depth == 1 && bracket_depth == 0 => {
                    let end = (token.span.start + 1).min(self.input.len());
                    self.bump();
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        self.bump();
                    }
                    return Ok(self.input[start.min(end)..end].to_string());
                }
                TokenKind::DblRParen => {
                    if bracket_depth > 0 && paren_depth == 1 {
                        paren_depth = 0;
                    } else {
                        paren_depth = paren_depth.saturating_sub(2);
                    }
                    self.bump();
                }
                TokenKind::DblLParen => {
                    paren_depth += 2;
                    self.bump();
                }
                TokenKind::LParen => {
                    paren_depth += 1;
                    self.bump();
                }
                TokenKind::RParen => {
                    paren_depth = paren_depth.saturating_sub(1);
                    self.bump();
                }
                TokenKind::End => {
                    return Err(ParseError {
                        message: if bracket_depth > 0 || paren_depth > 0 {
                            "unexpected EOF while looking for matching `)'".to_string()
                        } else {
                            "expected '))'".to_string()
                        },
                        span: self.peek_span().cloned(),
                    });
                }
                _ => {
                    update_arith_bracket_depth(token, &mut bracket_depth);
                    self.bump();
                }
            }
        }
        Err(ParseError {
            message: "expected '))'".to_string(),
            span: self.peek_span().cloned(),
        })
    }

    fn expect_kind(&mut self, kind: TokenKind, message: &str) -> Result<(), ParseError> {
        if kind == TokenKind::RParen && self.peek_kind() == Some(TokenKind::DblRParen) {
            let token = &mut self.tokens[self.index];
            token.kind = TokenKind::RParen;
            token.span.start = token.span.start.saturating_add(1);
            return Ok(());
        }
        if self.peek_kind() != Some(kind.clone()) {
            return Err(ParseError {
                message: message.to_string(),
                span: self.peek_span().cloned(),
            });
        }
        self.bump();
        Ok(())
    }

    fn is_redirection_operator(&self) -> bool {
        matches!(
            self.peek_n(1).map(|token| token.kind.clone()),
            Some(TokenKind::Less)
                | Some(TokenKind::Greater)
                | Some(TokenKind::LessLess)
                | Some(TokenKind::LessLessMinus)
                | Some(TokenKind::LessLessLess)
                | Some(TokenKind::GreaterGreater)
                | Some(TokenKind::GreaterBar)
                | Some(TokenKind::LessAnd)
                | Some(TokenKind::GreaterAnd)
                | Some(TokenKind::AndGreater)
                | Some(TokenKind::AndGreaterGreater)
                | Some(TokenKind::LessGreater)
        )
    }

    fn number_is_adjacent_redirection_operator(&self) -> bool {
        self.is_redirection_operator()
            && matches!(
                (self.peek(), self.peek_n(1)),
                (Some(number), Some(op)) if number.span.end == op.span.start
            )
    }

    fn current_is_redirection_operator(&self) -> bool {
        matches!(
            self.peek_kind(),
            Some(TokenKind::Less)
                | Some(TokenKind::Greater)
                | Some(TokenKind::LessLess)
                | Some(TokenKind::LessLessMinus)
                | Some(TokenKind::LessLessLess)
                | Some(TokenKind::GreaterGreater)
                | Some(TokenKind::GreaterBar)
                | Some(TokenKind::LessAnd)
                | Some(TokenKind::GreaterAnd)
                | Some(TokenKind::AndGreater)
                | Some(TokenKind::AndGreaterGreater)
                | Some(TokenKind::LessGreater)
        )
    }

    fn is_redir_word_redirector(&self) -> bool {
        matches!(self.peek(), Some(token) if is_redir_word_token(token))
            && matches!(self.peek_n(1), Some(next) if is_redirection_kind(&next.kind))
    }

    fn previous_token_terminated_list(&self) -> bool {
        if self.index == 0 {
            return false;
        }
        let Some(token) = self.tokens.get(self.index - 1) else {
            return false;
        };
        matches!(
            token.kind,
            TokenKind::Semicolon
                | TokenKind::Ampersand
                | TokenKind::Newline
                | TokenKind::RParen
                | TokenKind::DblRParen
                | TokenKind::RBrace
                | TokenKind::DblRBracket
        ) || matches!(
            &token.value,
            TokenValue::Text(word) if matches!(word.as_str(), "fi" | "done" | "esac" | "}")
        )
    }

    fn unexpected_eof_error(&self) -> ParseError {
        ParseError {
            message: "syntax error: unexpected end of file".to_string(),
            span: self.peek_span().cloned(),
        }
    }

    fn parse_redirect(&mut self, redirector_token: Option<Token>) -> Result<Redirect, ParseError> {
        let op_token = self.bump().expect("redirect operator");
        let instruction = match op_token.kind {
            TokenKind::Less => RedirectInstruction::InputDirection,
            TokenKind::Greater => RedirectInstruction::OutputDirection,
            TokenKind::LessLess => RedirectInstruction::ReadingUntil,
            TokenKind::LessLessMinus => RedirectInstruction::DeblankReadingUntil,
            TokenKind::LessLessLess => RedirectInstruction::ReadingString,
            TokenKind::GreaterGreater => RedirectInstruction::AppendingTo,
            TokenKind::GreaterBar => RedirectInstruction::OutputForce,
            TokenKind::LessAnd => RedirectInstruction::DuplicatingInput,
            TokenKind::GreaterAnd => RedirectInstruction::DuplicatingOutput,
            TokenKind::AndGreater => RedirectInstruction::ErrAndOut,
            TokenKind::AndGreaterGreater => RedirectInstruction::AppendErrAndOut,
            TokenKind::LessGreater => RedirectInstruction::InputOutput,
            _ => {
                return Err(ParseError {
                    message: "invalid redirection operator".to_string(),
                    span: Some(op_token.span),
                })
            }
        };

        let redirector = if let Some(token) = redirector_token {
            match token.value {
                TokenValue::Number { value, .. } => Redirector::Fd(value as i32),
                TokenValue::Text(text) => {
                    if let Some(name) = redir_word_name(&text) {
                        Redirector::Var(name.to_string())
                    } else if let Ok(value) = text.parse::<i32>() {
                        Redirector::Fd(value)
                    } else {
                        Redirector::Var(text)
                    }
                }
                TokenValue::None => Redirector::Fd(default_redirect_fd(&instruction)),
            }
        } else {
            Redirector::Fd(default_redirect_fd(&instruction))
        };

        let target_token = self.bump().ok_or(ParseError {
            message: "expected redirection target".to_string(),
            span: Some(op_token.span),
        })?;
        if !is_redirection_target(&target_token) {
            return Err(ParseError {
                message: "expected redirection target".to_string(),
                span: Some(target_token.span),
            });
        }
        let target_text = match &target_token.value {
            TokenValue::Text(text) => text.clone(),
            TokenValue::Number { raw, .. } => raw.clone(),
            TokenValue::None => String::new(),
        };

        // Refine LessAnd / GreaterAnd instruction based on the target shape.
        let mut instruction = instruction;
        let mut close_this = false;
        if matches!(
            instruction,
            RedirectInstruction::DuplicatingInput | RedirectInstruction::DuplicatingOutput
        ) {
            if target_text == "-" {
                instruction = RedirectInstruction::CloseThis;
                close_this = true;
            } else if let Some(stripped) = target_text.strip_suffix('-') {
                if !stripped.is_empty() && stripped.bytes().all(|b| b.is_ascii_digit()) {
                    instruction = match instruction {
                        RedirectInstruction::DuplicatingInput => RedirectInstruction::MoveInput,
                        RedirectInstruction::DuplicatingOutput => RedirectInstruction::MoveOutput,
                        _ => instruction,
                    };
                } else {
                    instruction = match instruction {
                        RedirectInstruction::DuplicatingInput => RedirectInstruction::MoveInputWord,
                        RedirectInstruction::DuplicatingOutput => {
                            RedirectInstruction::MoveOutputWord
                        }
                        _ => instruction,
                    };
                }
            } else if !target_text.bytes().all(|b| b.is_ascii_digit()) {
                instruction = match instruction {
                    RedirectInstruction::DuplicatingInput => {
                        RedirectInstruction::DuplicatingInputWord
                    }
                    RedirectInstruction::DuplicatingOutput => {
                        RedirectInstruction::DuplicatingOutputWord
                    }
                    _ => instruction,
                };
            }
        }

        let target_word = WordDesc {
            text: target_text.clone(),
            flags: target_token.word_flags,
            span: target_token.span,
            raw: self.raw_for_span(target_token.span),
        };

        let (here_doc_eof, here_doc_body) = match instruction {
            RedirectInstruction::ReadingUntil | RedirectInstruction::DeblankReadingUntil => {
                if self.here_doc_count >= HEREDOC_MAX {
                    return Err(ParseError {
                        message: "maximum here-document count exceeded".to_string(),
                        span: Some(op_token.span),
                    });
                }
                self.here_doc_count += 1;
                let allow_tabs = matches!(instruction, RedirectInstruction::DeblankReadingUntil);
                let mut delimiter = normalize_heredoc_delimiter(&target_word.text);
                if allow_tabs {
                    delimiter = delimiter.trim_start_matches('\t').to_string();
                }
                let remove_escaped_newlines = target_word.flags & cherubsh_common::W_QUOTED == 0;
                let (body, end_offset) = self.consume_heredoc_body(
                    &delimiter,
                    target_word.span.end,
                    allow_tabs,
                    remove_escaped_newlines,
                )?;
                self.here_doc_body_offset = Some(end_offset);
                (Some(delimiter), Some(body))
            }
            _ => (None, None),
        };
        let redirectee = if close_this {
            Redirectee::Fd(-1)
        } else if matches!(
            instruction,
            RedirectInstruction::DuplicatingInput
                | RedirectInstruction::DuplicatingOutput
                | RedirectInstruction::MoveInput
                | RedirectInstruction::MoveOutput
        ) {
            // FD form. For MoveInput/MoveOutput the trailing `-` was already accounted for;
            // the canonical fd is the digit prefix.
            let digits = target_text.trim_end_matches('-');
            let fd = digits.parse::<i32>().unwrap_or(-1);
            Redirectee::Fd(fd)
        } else {
            Redirectee::Word(target_word)
        };
        let rflags = if matches!(redirector, Redirector::Var(_)) {
            cherubsh_common::REDIR_VARASSIGN
        } else {
            0
        };
        Ok(Redirect {
            redirector,
            rflags,
            flags: 0,
            instruction,
            redirectee,
            here_doc_eof,
            here_doc_body,
        })
    }

    fn consume_heredoc_body(
        &mut self,
        delimiter: &str,
        start_offset: usize,
        allow_tabs: bool,
        remove_escaped_newlines: bool,
    ) -> Result<(String, usize), ParseError> {
        let input = self.input.as_str();
        if start_offset >= input.len() {
            return Ok((String::new(), start_offset));
        }
        let bytes = input.as_bytes();
        let mut cursor = if let Some(offset) = self.here_doc_body_offset {
            offset.max(start_offset)
        } else {
            let Some(offset) = self.initial_heredoc_body_cursor(start_offset, bytes) else {
                return Ok((String::new(), bytes.len()));
            };
            offset
        };
        let body_start = cursor;

        let mut body = String::new();
        loop {
            if cursor > bytes.len() {
                break;
            }
            let (line, line_end, consumed_newline) =
                self.heredoc_logical_line(cursor, remove_escaped_newlines);
            let compare_line = if allow_tabs {
                line.trim_start_matches('\t')
            } else {
                line.as_str()
            };
            if compare_line == delimiter {
                let end_offset = line_end;
                self.skip_tokens_in_range(body_start, end_offset);
                return Ok((body, end_offset));
            }

            body.push_str(&line);
            if consumed_newline {
                body.push('\n');
            }
            if line_end >= bytes.len() {
                self.skip_tokens_in_range(body_start, line_end);
                return Ok((body, line_end));
            }
            cursor = line_end;
        }
        Ok((body, cursor))
    }

    fn initial_heredoc_body_cursor(&self, start_offset: usize, bytes: &[u8]) -> Option<usize> {
        let mut newline = start_offset;
        while newline < bytes.len() && bytes[newline] != b'\n' {
            newline += 1;
        }
        if newline >= bytes.len() {
            return None;
        }

        let mut cursor = newline + 1;
        loop {
            let mut spanning_end = cursor;
            for token in &self.tokens {
                if token.span.start < cursor
                    && token.span.end > cursor
                    && token.span.end > spanning_end
                {
                    spanning_end = token.span.end;
                }
            }

            if spanning_end == cursor {
                return Some(cursor);
            }

            let mut extended_newline = spanning_end.min(bytes.len());
            while extended_newline < bytes.len() && bytes[extended_newline] != b'\n' {
                extended_newline += 1;
            }
            if extended_newline >= bytes.len() {
                return Some(extended_newline);
            }
            cursor = extended_newline + 1;
        }
    }

    fn heredoc_logical_line(
        &self,
        mut cursor: usize,
        remove_escaped_newlines: bool,
    ) -> (String, usize, bool) {
        let input = self.input.as_str();
        let bytes = input.as_bytes();
        let mut line = String::new();
        loop {
            let start = cursor;
            let mut line_end = cursor;
            while line_end < bytes.len() && bytes[line_end] != b'\n' {
                line_end += 1;
            }
            let physical = &input[start..line_end];
            let consumed_newline = line_end < bytes.len();
            if remove_escaped_newlines && consumed_newline && physical.ends_with('\\') {
                line.push_str(&physical[..physical.len() - 1]);
                cursor = line_end + 1;
                continue;
            }
            line.push_str(physical);
            let end = if consumed_newline {
                line_end + 1
            } else {
                line_end
            };
            return (line, end, consumed_newline);
        }
    }

    fn skip_tokens_in_range(&mut self, start_offset: usize, end_offset: usize) {
        let spanning_source = self
            .tokens
            .iter()
            .find(|token| {
                token.span.start >= start_offset
                    && token.span.start < end_offset
                    && token.span.end > end_offset
            })
            .map(|token| token.span.source);

        if let Some(source) = spanning_source {
            self.tokens.retain(|token| token.span.start < start_offset);
            let mut lexer = Lexer::with_source(&self.input[end_offset..], source);
            while let Some(mut token) = lexer.next_token() {
                token.span.start += end_offset;
                token.span.end += end_offset;
                self.tokens.push(token);
            }
            return;
        }

        let mut idx = self.index;
        while idx < self.tokens.len() {
            let token_start = self.tokens[idx].span.start;
            if token_start >= start_offset && token_start < end_offset {
                self.tokens.remove(idx);
            } else {
                idx += 1;
            }
        }
    }

    fn connect(&self, left: Command, right: Command, connector: u32) -> Command {
        let line = left.line;
        Command {
            data: CommandData::Connection(Connection {
                first: Box::new(left),
                second: Box::new(right),
                connector,
            }),
            flags: 0,
            line,
            redirects: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatternCollectContext {
    Case,
    Cond,
    CondRegex,
}

fn pattern_word_stops_before(
    kind: &TokenKind,
    context: PatternCollectContext,
    depth: usize,
) -> bool {
    if depth != 0 {
        return false;
    }
    match context {
        PatternCollectContext::Case => is_case_pattern_separator(kind),
        PatternCollectContext::Cond => matches!(
            kind,
            TokenKind::DblRBracket
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::RParen
                | TokenKind::Newline
        ),
        PatternCollectContext::CondRegex => {
            matches!(
                kind,
                TokenKind::DblRBracket
                    | TokenKind::AndAnd
                    | TokenKind::OrOr
                    | TokenKind::RParen
                    | TokenKind::Newline
            )
        }
    }
}

fn pattern_extglob_op_precedes(text: &str) -> bool {
    text.as_bytes()
        .last()
        .is_some_and(|b| matches!(*b, b'@' | b'+' | b'*' | b'?' | b'!'))
}

fn split_arith_for_clauses(text: &str) -> (Vec<(String, String)>, usize) {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut separators = 0usize;
    let mut paren_depth = 0usize;
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;

    let mut iter = text.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if single_quoted {
            if ch == '\'' {
                single_quoted = false;
            }
            continue;
        }
        if double_quoted {
            if ch == '"' {
                double_quoted = false;
            }
            continue;
        }

        if ch == '$' {
            if let Some((_, next)) = iter.peek().copied() {
                if next == '(' {
                    iter.next();
                    skip_arith_for_nested(&mut iter, '(', ')');
                    continue;
                }
                if next == '{' {
                    iter.next();
                    skip_arith_for_nested(&mut iter, '{', '}');
                    continue;
                }
            }
        }

        match ch {
            '\'' => single_quoted = true,
            '"' => double_quoted = true,
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            ';' if paren_depth == 0 => {
                separators += 1;
                if parts.len() < 2 {
                    parts.push(arith_for_clause_text(&text[start..idx]));
                    start = idx + ch.len_utf8();
                }
            }
            _ => {}
        }
    }

    parts.push(arith_for_clause_text(&text[start..]));
    (parts, separators)
}

fn skip_arith_for_nested<I>(iter: &mut std::iter::Peekable<I>, open: char, close: char)
where
    I: Iterator<Item = (usize, char)>,
{
    let mut depth = 1usize;
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    while let Some((_, ch)) = iter.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if single_quoted {
            if ch == '\'' {
                single_quoted = false;
            }
            continue;
        }
        if double_quoted {
            if ch == '"' {
                double_quoted = false;
            }
            continue;
        }
        if ch == '\'' {
            single_quoted = true;
            continue;
        }
        if ch == '"' {
            double_quoted = true;
            continue;
        }
        if ch == '$' {
            if let Some((_, next)) = iter.peek().copied() {
                if next == '(' {
                    iter.next();
                    skip_arith_for_nested(iter, '(', ')');
                    continue;
                }
                if next == '{' {
                    iter.next();
                    skip_arith_for_nested(iter, '{', '}');
                    continue;
                }
            }
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                break;
            }
        }
    }
}

fn arith_for_clause_text(text: &str) -> (String, String) {
    if text.trim().is_empty() {
        (String::new(), String::new())
    } else {
        (text.trim().to_string(), text.trim_start().to_string())
    }
}

fn token_value_to_string(value: &TokenValue) -> String {
    match value {
        TokenValue::None => String::new(),
        TokenValue::Text(text) => text.clone(),
        TokenValue::Number { raw, .. } => raw.clone(),
    }
}

fn token_to_text(token: &Token) -> String {
    match token.kind {
        TokenKind::Word | TokenKind::AssignmentWord | TokenKind::Number => {
            token_value_to_string(&token.value)
        }
        _ => String::new(),
    }
}

fn token_to_text_lossy(token: &Token) -> String {
    let text = token_to_text(token);
    if !text.is_empty() {
        return text;
    }
    match token.kind {
        TokenKind::End => "EOF",
        TokenKind::Newline => "newline",
        TokenKind::Semicolon => ";",
        TokenKind::Ampersand => "&",
        TokenKind::Bang => "!",
        TokenKind::Pipe => "|",
        TokenKind::BarAnd => "|&",
        TokenKind::AndAnd => "&&",
        TokenKind::OrOr => "||",
        TokenKind::Less => "<",
        TokenKind::Greater => ">",
        TokenKind::LessLess => "<<",
        TokenKind::LessLessMinus => "<<-",
        TokenKind::LessLessLess => "<<<",
        TokenKind::GreaterGreater => ">>",
        TokenKind::GreaterBar => ">|",
        TokenKind::LessAnd => "<&",
        TokenKind::GreaterAnd => ">&",
        TokenKind::AndGreater => "&>",
        TokenKind::AndGreaterGreater => "&>>",
        TokenKind::LessGreater => "<>",
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::DblLParen => "((",
        TokenKind::DblRParen => "))",
        TokenKind::DblLBracket => "[[",
        TokenKind::DblRBracket => "]]",
        TokenKind::LBrace => "{",
        TokenKind::RBrace => "}",
        TokenKind::DblSemicolon => ";;",
        TokenKind::SemiAmp => ";&",
        TokenKind::DblSemiAmp => ";;&",
        _ => "?",
    }
    .to_string()
}

fn update_arith_bracket_depth(token: &Token, depth: &mut usize) {
    let text = match &token.value {
        TokenValue::Text(text) => text.as_str(),
        TokenValue::Number { raw, .. } => raw.as_str(),
        TokenValue::None => return,
    };
    for byte in text.bytes() {
        match byte {
            b'[' => *depth += 1,
            b']' => *depth = depth.saturating_sub(1),
            _ => {}
        }
    }
}

fn assignment_builtin_accepts_compound(name: &str) -> bool {
    matches!(
        name,
        "declare" | "typeset" | "local" | "readonly" | "export" | "eval" | "let"
    )
}

fn is_case_pattern_separator(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RParen
            | TokenKind::Pipe
            | TokenKind::Newline
            | TokenKind::End
            | TokenKind::DblSemicolon
            | TokenKind::SemiAmp
            | TokenKind::DblSemiAmp
    )
}

fn is_redirection_kind(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Less
            | TokenKind::Greater
            | TokenKind::LessLess
            | TokenKind::LessLessMinus
            | TokenKind::LessLessLess
            | TokenKind::GreaterGreater
            | TokenKind::GreaterBar
            | TokenKind::LessAnd
            | TokenKind::GreaterAnd
            | TokenKind::AndGreater
            | TokenKind::AndGreaterGreater
            | TokenKind::LessGreater
    )
}

fn is_redirection_target(token: &Token) -> bool {
    matches!(
        token.kind,
        TokenKind::Word | TokenKind::AssignmentWord | TokenKind::Number
    )
}

fn redir_word_name(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('{')?.strip_suffix('}')?;
    if is_shell_identifier(inner) || is_shell_array_reference(inner) {
        Some(inner)
    } else {
        None
    }
}

fn is_redir_word_token(token: &Token) -> bool {
    matches!(&token.value, TokenValue::Text(text) if redir_word_name(text).is_some())
}

fn is_shell_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_shell_array_reference(text: &str) -> bool {
    let Some(open) = text.find('[') else {
        return false;
    };
    if !text.ends_with(']') || open == 0 {
        return false;
    }
    let name = &text[..open];
    let subscript = &text[open + 1..text.len() - 1];
    is_shell_identifier(name) && !subscript.is_empty()
}

fn add_stderr_to_stdout_redirect(command: &mut Command) {
    let redirect = Redirect {
        redirector: Redirector::Fd(2),
        rflags: 0,
        flags: 0,
        instruction: RedirectInstruction::DuplicatingOutput,
        redirectee: Redirectee::Fd(1),
        here_doc_eof: None,
        here_doc_body: None,
    };
    match &mut command.data {
        CommandData::Simple(simple) => simple.redirects.push(redirect),
        _ => command.redirects.push(redirect),
    }
}

fn normalize_heredoc_delimiter(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                } else {
                    out.push(ch);
                }
            }
            '\'' => {
                for next in chars.by_ref() {
                    if next == '\'' {
                        break;
                    }
                    out.push(next);
                }
            }
            '"' => {
                for next in chars.by_ref() {
                    if next == '"' {
                        break;
                    }
                    out.push(next);
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

fn cond_leaf(word: WordDesc) -> CondCommand {
    CondCommand {
        cond_type: CondType::Term,
        op: None,
        left: None,
        right: None,
        term: Some(word),
    }
}

const UNARY_TEST_OPS: &[&str] = &[
    "-a", "-b", "-c", "-d", "-e", "-f", "-g", "-h", "-k", "-n", "-o", "-p", "-r", "-s", "-t", "-u",
    "-v", "-w", "-x", "-z", "-G", "-L", "-N", "-O", "-R", "-S",
];

const BINARY_TEST_OPS: &[&str] = &[
    "==", "=", "!=", "=~", "<", ">", "-eq", "-ne", "-lt", "-le", "-gt", "-ge", "-ef", "-nt", "-ot",
];

fn is_unary_test_op(text: &str) -> bool {
    UNARY_TEST_OPS.contains(&text)
}

fn is_binary_test_op(text: &str) -> bool {
    BINARY_TEST_OPS.contains(&text)
}

fn default_redirect_fd(instruction: &RedirectInstruction) -> i32 {
    match instruction {
        RedirectInstruction::InputDirection
        | RedirectInstruction::InputaDirection
        | RedirectInstruction::ReadingUntil
        | RedirectInstruction::ReadingString
        | RedirectInstruction::DeblankReadingUntil
        | RedirectInstruction::DuplicatingInput
        | RedirectInstruction::DuplicatingInputWord
        | RedirectInstruction::MoveInput
        | RedirectInstruction::MoveInputWord
        | RedirectInstruction::InputOutput => 0,
        RedirectInstruction::OutputDirection
        | RedirectInstruction::AppendingTo
        | RedirectInstruction::DuplicatingOutput
        | RedirectInstruction::DuplicatingOutputWord
        | RedirectInstruction::OutputForce
        | RedirectInstruction::MoveOutput
        | RedirectInstruction::MoveOutputWord
        | RedirectInstruction::ErrAndOut
        | RedirectInstruction::AppendErrAndOut => 1,
        RedirectInstruction::CloseThis => 0,
    }
}

fn line_starts(input: &str) -> Vec<usize> {
    let mut starts =
        Vec::with_capacity(1 + input.as_bytes().iter().filter(|b| **b == b'\n').count());
    starts.push(0);
    for (idx, byte) in input.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn line_number_for_offset_cached(line_starts: &[usize], input_len: usize, offset: usize) -> u32 {
    let end = offset.min(input_len);
    line_starts.partition_point(|start| *start <= end) as u32
}

#[cfg(test)]
fn line_number_for_offset(input: &str, offset: usize) -> u32 {
    line_number_for_offset_cached(&line_starts(input), input.len(), offset)
}

#[cfg(test)]
mod tests {
    use super::{line_number_for_offset, CommandData, Parser};
    use cherubsh_lexer::Lexer;

    fn parse_command(input: &str) -> CommandData {
        parse_ast(input).root.data
    }

    fn parse_ast(input: &str) -> super::Ast {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        parser.parse().expect("parse")
    }

    fn parse_err(input: &str) {
        let _ = parse_error(input);
    }

    #[test]
    fn cached_line_lookup_matches_scanning() {
        let input = "one\ntwo\n\nfour";
        let parser = Parser::new(Vec::new(), input);
        for offset in 0..=input.len() {
            assert_eq!(
                parser.line_number_for_offset(offset),
                line_number_for_offset(input, offset),
                "offset {offset}"
            );
        }
    }

    fn parse_error(input: &str) -> super::ParseError {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        parser
            .parse()
            .expect_err(&format!("expected parse error for {input:?}"))
    }

    #[test]
    fn parse_if_command() {
        let data = parse_command("if true; then echo ok; fi");
        assert!(matches!(data, CommandData::If(_)));
    }

    #[test]
    fn parse_while_command() {
        let data = parse_command("while true; do echo ok; done");
        assert!(matches!(data, CommandData::While(_)));
    }

    #[test]
    fn parse_until_command() {
        let data = parse_command("until true; do echo ok; done");
        assert!(matches!(data, CommandData::Until(_)));
    }

    #[test]
    fn parse_for_command() {
        let data = parse_command("for i in a b; do echo $i; done");
        assert!(matches!(data, CommandData::For(_)));
    }

    #[test]
    fn parse_arith_for_command() {
        let data = parse_command("for ((i=0;i<3;i++)); do echo $i; done");
        assert!(matches!(data, CommandData::ArithFor(_)));
    }

    #[test]
    fn parse_arith_for_preserves_compound_operators() {
        let data = parse_command("for ((i=0;i<=3;i++)); do echo $i; done");
        match data {
            CommandData::ArithFor(cmd) => {
                assert_eq!(
                    cmd.init.as_ref().map(|word| word.text.as_str()),
                    Some("i=0")
                );
                assert_eq!(
                    cmd.test.as_ref().map(|word| word.text.as_str()),
                    Some("i<=3")
                );
                assert_eq!(
                    cmd.step.as_ref().map(|word| word.text.as_str()),
                    Some("i++")
                );
            }
            _ => panic!("expected arith-for"),
        }
    }

    #[test]
    fn parse_arith_for_keeps_nested_arith_condition() {
        let data = parse_command("for (( i=0 ; (( i < n )) ; i++ )) ; do echo $i; done");
        match data {
            CommandData::ArithFor(cmd) => {
                assert_eq!(
                    cmd.init.as_ref().map(|word| word.text.as_str()),
                    Some("i=0")
                );
                assert_eq!(
                    cmd.test.as_ref().map(|word| word.text.as_str()),
                    Some("(( i < n ))")
                );
                assert_eq!(
                    cmd.step.as_ref().map(|word| word.text.as_str()),
                    Some("i++")
                );
            }
            _ => panic!("expected arith-for"),
        }
    }

    #[test]
    fn parse_arith_for_ignores_semicolon_inside_command_substitution() {
        let data = parse_command(
            r#"for (( i = j = k = 1; i % 9 || (j *= -1, $( ((i%9)) || printf " " >&2; echo 0), k++ <= 10); i += j )); do :; done"#,
        );
        match data {
            CommandData::ArithFor(cmd) => {
                assert_eq!(
                    cmd.init.as_ref().map(|word| word.text.as_str()),
                    Some("i = j = k = 1")
                );
                assert_eq!(
                    cmd.test.as_ref().map(|word| word.text.as_str()),
                    Some(r#"i % 9 || (j *= -1, $( ((i%9)) || printf " " >&2; echo 0), k++ <= 10)"#)
                );
                assert_eq!(
                    cmd.step.as_ref().map(|word| word.text.as_str()),
                    Some("i += j")
                );
            }
            _ => panic!("expected arith-for"),
        }
    }

    #[test]
    fn parse_arith_for_rejects_missing_or_extra_clause_separator() {
        parse_err(r#"for (( i=0; "i < 3" )); do echo $i; done"#);
        parse_err("for (( i=0; i < 3; i++; 7 )); do echo $i; done");
    }

    #[test]
    fn parse_nested_paren_arith_command() {
        let data = parse_command("if (((4+4) + (4 + 7))); then echo ok; fi");
        assert!(matches!(data, CommandData::If(_)));
    }

    #[test]
    fn parse_group_command() {
        let data = parse_command("{ echo ok; }");
        assert!(matches!(data, CommandData::Group(_)));
    }

    #[test]
    fn parse_subshell_command() {
        let ast = parse_ast("( echo ok )");
        assert!(matches!(ast.root.data, CommandData::Subshell(_)));
        assert!(ast.root.flags & cherubsh_common::CMD_WANT_SUBSHELL != 0);
    }

    #[test]
    fn parse_double_lparen_command_list_as_subshells() {
        let ast = parse_ast("((echo abc; echo def;); echo ghi)");
        assert!(matches!(ast.root.data, CommandData::Subshell(_)));
    }

    #[test]
    fn parse_multiline_double_lparen_command_list_as_subshells() {
        let ast = parse_ast("(( echo 1\necho 2\n(( x ))\n: $(( x ))\necho 3\n) )");
        assert!(matches!(ast.root.data, CommandData::Subshell(_)));
    }

    #[test]
    fn parse_case_time_pattern() {
        let data = parse_command("case k in else|done|time|esac) echo ok;; esac");
        assert!(matches!(data, CommandData::Case(_)));
    }

    #[test]
    fn parse_case_rejects_unexpected_reserved_action_word() {
        let err = parse_error("case x in x) ;; x) done ;; esac");
        assert_eq!(err.message, "unexpected token 'done'");

        let err = parse_error("case x in x) (esac) esac");
        assert_eq!(err.message, "unexpected token 'esac'");
    }

    #[test]
    fn parse_case_command() {
        let data = parse_command("case x in a) echo a ;; b) echo b ;; esac");
        assert!(matches!(data, CommandData::Case(_)));
    }

    #[test]
    fn parse_case_extglob_pattern() {
        let data = parse_command("case 12 in 0|[1-9]*([0-9])) echo ok ;; esac");
        match data {
            CommandData::Case(case_cmd) => {
                assert_eq!(case_cmd.clauses.len(), 1);
                assert_eq!(case_cmd.clauses[0].patterns[0].text, "0");
                assert_eq!(case_cmd.clauses[0].patterns[1].text, "[1-9]*([0-9])");
            }
            _ => panic!("expected case command"),
        }
    }

    #[test]
    fn parse_function_def_command() {
        let data = parse_command("foo() { echo ok; }");
        assert!(matches!(data, CommandData::FunctionDef(_)));
    }

    #[test]
    fn parse_function_kw_no_parens() {
        let data = parse_command("function f { :; }");
        assert!(matches!(data, CommandData::FunctionDef(_)));
    }

    #[test]
    fn parse_function_def_newline_before_body() {
        let data = parse_command("f()\n{ :; }");
        assert!(matches!(data, CommandData::FunctionDef(_)));
    }

    #[test]
    fn parse_function_body_subshell_can_close_before_brace() {
        let data = parse_command("foo(){( echo hi )}\n");
        assert!(matches!(data, CommandData::FunctionDef(_)));
    }

    #[test]
    fn parse_function_body_arith_can_close_before_brace() {
        let data = parse_command("foo(){ ((1))}\n");
        assert!(matches!(data, CommandData::FunctionDef(_)));
    }

    #[test]
    fn parse_coproc_named_with_compound() {
        let data = parse_command("coproc x { sleep 1; }");
        assert!(matches!(data, CommandData::Coproc(_)));
    }

    #[test]
    fn parse_coproc_subshell() {
        let ast = parse_ast("coproc ( sleep 1 )");
        assert!(matches!(ast.root.data, CommandData::Coproc(_)));
        assert!(ast.root.flags & cherubsh_common::CMD_COPROC_SUBSHELL != 0);
    }

    #[test]
    fn parse_select_command() {
        let data = parse_command("select x in a b; do echo $x; done");
        assert!(matches!(data, CommandData::Select(_)));
    }

    #[test]
    fn parse_cond_command() {
        let data = parse_command("[[ a == b ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Binary));
                assert_eq!(c.op.as_ref().unwrap().text, "==");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_unary() {
        let data = parse_command("[[ -n $x ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Unary));
                assert_eq!(c.op.as_ref().unwrap().text, "-n");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_implicit_n() {
        let data = parse_command("[[ $x ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Unary));
                assert_eq!(c.op.as_ref().unwrap().text, "-n");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_and_or() {
        let data = parse_command("[[ $a == $b && -n $c ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::And));
                let left = c.left.as_ref().unwrap();
                let right = c.right.as_ref().unwrap();
                assert!(matches!(left.cond_type, super::CondType::Binary));
                assert!(matches!(right.cond_type, super::CondType::Unary));
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_paren_group() {
        let data = parse_command("[[ ( $a == $b ) || -n $c ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Or));
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_negation() {
        let data = parse_command("[[ ! -e /tmp ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Term));
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_regex() {
        let data = parse_command("[[ $x =~ abc ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Binary));
                assert_eq!(c.op.as_ref().unwrap().text, "=~");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_regex_rhs_accepts_ere_grouping() {
        let data = parse_command(
            "[[ jbig2dec-0.9-i586-001.tgz =~ ([^-]+)-([^-]+)-([^-]+)-0*([1-9][0-9]*)\\.tgz ]]",
        );
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Binary));
                let rhs = c
                    .right
                    .as_ref()
                    .and_then(|right| right.term.as_ref())
                    .expect("rhs");
                assert_eq!(rhs.text, "([^-]+)-([^-]+)-([^-]+)-0*([1-9][0-9]*)\\.tgz");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_regex_rhs_preserves_spaces_inside_groups() {
        let data = parse_command("[[ ${v} =~ (one two) ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Binary));
                let rhs = c
                    .right
                    .as_ref()
                    .and_then(|right| right.term.as_ref())
                    .expect("rhs");
                assert_eq!(rhs.text, "(one two)");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_cond_extglob_rhs() {
        let data = parse_command("[[ ab/../ == @(ab|+([^/]))/..?(/) ]]");
        match data {
            CommandData::Cond(c) => {
                assert!(matches!(c.cond_type, super::CondType::Binary));
                let rhs = c
                    .right
                    .as_ref()
                    .and_then(|right| right.term.as_ref())
                    .expect("rhs");
                assert_eq!(rhs.text, "@(ab|+([^/]))/..?(/)");
            }
            _ => panic!("expected cond"),
        }
    }

    #[test]
    fn parse_arith_command() {
        let data = parse_command("((1+2))");
        assert!(matches!(data, CommandData::Arith(_)));
    }

    #[test]
    fn parse_arith_command_preserves_compound_operators() {
        let data = parse_command("(( now1 - offset <= now2 && now2 >= now1 ))");
        match data {
            CommandData::Arith(cmd) => {
                assert_eq!(
                    cmd.expression.text,
                    " now1 - offset <= now2 && now2 >= now1 "
                );
            }
            _ => panic!("expected arith command"),
        }
    }

    #[test]
    fn parse_arith_command_accepts_adjacent_grouping_parentheses() {
        let data = parse_command("(( strength = 100 - (100 * ((level + 40) * -1 ) / 60 ) ))");
        match data {
            CommandData::Arith(cmd) => assert_eq!(
                cmd.expression.text,
                " strength = 100 - (100 * ((level + 40) * -1 ) / 60 ) ",
            ),
            _ => panic!("expected arith command"),
        }
    }

    #[test]
    fn parse_bang_negation_sets_invert_flag() {
        let input = "! true";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        assert!(ast.root.flags & cherubsh_common::CMD_INVERT_RETURN != 0);
    }

    #[test]
    fn parse_time_pipeline_sets_time_flag() {
        let input = "time -p sleep 1";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        assert!(ast.root.flags & cherubsh_common::CMD_TIME_PIPELINE != 0);
        assert!(ast.root.flags & cherubsh_common::CMD_TIME_POSIX != 0);
    }

    #[test]
    fn parse_input_unit_accepts_empty() {
        let input = "\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        assert!(parser.parse_input_unit().expect("input unit").is_none());
    }

    #[test]
    fn parse_null_time_and_bang_commands() {
        let ast = parse_ast("time;");
        assert!(ast.root.flags & cherubsh_common::CMD_TIME_PIPELINE != 0);
        let ast = parse_ast("!;");
        assert!(ast.root.flags & cherubsh_common::CMD_INVERT_RETURN != 0);
        let ast = parse_ast("! !;");
        assert_eq!(ast.root.flags & cherubsh_common::CMD_INVERT_RETURN, 0);
        let ast = parse_ast("! time echo a");
        assert!(ast.root.flags & cherubsh_common::CMD_INVERT_RETURN != 0);
        assert!(ast.root.flags & cherubsh_common::CMD_TIME_PIPELINE != 0);
    }

    #[test]
    fn parse_pipeline_allows_newline_after_pipe() {
        let data = parse_command("true |\nfalse");
        assert!(matches!(data, CommandData::Connection(_)));
    }

    #[test]
    fn parse_trailing_background_operator() {
        let data = parse_command("sleep 1 &");
        match data {
            CommandData::Connection(conn) => {
                assert_eq!(conn.connector, super::CONN_AMP);
                assert!(matches!(&conn.first.data, CommandData::Simple(_)));
                match &conn.second.data {
                    CommandData::Simple(simple) => {
                        assert!(simple.words.is_empty());
                        assert!(simple.redirects.is_empty());
                    }
                    _ => panic!("expected null command on rhs"),
                }
            }
            _ => panic!("expected background connection"),
        }
    }

    #[test]
    fn parse_trailing_background_before_group_close() {
        let data = parse_command("{ sleep 1 & }");
        match data {
            CommandData::Group(group) => match &group.command.data {
                CommandData::Connection(conn) => {
                    assert_eq!(conn.connector, super::CONN_AMP);
                    assert!(
                        matches!(&conn.second.data, CommandData::Simple(simple) if simple.words.is_empty())
                    );
                }
                _ => panic!("expected background connection in group"),
            },
            _ => panic!("expected group command"),
        }
    }

    #[test]
    fn parse_bar_and_adds_stderr_redirect() {
        let data = parse_command("true |& false");
        match data {
            CommandData::Connection(conn) => {
                assert_eq!(conn.connector, super::CONN_PIPE);
                match conn.first.data {
                    CommandData::Simple(simple) => {
                        assert_eq!(simple.redirects.len(), 1);
                        assert!(matches!(
                            simple.redirects[0].instruction,
                            super::RedirectInstruction::DuplicatingOutput
                        ));
                    }
                    _ => panic!("expected simple pipeline lhs"),
                }
            }
            _ => panic!("expected connection"),
        }
    }

    #[test]
    fn parse_compound_redirects_attach_to_command() {
        let ast = parse_ast("if true; then :; fi >out");
        assert_eq!(ast.root.redirects.len(), 1);
        let ast = parse_ast("{ echo a; } >out");
        assert_eq!(ast.root.redirects.len(), 1);
    }

    #[test]
    fn parse_function_body_redirect() {
        let data = parse_command("f() { :; } >out");
        match data {
            CommandData::FunctionDef(func) => {
                assert_eq!(func.command.redirects.len(), 1);
            }
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn parse_for_and_select_brace_bodies() {
        assert!(matches!(
            parse_command("for i; { :; }"),
            CommandData::For(_)
        ));
        assert!(matches!(
            parse_command("select x; { :; }"),
            CommandData::Select(_)
        ));
    }

    #[test]
    fn parse_redir_word_redirector_at_command_start() {
        let data = parse_command("{fd}<>out echo ok");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.redirects.len(), 1);
                assert!(matches!(
                    simple.redirects[0].redirector,
                    super::Redirector::Var(ref name) if name == "fd"
                ));
            }
            _ => panic!("expected simple"),
        }
    }

    #[test]
    fn parse_array_redir_word_redirector() {
        let data = parse_command("exec {fd[0]}<&0");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.redirects.len(), 1);
                assert!(matches!(
                    simple.redirects[0].redirector,
                    super::Redirector::Var(ref name) if name == "fd[0]"
                ));
            }
            _ => panic!("expected simple"),
        }
    }

    #[test]
    fn reject_trailing_garbage_and_missing_redirect_target() {
        parse_err("echo >");
        parse_err("for i in a b do :; done");
        parse_err("{ echo a }");
    }

    #[test]
    fn parse_compound_assignment() {
        let data = parse_command("arr=(a b c)");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.words.len(), 1);
                let w = &simple.words[0];
                assert!(w.flags & cherubsh_common::W_COMPASSIGN != 0);
                assert!(w.flags & cherubsh_common::W_ASSIGNMENT != 0);
                assert_eq!(w.text, "arr=(a b c)");
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_process_substitution_in_compound_assignment() {
        let data = parse_command("arr=(<(echo hi) \">\")");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.words.len(), 1);
                assert!(simple.words[0].flags & cherubsh_common::W_COMPASSIGN != 0);
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_array_element_assignment() {
        let data = parse_command("a[0]=hi");
        match data {
            CommandData::Simple(simple) => {
                let w = &simple.words[0];
                assert!(w.flags & cherubsh_common::W_ARRAYREF != 0);
                assert!(w.flags & cherubsh_common::W_ASSIGNMENT != 0);
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_close_this_redirect() {
        let data = parse_command("exec 3>&-");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.redirects.len(), 1);
                assert!(matches!(
                    simple.redirects[0].instruction,
                    super::RedirectInstruction::CloseThis
                ));
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_move_output_redirect() {
        let data = parse_command("cmd 2>&1-");
        match data {
            CommandData::Simple(simple) => {
                let redir = simple.redirects.first().expect("redirect");
                assert!(matches!(
                    redir.instruction,
                    super::RedirectInstruction::MoveOutput
                ));
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_dup_output_word_redirect() {
        let data = parse_command("cmd 2>&fd");
        match data {
            CommandData::Simple(simple) => {
                let redir = simple.redirects.first().expect("redirect");
                assert!(matches!(
                    redir.instruction,
                    super::RedirectInstruction::DuplicatingOutputWord
                ));
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_case_with_fallthrough() {
        let data = parse_command("case x in a) echo a ;& b) echo b ;; esac");
        match data {
            CommandData::Case(case_cmd) => {
                assert_eq!(case_cmd.clauses.len(), 2);
                assert!(case_cmd.clauses[0].flags & cherubsh_common::CASEPAT_FALLTHROUGH != 0);
            }
            _ => panic!("expected case command"),
        }
    }

    #[test]
    fn parse_case_with_testnext() {
        let data = parse_command("case x in a) echo a ;;& b) echo b ;; esac");
        match data {
            CommandData::Case(case_cmd) => {
                assert_eq!(case_cmd.clauses.len(), 2);
                assert!(case_cmd.clauses[0].flags & cherubsh_common::CASEPAT_TESTNEXT != 0);
            }
            _ => panic!("expected case command"),
        }
    }

    #[test]
    fn parse_heredoc_body() {
        let input = "cat <<EOF\nhello\nEOF\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Simple(simple) => {
                let redirect = simple.redirects.first().expect("redirect");
                let body = redirect.here_doc_body.as_deref().expect("here-doc body");
                assert_eq!(body, "hello\n");
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_multiple_heredoc_bodies_in_redirect_order() {
        let input = "cat <<EOF1 <<EOF2\none\nEOF1\ntwo\nEOF2\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.redirects.len(), 2);
                assert_eq!(simple.redirects[0].here_doc_body.as_deref(), Some("one\n"));
                assert_eq!(simple.redirects[1].here_doc_body.as_deref(), Some("two\n"));
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn parse_outer_heredoc_starts_after_multiline_command_substitution() {
        let input = "cat <<EOF && grep $(\n foobar\nEOF\necho notthereanywhere) *.c\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => match conn.first.data {
                CommandData::Simple(simple) => {
                    let redirect = simple.redirects.first().expect("redirect");
                    assert_eq!(redirect.here_doc_eof.as_deref(), Some("EOF"));
                    assert_eq!(redirect.here_doc_body.as_deref(), Some(""));
                }
                _ => panic!("expected simple command"),
            },
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn reject_more_than_sixteen_heredocs() {
        let input = "cat <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF\n";
        let err = parse_error(input);
        assert_eq!(err.message, "maximum here-document count exceeded");
    }

    #[test]
    fn unterminated_group_reports_unexpected_eof() {
        let err = parse_error("foo() { echo hi\n");
        assert_eq!(err.message, "syntax error: unexpected end of file");
    }

    #[test]
    fn parse_backslash_quoted_heredoc_delimiter() {
        let input = "cat << \\EOF\nhello\nEOF\necho after\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => match conn.first.data {
                CommandData::Simple(simple) => {
                    let redirect = simple.redirects.first().expect("redirect");
                    assert_eq!(redirect.here_doc_eof.as_deref(), Some("EOF"));
                    assert_eq!(redirect.here_doc_body.as_deref(), Some("hello\n"));
                }
                _ => panic!("expected simple command"),
            },
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn parse_line_continued_heredoc_delimiter() {
        let input = "cat << EO\\\nF\nhello\nEOF\necho after\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => match conn.first.data {
                CommandData::Simple(simple) => {
                    let redirect = simple.redirects.first().expect("redirect");
                    assert_eq!(redirect.here_doc_eof.as_deref(), Some("EOF"));
                    assert_eq!(redirect.here_doc_body.as_deref(), Some("hello\n"));
                }
                _ => panic!("expected simple command"),
            },
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn parse_unquoted_heredoc_matches_line_continued_body_delimiter() {
        let input = "cat <<EOF\nhello\nEO\\\nF\necho after\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => match conn.first.data {
                CommandData::Simple(simple) => {
                    let redirect = simple.redirects.first().expect("redirect");
                    assert_eq!(redirect.here_doc_body.as_deref(), Some("hello\n"));
                }
                _ => panic!("expected simple command"),
            },
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn parse_unquoted_heredoc_removes_body_line_continuations() {
        let input = "cat <<END\nhello\\\nEND\nEND\necho after\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => match conn.first.data {
                CommandData::Simple(simple) => {
                    let redirect = simple.redirects.first().expect("redirect");
                    assert_eq!(redirect.here_doc_body.as_deref(), Some("helloEND\n"));
                }
                _ => panic!("expected simple command"),
            },
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn parse_unquoted_heredoc_skips_line_continued_body_tokens() {
        let input = "cat <<END\nhello\nEND\\\nEND\nEND\necho end ENDEND\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => {
                match conn.first.data {
                    CommandData::Simple(simple) => {
                        let redirect = simple.redirects.first().expect("redirect");
                        assert_eq!(redirect.here_doc_body.as_deref(), Some("hello\nENDEND\n"));
                    }
                    _ => panic!("expected simple command"),
                }
                match conn.second.data {
                    CommandData::Simple(simple) => {
                        assert_eq!(simple.words[0].text, "echo");
                    }
                    _ => panic!("expected simple command"),
                }
            }
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn parse_deblank_heredoc_strips_tabs_before_quoted_delimiter() {
        let input = "cat <<-'\tEND'\n\thello\n\tEND\necho after\n";
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            tokens.push(token);
        }
        let mut parser = Parser::new(tokens, input);
        let ast = parser.parse().expect("parse");
        match ast.root.data {
            CommandData::Connection(conn) => match conn.first.data {
                CommandData::Simple(simple) => {
                    let redirect = simple.redirects.first().expect("redirect");
                    assert_eq!(redirect.here_doc_eof.as_deref(), Some("END"));
                    assert_eq!(redirect.here_doc_body.as_deref(), Some("\thello\n"));
                }
                _ => panic!("expected simple command"),
            },
            _ => panic!("expected command list"),
        }
    }

    #[test]
    fn number_before_spaced_redirect_is_word() {
        let data = parse_command("echo 0 >&2");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.words.len(), 2);
                assert_eq!(simple.words[1].text, "0");
                assert_eq!(simple.redirects.len(), 1);
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn adjacent_number_before_redirect_is_redirector() {
        let data = parse_command("echo 0>&2");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.words.len(), 1);
                assert_eq!(simple.redirects.len(), 1);
            }
            _ => panic!("expected simple command"),
        }
    }

    #[test]
    fn oversized_number_before_redirect_is_word() {
        let data = parse_command("2147483648</dev/null");
        match data {
            CommandData::Simple(simple) => {
                assert_eq!(simple.words.len(), 1);
                assert_eq!(simple.words[0].text, "2147483648");
                assert_eq!(simple.redirects.len(), 1);
            }
            _ => panic!("expected simple command"),
        }
    }
}
