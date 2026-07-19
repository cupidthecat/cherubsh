//! `CommandRunner` implementation. Bridges the expander back into exec so
//! `$(cmd)`, backticks, and `<(cmd)` can fork+exec sub-shells.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_common::signals::TrapKind;
use cherubsh_common::{expand_aliases_for_parse, Environment, ProcSubstDir};
use cherubsh_expander::{CommandRunner, CurrentSubstMode, ExpandError, ProcSubstHandle};
use cherubsh_lexer::{Lexer, TokenKind};
use cherubsh_parser::{Command, CommandData, Parser};

use crate::{execute_with_state, ExecContext, ExecState, FunctionMap, FunctionSourceMap, Unwind};

pub struct ExecRunner<'a> {
    functions: FunctionAccess<'a>,
    function_depth: u32,
    source_depth: u32,
}

enum FunctionAccess<'a> {
    Shared(&'a FunctionMap, &'a FunctionSourceMap),
    Mutable(&'a mut FunctionMap, &'a mut FunctionSourceMap),
}

impl FunctionAccess<'_> {
    fn clone_maps(&self) -> (FunctionMap, FunctionSourceMap) {
        match self {
            FunctionAccess::Shared(functions, sources) => {
                ((**functions).clone(), (**sources).clone())
            }
            FunctionAccess::Mutable(functions, sources) => {
                ((**functions).clone(), (**sources).clone())
            }
        }
    }

    fn replace_maps(&mut self, functions: FunctionMap, sources: FunctionSourceMap) {
        if let FunctionAccess::Mutable(target_functions, target_sources) = self {
            **target_functions = functions;
            **target_sources = sources;
        }
    }
}

impl<'a> ExecRunner<'a> {
    pub(crate) fn with_functions_mut_at_depth(
        functions: &'a mut FunctionMap,
        function_sources: &'a mut FunctionSourceMap,
        function_depth: u32,
        source_depth: u32,
    ) -> Self {
        Self {
            functions: FunctionAccess::Mutable(functions, function_sources),
            function_depth,
            source_depth,
        }
    }
}

impl Default for ExecRunner<'static> {
    fn default() -> Self {
        static EMPTY_FUNCTIONS: OnceLock<FunctionMap> = OnceLock::new();
        static EMPTY_FUNCTION_SOURCES: OnceLock<FunctionSourceMap> = OnceLock::new();
        Self {
            functions: FunctionAccess::Shared(
                EMPTY_FUNCTIONS.get_or_init(Default::default),
                EMPTY_FUNCTION_SOURCES.get_or_init(Default::default),
            ),
            function_depth: 0,
            source_depth: 0,
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

    fn run_current_subst(
        &mut self,
        env: &mut dyn Environment,
        src: &str,
        mode: CurrentSubstMode,
    ) -> Result<(Vec<u8>, i32), ExpandError> {
        self.run_current_subst_with_mode(env, src, mode)
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
        let (mut parent_fd, child_fd, child_target) = match dir {
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
            env.enter_command_substitution();
            env.suppress_inherited_exit_trap();
            clear_inherited_signal_traps_for_proc_subst(env);
            let (functions, function_sources) = self.functions.clone_maps();
            let mut status = run_inner(
                env,
                src,
                functions,
                function_sources,
                SubstMode::DollarParen,
                self.function_depth,
                self.source_depth,
            );
            env.set_last_status(status);
            if let Some(trap_status) = env.run_exit_trap_hook() {
                status = trap_status;
            }
            unsafe { libc::_exit(status) };
        }
        unsafe {
            libc::close(child_fd);
        }
        parent_fd = move_proc_subst_parent_fd(parent_fd);
        let path = format!("/dev/fd/{}", parent_fd);
        env.set_last_async_pid(pid);
        Ok(ProcSubstHandle {
            path,
            pid,
            fd: parent_fd,
        })
    }
}

fn move_proc_subst_parent_fd(fd: RawFd) -> RawFd {
    if fd >= 63 {
        return fd;
    }
    let moved = unsafe { libc::fcntl(fd, libc::F_DUPFD, 63) };
    if moved >= 0 {
        unsafe {
            libc::close(fd);
        }
        moved
    } else {
        fd
    }
}

fn clear_inherited_signal_traps_for_proc_subst(env: &mut dyn Environment) {
    let signals = env
        .trap_all()
        .into_iter()
        .filter_map(|(kind, _)| match kind {
            TrapKind::Numeric(_) => Some(kind),
            _ => None,
        })
        .collect::<Vec<_>>();
    for kind in signals {
        env.trap_clear(kind);
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
            env.enter_command_substitution();
            env.suppress_inherited_exit_trap();
            if !env.option("inherit_errexit") && !env.option("posix") {
                env.set_option("errexit", false);
            }
            let (functions, function_sources) = self.functions.clone_maps();
            let mut status = run_inner(
                env,
                src,
                functions,
                function_sources,
                mode,
                self.function_depth,
                self.source_depth,
            );
            env.set_last_status(status);
            if let Some(trap_status) = env.run_exit_trap_hook() {
                status = trap_status;
            }
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

    fn run_current_subst_with_mode(
        &mut self,
        env: &mut dyn Environment,
        src: &str,
        mode: CurrentSubstMode,
    ) -> Result<(Vec<u8>, i32), ExpandError> {
        let saved_reply = if mode == CurrentSubstMode::Reply {
            env.get("REPLY")
        } else {
            None
        };
        let (mut functions, mut function_sources) = self.functions.clone_maps();
        let result = if mode == CurrentSubstMode::Output {
            run_current_with_stdout_capture(env, src, &mut functions, &mut function_sources)
        } else {
            run_current_inner(env, src, &mut functions, &mut function_sources, true)
        };
        self.functions.replace_maps(functions, function_sources);
        match result {
            Ok(CurrentSubstResult {
                bytes,
                status,
                exit_shell: false,
            }) => {
                if mode == CurrentSubstMode::Reply {
                    match saved_reply {
                        Some(value) => env.set("REPLY", value),
                        None => env.unset("REPLY"),
                    }
                }
                env.set_last_status(status);
                Ok((bytes, status))
            }
            Ok(CurrentSubstResult {
                status,
                exit_shell: true,
                ..
            }) => {
                env.set_last_status(status);
                Err(ExpandError::ExitShell(status))
            }
            Err(err) => Err(err),
        }
    }
}

struct CurrentSubstResult {
    bytes: Vec<u8>,
    status: i32,
    exit_shell: bool,
}

fn run_current_with_stdout_capture(
    env: &mut dyn Environment,
    src: &str,
    functions: &mut FunctionMap,
    function_sources: &mut FunctionSourceMap,
) -> Result<CurrentSubstResult, ExpandError> {
    std::io::stdout().flush().ok();
    let mut file = create_capture_file()?;
    let saved_stdout = unsafe { libc::dup(1) };
    if saved_stdout < 0 {
        return Err(ExpandError::Io("dup stdout failed".into()));
    }
    if unsafe { libc::dup2(file.as_raw_fd(), 1) } < 0 {
        unsafe {
            libc::close(saved_stdout);
        }
        return Err(ExpandError::Io("redirect stdout failed".into()));
    }
    let result = run_current_inner(env, src, functions, function_sources, false);
    std::io::stdout().flush().ok();
    unsafe {
        libc::dup2(saved_stdout, 1);
        libc::close(saved_stdout);
    }
    let mut result = result?;
    file.seek(SeekFrom::Start(0))?;
    file.read_to_end(&mut result.bytes)?;
    Ok(result)
}

fn create_capture_file() -> Result<std::fs::File, ExpandError> {
    let pid = unsafe { libc::getpid() };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..128u32 {
        let mut path = std::env::temp_dir();
        path.push(format!("cherubsh-current-subst-{pid}-{nanos}-{attempt}"));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                let _ = std::fs::remove_file(path);
                return Ok(file);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(ExpandError::Io(err.to_string())),
        }
    }
    Err(ExpandError::Io("create temp file failed".into()))
}

fn run_inner(
    env: &mut dyn Environment,
    src: &str,
    functions: FunctionMap,
    function_sources: FunctionSourceMap,
    mode: SubstMode,
    function_depth: u32,
    source_depth: u32,
) -> i32 {
    let src = subst_parse_source(src);
    let parse_src = expand_aliases_for_parse(&src, env);
    let mut lex = Lexer::new(&parse_src);
    lex.set_extglob_patterns(env.option("extglob"));
    lex.set_posix_mode(env.option("posix"));
    let mut tokens = Vec::new();
    while let Some(tok) = lex.next_token() {
        if tok.kind == TokenKind::End {
            tokens.push(tok);
            break;
        }
        tokens.push(tok);
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
                function_sources,
                function_traced: Default::default(),
                suppress_err_traps: matches!(mode, SubstMode::DollarParen | SubstMode::Backquote)
                    && !env.option("errtrace")
                    && !env.option("extdebug"),
                suppress_debug_traps: !env.option("functrace"),
                function_depth,
                source_depth,
            };
            let result = execute_with_state(&ast, env, &mut state);
            result.status
        }
        Err(_e) => 2,
    }
}

fn run_current_inner(
    env: &mut dyn Environment,
    src: &str,
    functions: &mut FunctionMap,
    function_sources: &mut FunctionSourceMap,
    collect_reply: bool,
) -> Result<CurrentSubstResult, ExpandError> {
    let src = subst_parse_source(src);
    let parse_src = expand_aliases_for_parse(&src, env);
    let mut lex = Lexer::new(&parse_src);
    lex.set_extglob_patterns(env.option("extglob"));
    lex.set_posix_mode(env.option("posix"));
    let mut tokens = Vec::new();
    while let Some(tok) = lex.next_token() {
        if tok.kind == TokenKind::End {
            tokens.push(tok);
            break;
        }
        tokens.push(tok);
    }
    let mut parser = Parser::new(tokens, &parse_src);
    match parser.parse() {
        Ok(mut ast) => {
            if let Some(base) = env.diagnostic_line() {
                if let Some(first_line) = first_command_line(&ast.root) {
                    let anchor = if src.contains('\n') {
                        base.saturating_add(1)
                    } else {
                        base
                    };
                    offset_dollar_paren_command_lines(&mut ast.root, anchor, first_line);
                }
            }
            let mut ctx = ExecContext::new(env);
            ctx.functions = std::mem::take(functions);
            ctx.function_sources = std::mem::take(function_sources);
            ctx.function_depth += 1;
            ctx.env.push_local_scope();
            let saved_errexit = ctx.env.option("errexit");
            if !ctx.env.option("posix") {
                ctx.env.set_option("errexit", false);
            }
            let status = ctx.execute_command(&ast.root, crate::ExecMode::Parent);
            let (status, exit_shell) = match ctx.pending.take() {
                Some(Unwind::Return { status, .. }) => (status, false),
                Some(Unwind::Exit(status)) => (status, true),
                Some(Unwind::AbortLine(status)) => (status, false),
                Some(other) => {
                    ctx.pending = Some(other);
                    (status, false)
                }
                None => (status, false),
            };
            let bytes = if collect_reply {
                ctx.env.get("REPLY").unwrap_or_default().into_bytes()
            } else {
                Vec::new()
            };
            if !ctx.env.option("posix") {
                ctx.env.set_option("errexit", saved_errexit);
            }
            ctx.env.pop_local_scope();
            ctx.function_depth = ctx.function_depth.saturating_sub(1);
            *functions = ctx.functions;
            *function_sources = ctx.function_sources;
            Ok(CurrentSubstResult {
                bytes,
                status,
                exit_shell,
            })
        }
        Err(_) => Ok(CurrentSubstResult {
            bytes: Vec::new(),
            status: 2,
            exit_shell: false,
        }),
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
        base.saturating_add((*line).saturating_sub(1))
    };
}
