//! `CommandRunner` implementation. Bridges the expander back into exec so
//! `$(cmd)`, backticks, and `<(cmd)` can fork+exec sub-shells.

use std::io::Read;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::OnceLock;

use cherubsh_common::{expand_aliases_for_parse, Environment, ProcSubstDir};
use cherubsh_expander::{CommandRunner, ExpandError, ProcSubstHandle};
use cherubsh_lexer::{Lexer, TokenKind};
use cherubsh_parser::{Command, CommandData, Parser};

use crate::{execute_with_state, ExecState, FunctionMap};

pub struct ExecRunner<'a> {
    functions: &'a FunctionMap,
}

impl<'a> ExecRunner<'a> {
    pub(crate) fn with_functions(functions: &'a FunctionMap) -> Self {
        Self { functions }
    }
}

impl Default for ExecRunner<'static> {
    fn default() -> Self {
        static EMPTY_FUNCTIONS: OnceLock<FunctionMap> = OnceLock::new();
        Self {
            functions: EMPTY_FUNCTIONS.get_or_init(Default::default),
        }
    }
}

impl CommandRunner for ExecRunner<'_> {
    fn run_subst(&mut self, env: &mut dyn Environment, src: &str) -> Result<Vec<u8>, ExpandError> {
        self.run_subst_with_mode(env, src, SubstMode::DollarParen)
    }

    fn run_backquote_subst(
        &mut self,
        env: &mut dyn Environment,
        src: &str,
    ) -> Result<Vec<u8>, ExpandError> {
        self.run_subst_with_mode(env, src, SubstMode::Backquote)
    }

    fn spawn_proc_subst(
        &mut self,
        env: &mut dyn Environment,
        dir: ProcSubstDir,
        src: &str,
    ) -> Result<ProcSubstHandle, ExpandError> {
        let mut fds: [RawFd; 2] = [0, 0];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(ExpandError::Io("pipe failed".into()));
        }
        let (parent_fd, child_fd, child_target) = match dir {
            ProcSubstDir::Input => (fds[0], fds[1], 1i32), // parent reads, child writes stdout
            ProcSubstDir::Output => (fds[1], fds[0], 0i32), // parent writes, child reads stdin
        };
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            return Err(ExpandError::Io("fork failed".into()));
        }
        if pid == 0 {
            // Child
            unsafe {
                libc::close(parent_fd);
                libc::dup2(child_fd, child_target);
                libc::close(child_fd);
            }
            env.enter_subshell();
            let status = run_inner(env, src, (*self.functions).clone(), SubstMode::DollarParen);
            unsafe { libc::_exit(status) };
        }
        unsafe {
            libc::close(child_fd);
        }
        let path = format!("/dev/fd/{}", parent_fd);
        env.set_last_async_pid(pid);
        Ok(ProcSubstHandle {
            path,
            pid,
            fd: parent_fd,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubstMode {
    DollarParen,
    Backquote,
}

impl ExecRunner<'_> {
    fn run_subst_with_mode(
        &mut self,
        env: &mut dyn Environment,
        src: &str,
        mode: SubstMode,
    ) -> Result<Vec<u8>, ExpandError> {
        let mut fds: [RawFd; 2] = [0, 0];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(ExpandError::Io("pipe failed".into()));
        }
        let read_fd = fds[0];
        let write_fd = fds[1];

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return Err(ExpandError::Io("fork failed".into()));
        }
        if pid == 0 {
            // Child
            unsafe {
                libc::close(read_fd);
                libc::dup2(write_fd, 1);
                libc::close(write_fd);
            }
            env.enter_subshell();
            if !env.option("inherit_errexit") && !env.option("posix") {
                env.set_option("errexit", false);
            }
            let status = run_inner(env, src, (*self.functions).clone(), mode);
            unsafe { libc::_exit(status) };
        }
        // Parent
        unsafe { libc::close(write_fd) };
        let mut reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
        let mut out = Vec::new();
        if reader.read_to_end(&mut out).is_err() {
            return Err(ExpandError::Io("read pipe failed".into()));
        }
        let mut status: libc::c_int = 0;
        unsafe {
            libc::waitpid(pid, &mut status, 0);
        }
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            128 + libc::WTERMSIG(status)
        } else {
            1
        };
        env.set_last_status(code);
        Ok(out)
    }
}

fn run_inner(env: &mut dyn Environment, src: &str, functions: FunctionMap, mode: SubstMode) -> i32 {
    let src = subst_parse_source(src);
    let parse_src = expand_aliases_for_parse(&src, env);
    let mut lex = Lexer::new(&parse_src);
    lex.set_extglob_patterns(env.option("extglob"));
    lex.set_posix_mode(env.option("posix"));
    let mut tokens = Vec::new();
    loop {
        match lex.next_token() {
            Some(tok) => {
                if tok.kind == TokenKind::End {
                    tokens.push(tok);
                    break;
                }
                tokens.push(tok);
            }
            None => break,
        }
    }
    let mut parser = Parser::new(tokens, &parse_src);
    match parser.parse() {
        Ok(mut ast) => {
            if let Some(base) = env.diagnostic_line() {
                if mode == SubstMode::Backquote {
                    offset_backquote_command_lines(&mut ast.root, base);
                } else if let Some(first_line) = first_command_line(&ast.root) {
                    offset_dollar_paren_command_lines(&mut ast.root, base, first_line);
                }
            }
            let mut state = ExecState {
                functions,
                function_traced: Default::default(),
                suppress_err_traps: matches!(mode, SubstMode::DollarParen | SubstMode::Backquote)
                    && !env.option("errtrace")
                    && !env.option("extdebug"),
                suppress_debug_traps: !env.option("functrace"),
            };
            let result = execute_with_state(&ast, env, &mut state);
            result.status
        }
        Err(_e) => 2,
    }
}

fn subst_parse_source(src: &str) -> String {
    let mut src = src.to_string();
    if !src.ends_with('\n') && has_odd_trailing_backslashes(&src) {
        src.push('\\');
    }
    src
}

fn has_odd_trailing_backslashes(src: &str) -> bool {
    let mut count = 0usize;
    for byte in src.as_bytes().iter().rev() {
        if *byte == b'\\' {
            count += 1;
        } else {
            break;
        }
    }
    count % 2 == 1
}

fn first_command_line(command: &Command) -> Option<u32> {
    let mut first = (command.line > 0).then_some(command.line);
    match &command.data {
        CommandData::For(c) => {
            first = min_line(first, (c.line > 0).then_some(c.line));
            first = min_line(first, first_command_line(&c.action));
        }
        CommandData::Case(c) => {
            first = min_line(first, (c.line > 0).then_some(c.line));
            for clause in &c.clauses {
                if let Some(action) = &clause.action {
                    first = min_line(first, first_command_line(action));
                }
            }
        }
        CommandData::While(c) => {
            first = min_line(first, first_command_line(&c.test));
            first = min_line(first, first_command_line(&c.action));
        }
        CommandData::Until(c) => {
            first = min_line(first, first_command_line(&c.test));
            first = min_line(first, first_command_line(&c.action));
        }
        CommandData::If(c) => {
            first = min_line(first, first_command_line(&c.test));
            first = min_line(first, first_command_line(&c.true_case));
            if let Some(false_case) = &c.false_case {
                first = min_line(first, first_command_line(false_case));
            }
        }
        CommandData::Connection(c) => {
            first = min_line(first, first_command_line(&c.first));
            first = min_line(first, first_command_line(&c.second));
        }
        CommandData::FunctionDef(c) => first = min_line(first, first_command_line(&c.command)),
        CommandData::Group(c) => first = min_line(first, first_command_line(&c.command)),
        CommandData::Select(c) => {
            first = min_line(first, (c.line > 0).then_some(c.line));
            first = min_line(first, first_command_line(&c.action));
        }
        CommandData::ArithFor(c) => first = min_line(first, first_command_line(&c.action)),
        CommandData::Subshell(c) => first = min_line(first, first_command_line(&c.command)),
        CommandData::Coproc(c) => first = min_line(first, first_command_line(&c.command)),
        CommandData::Simple(_) | CommandData::Arith(_) | CommandData::Cond(_) => {}
    }
    first
}

fn min_line(left: Option<u32>, right: Option<u32>) -> Option<u32> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(line), None) | (None, Some(line)) => Some(line),
        (None, None) => None,
    }
}

fn offset_dollar_paren_command_lines(command: &mut Command, close_line: u32, first_line: u32) {
    offset_dollar_paren_line(&mut command.line, close_line, first_line);
    match &mut command.data {
        CommandData::For(c) => {
            offset_dollar_paren_line(&mut c.line, close_line, first_line);
            offset_dollar_paren_command_lines(&mut c.action, close_line, first_line);
        }
        CommandData::Case(c) => {
            offset_dollar_paren_line(&mut c.line, close_line, first_line);
            for clause in &mut c.clauses {
                if let Some(action) = &mut clause.action {
                    offset_dollar_paren_command_lines(action, close_line, first_line);
                }
            }
        }
        CommandData::While(c) => {
            offset_dollar_paren_command_lines(&mut c.test, close_line, first_line);
            offset_dollar_paren_command_lines(&mut c.action, close_line, first_line);
        }
        CommandData::Until(c) => {
            offset_dollar_paren_command_lines(&mut c.test, close_line, first_line);
            offset_dollar_paren_command_lines(&mut c.action, close_line, first_line);
        }
        CommandData::If(c) => {
            offset_dollar_paren_command_lines(&mut c.test, close_line, first_line);
            offset_dollar_paren_command_lines(&mut c.true_case, close_line, first_line);
            if let Some(false_case) = &mut c.false_case {
                offset_dollar_paren_command_lines(false_case, close_line, first_line);
            }
        }
        CommandData::Connection(c) => {
            offset_dollar_paren_command_lines(&mut c.first, close_line, first_line);
            offset_dollar_paren_command_lines(&mut c.second, close_line, first_line);
        }
        CommandData::FunctionDef(c) => offset_dollar_paren_command_lines(
            std::sync::Arc::make_mut(&mut c.command),
            close_line,
            first_line,
        ),
        CommandData::Group(c) => {
            offset_dollar_paren_command_lines(&mut c.command, close_line, first_line)
        }
        CommandData::Select(c) => {
            offset_dollar_paren_line(&mut c.line, close_line, first_line);
            offset_dollar_paren_command_lines(&mut c.action, close_line, first_line);
        }
        CommandData::ArithFor(c) => {
            offset_dollar_paren_command_lines(&mut c.action, close_line, first_line)
        }
        CommandData::Subshell(c) => {
            offset_dollar_paren_command_lines(&mut c.command, close_line, first_line)
        }
        CommandData::Coproc(c) => {
            offset_dollar_paren_command_lines(&mut c.command, close_line, first_line)
        }
        CommandData::Simple(_) | CommandData::Arith(_) | CommandData::Cond(_) => {}
    }
}

fn offset_dollar_paren_line(line: &mut u32, close_line: u32, first_line: u32) {
    if *line == 0 {
        return;
    }
    *line = close_line.saturating_add(line.saturating_sub(first_line));
}

fn offset_backquote_command_lines(command: &mut Command, base: u32) {
    offset_backquote_line(&mut command.line, base);
    match &mut command.data {
        CommandData::For(c) => {
            offset_backquote_line(&mut c.line, base);
            offset_backquote_command_lines(&mut c.action, base);
        }
        CommandData::Case(c) => {
            offset_backquote_line(&mut c.line, base);
            for clause in &mut c.clauses {
                if let Some(action) = &mut clause.action {
                    offset_backquote_command_lines(action, base);
                }
            }
        }
        CommandData::While(c) => {
            offset_backquote_command_lines(&mut c.test, base);
            offset_backquote_command_lines(&mut c.action, base);
        }
        CommandData::Until(c) => {
            offset_backquote_command_lines(&mut c.test, base);
            offset_backquote_command_lines(&mut c.action, base);
        }
        CommandData::If(c) => {
            offset_backquote_command_lines(&mut c.test, base);
            offset_backquote_command_lines(&mut c.true_case, base);
            if let Some(false_case) = &mut c.false_case {
                offset_backquote_command_lines(false_case, base);
            }
        }
        CommandData::Connection(c) => {
            offset_backquote_command_lines(&mut c.first, base);
            offset_backquote_command_lines(&mut c.second, base);
        }
        CommandData::FunctionDef(c) => {
            offset_backquote_command_lines(std::sync::Arc::make_mut(&mut c.command), base)
        }
        CommandData::Group(c) => offset_backquote_command_lines(&mut c.command, base),
        CommandData::Select(c) => {
            offset_backquote_line(&mut c.line, base);
            offset_backquote_command_lines(&mut c.action, base);
        }
        CommandData::ArithFor(c) => offset_backquote_command_lines(&mut c.action, base),
        CommandData::Subshell(c) => offset_backquote_command_lines(&mut c.command, base),
        CommandData::Coproc(c) => offset_backquote_command_lines(&mut c.command, base),
        CommandData::Simple(_) | CommandData::Arith(_) | CommandData::Cond(_) => {}
    }
}

fn offset_backquote_line(line: &mut u32, base: u32) {
    if *line == 0 {
        return;
    }
    *line = if *line == 1 {
        base
    } else {
        base.saturating_add(*line)
    };
}
