use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cherubsh_common::{
    Environment, FastHashMap as HashMap, CMD_IGNORE_RETURN, CMD_INVERT_RETURN, CMD_TIME_PIPELINE,
    CMD_TIME_POSIX, W_ASSIGNMENT,
};
use cherubsh_expander::{CommandRunner, ExpandError, ExpandFlags, ProcSubstHandle};
use cherubsh_lexer::{Lexer, TokenKind};
use cherubsh_parser::{
    Ast, Command, CommandData, CondCommand, CondType, Parser, Redirect, RedirectInstruction,
    Redirectee, Redirector, SimpleCommand, WordDesc, CONN_AMP, CONN_AND_AND, CONN_BAR_AND,
    CONN_NEWLINE, CONN_OR_OR, CONN_PIPE, CONN_SEMI,
};

mod controlflow;
mod coproc;
mod function;
mod pipeline;
mod redirect;
mod runner;
mod shell_ops;
mod simple;
mod subshell;
mod trap;
mod util;
mod xtrace;

pub use runner::ExecRunner;

pub use redirect::ExecError;

pub(crate) type FunctionMap = HashMap<String, Arc<Command>>;
pub(crate) type FunctionSourceMap = HashMap<String, String>;

#[derive(Default)]
pub struct ExecState {
    functions: FunctionMap,
    function_sources: FunctionSourceMap,
    function_traced: HashSet<String>,
    suppress_err_traps: bool,
    suppress_debug_traps: bool,
    function_depth: u32,
    source_depth: u32,
}

impl ExecState {
    pub fn import_exported_functions(&mut self, env: &dyn Environment) {
        for snap in env.iter_vars() {
            let Some(value) = snap.scalar.as_deref() else {
                continue;
            };
            let Some(name) = imported_function_name(&snap.name) else {
                continue;
            };
            if let Some(function) = parse_imported_function(name, value) {
                self.functions.insert(name.to_string(), Arc::new(function));
                self.function_sources
                    .insert(name.to_string(), "main".to_string());
            }
        }
    }

    pub fn expand_string(
        &mut self,
        text: &str,
        env: &mut dyn Environment,
    ) -> Result<String, ExpandError> {
        let mut runner = ExecRunner::with_functions_mut_at_depth(
            &mut self.functions,
            &mut self.function_sources,
            self.function_depth,
            self.source_depth,
        );
        cherubsh_expander::expand_string_to_string(text, env, &mut runner)
    }

    pub fn function_names(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }

    pub fn execute_source(
        &mut self,
        source: &str,
        env: &mut dyn Environment,
    ) -> Result<ExecResult, String> {
        let parse_source = cherubsh_common::expand_aliases_for_parse(source, env);
        let mut lexer = Lexer::new(&parse_source);
        lexer.set_extglob_patterns(env.option("extglob"));
        lexer.set_posix_mode(env.option("posix"));
        lexer.set_comments_enabled(true);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            let at_end = token.kind == TokenKind::End;
            tokens.push(token);
            if at_end {
                break;
            }
        }
        let mut parser = Parser::new(tokens, &parse_source);
        let ast = parser
            .parse_input_unit()
            .map_err(|error| error.message)?
            .ok_or_else(|| "empty input".to_string())?;
        Ok(execute_with_state(&ast, env, self))
    }

    pub fn capture_subshell(
        &mut self,
        source: &str,
        env: &mut dyn Environment,
    ) -> Result<Vec<u8>, ExpandError> {
        let mut runner = ExecRunner::with_functions_mut_at_depth(
            &mut self.functions,
            &mut self.function_sources,
            self.function_depth,
            self.source_depth,
        );
        runner.run_subst(env, source)
    }

    pub fn expand_words_source(
        &mut self,
        source: &str,
        env: &mut dyn Environment,
    ) -> Result<Vec<String>, ExpandError> {
        let parse_source = format!("__cherub_completion {source}");
        let mut lexer = Lexer::new(&parse_source);
        lexer.set_extglob_patterns(env.option("extglob"));
        lexer.set_posix_mode(env.option("posix"));
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token() {
            let at_end = token.kind == TokenKind::End;
            tokens.push(token);
            if at_end {
                break;
            }
        }
        let mut parser = Parser::new(tokens, &parse_source);
        let ast = parser
            .parse_input_unit()
            .map_err(|error| ExpandError::Other(error.message))?
            .ok_or_else(|| ExpandError::Other("empty completion word list".to_string()))?;
        let CommandData::Simple(simple) = ast.root.data else {
            return Err(ExpandError::Other(
                "completion word list is not a simple word list".to_string(),
            ));
        };
        let mut runner = ExecRunner::with_functions_mut_at_depth(
            &mut self.functions,
            &mut self.function_sources,
            self.function_depth,
            self.source_depth,
        );
        let words = cherubsh_expander::expand_word_list(
            simple.words.get(1..).unwrap_or_default(),
            env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::EXPAND_GLOB | ExpandFlags::QUOTE_REMOVAL,
        )?;
        Ok(words.into_iter().map(|word| word.text).collect())
    }
}

fn imported_function_name(env_name: &str) -> Option<&str> {
    env_name
        .strip_prefix("BASH_FUNC_")
        .and_then(|rest| rest.strip_suffix("%%"))
        .filter(|name| function_name_importable(name))
}

fn function_name_importable(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('=')
        && !name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

fn parse_imported_function(name: &str, value: &str) -> Option<Command> {
    if !value.starts_with("() {") {
        return None;
    }
    let source = format!("{name} {value}");
    let mut lex = Lexer::new(&source);
    let mut tokens = Vec::new();
    while let Some(tok) = lex.next_token() {
        let end = tok.kind == TokenKind::End;
        tokens.push(tok);
        if end {
            break;
        }
    }
    let mut parser = Parser::new(tokens, &source);
    let ast = parser.parse_input_unit().ok().flatten()?;
    let CommandData::FunctionDef(function) = ast.root.data else {
        return None;
    };
    (function.name.text == name).then(|| function.command.as_ref().clone())
}

pub struct ExecResult {
    pub status: i32,
    pub exit_shell: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecMode {
    Parent,
    Child,
}

#[derive(Clone, Debug)]
pub(crate) enum Unwind {
    Return { status: i32, trap_status: i32 },
    Break(u32),
    Continue(u32),
    AbortLine(i32),
    Exit(i32),
}

pub(crate) struct ExecContext<'a> {
    pub(crate) last_status: i32,
    pub(crate) functions: FunctionMap,
    pub(crate) function_sources: FunctionSourceMap,
    pub(crate) function_traced: HashSet<String>,
    pub(crate) suppress_err_traps: bool,
    pub(crate) suppress_debug_traps: bool,
    pub(crate) pending: Option<Unwind>,
    pub(crate) loop_depth: u32,
    pub(crate) function_depth: u32,
    pub(crate) function_call_stack: Vec<u64>,
    pub(crate) next_function_call_id: u64,
    pub(crate) posix_special_assignment_persisted: Vec<(String, u64)>,
    pub(crate) explicit_function_exports: Vec<(String, u64)>,
    pub(crate) function_prefix_assignment_stack: Vec<Vec<String>>,
    pub(crate) debug_trap_scopes: Vec<bool>,
    pub(crate) source_depth: u32,
    pub(crate) trap_base_function_depth: Option<u32>,
    pub(crate) errexit_suppressed: u32,
    pub(crate) abort_line_depth: u32,
    pub(crate) proc_subst: Vec<ProcSubstHandle>,
    pub(crate) funcnest_max: u32,
    pub(crate) reuse_current_subshell: bool,
    pub(crate) allow_child_external_exec: bool,
    pub(crate) stdin_redirect_depth: u32,
    pub(crate) env: &'a mut (dyn Environment + 'a),
}

pub fn execute_in(ast: &Ast, env: &mut dyn Environment) -> ExecResult {
    let mut state = ExecState::default();
    execute_with_state(ast, env, &mut state)
}

pub fn execute_with_state(
    ast: &Ast,
    env: &mut dyn Environment,
    state: &mut ExecState,
) -> ExecResult {
    let mut ctx = ExecContext::new(env);
    ctx.functions = std::mem::take(&mut state.functions);
    ctx.function_sources = std::mem::take(&mut state.function_sources);
    ctx.function_traced = std::mem::take(&mut state.function_traced);
    ctx.suppress_err_traps = state.suppress_err_traps;
    ctx.suppress_debug_traps = state.suppress_debug_traps;
    ctx.function_depth = state.function_depth;
    ctx.source_depth = state.source_depth;
    let status = ctx.execute_command(&ast.root, ExecMode::Parent);
    let (final_status, exit_shell) = match ctx.pending.take() {
        Some(Unwind::Exit(n)) => (n, true),
        _ => (status, false),
    };
    ctx.env.set_last_status(final_status);
    state.functions = ctx.functions;
    state.function_sources = ctx.function_sources;
    state.function_traced = ctx.function_traced;
    ExecResult {
        status: final_status,
        exit_shell,
    }
}

impl<'a> ExecContext<'a> {
    pub(crate) fn new(env: &'a mut (dyn Environment + 'a)) -> Self {
        let last = env.last_status();
        let funcnest_max = env
            .get("FUNCNEST")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        Self {
            last_status: last,
            functions: HashMap::default(),
            function_sources: HashMap::default(),
            function_traced: HashSet::new(),
            suppress_err_traps: false,
            suppress_debug_traps: false,
            pending: None,
            loop_depth: 0,
            function_depth: 0,
            function_call_stack: Vec::new(),
            next_function_call_id: 0,
            posix_special_assignment_persisted: Vec::new(),
            explicit_function_exports: Vec::new(),
            function_prefix_assignment_stack: Vec::new(),
            debug_trap_scopes: Vec::new(),
            source_depth: 0,
            trap_base_function_depth: None,
            errexit_suppressed: 0,
            abort_line_depth: 0,
            proc_subst: Vec::new(),
            funcnest_max,
            reuse_current_subshell: false,
            allow_child_external_exec: false,
            stdin_redirect_depth: 0,
            env,
        }
    }

    pub(crate) fn execute_child_command(&mut self, command: &Command) -> i32 {
        let saved = self.allow_child_external_exec;
        self.allow_child_external_exec = child_command_can_exec_direct(command);
        let status = self.execute_command(command, ExecMode::Child);
        self.allow_child_external_exec = saved;
        status
    }

    pub(crate) fn execute_command(&mut self, command: &Command, mode: ExecMode) -> i32 {
        self.execute_command_inner(command, mode, false)
    }

    pub(crate) fn execute_function_body_command(
        &mut self,
        command: &Command,
        mode: ExecMode,
    ) -> i32 {
        self.execute_command_inner(command, mode, true)
    }

    fn execute_command_inner(
        &mut self,
        command: &Command,
        mode: ExecMode,
        suppress_own_err_trap: bool,
    ) -> i32 {
        if self.pending.is_some() {
            return self.last_status;
        }
        if mode == ExecMode::Parent {
            crate::trap::run_pending_traps(self);
            if self.pending.is_some() {
                return self.last_status;
            }
        }
        if self.env.running_trap().is_none() {
            self.env.set_current_command(Some(command_label(command)));
        }
        let proc_subst_mark = self.proc_subst.len();
        let pushed_line = command.line > 0;
        if pushed_line {
            self.env.push_diagnostic_line(command.line);
        }
        let time_start = (command.flags & CMD_TIME_PIPELINE != 0).then(Instant::now);
        let suppress_errexit = command.flags & CMD_INVERT_RETURN != 0;
        if suppress_errexit {
            self.errexit_suppressed += 1;
        }
        let redirects_stdin = command_redirects_stdin(command);
        if redirects_stdin {
            self.stdin_redirect_depth += 1;
        }
        let raw = match mode {
            ExecMode::Parent => {
                if command.redirects.is_empty() {
                    self.dispatch(command, mode)
                } else {
                    let adjusted_line = redirection_error_report_line(command, self.env);
                    if let Some(line) = adjusted_line {
                        self.env.push_diagnostic_line(line);
                    }
                    let redirects = redirect::apply_redirects_to_parent(self, &command.redirects);
                    if adjusted_line.is_some() {
                        self.env.pop_diagnostic_line();
                    }
                    match redirects {
                        Ok(_guard) => self.dispatch(command, mode),
                        Err(err) => {
                            err.report_with_env(self.env);
                            if mode == ExecMode::Parent && redirection_error_runs_err_trap(command)
                            {
                                crate::trap::run_err_trap(self);
                            }
                            if self.env.option("posix")
                                && redirection_error_exits_for_command(command)
                            {
                                self.pending = Some(Unwind::Exit(1));
                            }
                            self.last_status = 1;
                            1
                        }
                    }
                }
            }
            ExecMode::Child => {
                if let Err(err) = redirect::apply_redirects_to_child(self, &command.redirects) {
                    err.report_with_env(self.env);
                    self.last_status = 1;
                    1
                } else {
                    self.dispatch(command, mode)
                }
            }
        };
        if redirects_stdin {
            self.stdin_redirect_depth -= 1;
        }
        if suppress_errexit {
            self.errexit_suppressed -= 1;
        }
        if let Some(start) = time_start {
            report_pipeline_time(
                self.env,
                start.elapsed(),
                command.flags & CMD_TIME_POSIX != 0,
            );
        }
        let inverted = if command.flags & CMD_INVERT_RETURN != 0
            && !matches!(self.pending, Some(Unwind::AbortLine(_)))
        {
            if raw == 0 {
                1
            } else {
                0
            }
        } else {
            raw
        };
        self.last_status = inverted;
        self.env.set_last_status(inverted);
        if !is_pipeline_command(command) {
            self.env.set_array("PIPESTATUS", vec![inverted.to_string()]);
        }
        if !suppress_own_err_trap {
            self.maybe_run_err_trap(command, mode, inverted);
        }
        crate::trap::run_pending_traps(self);
        self.maybe_errexit(command);
        self.close_proc_subst_since(proc_subst_mark);
        if pushed_line {
            self.env.pop_diagnostic_line();
        }
        inverted
    }

    pub(crate) fn register_proc_subst(&mut self, handles: Vec<ProcSubstHandle>) {
        self.proc_subst.extend(handles);
    }

    pub(crate) fn mark_posix_special_assignment_persisted(&mut self, name: &str) {
        for call_id in &self.function_call_stack {
            if !self.posix_special_assignment_affects_call(name, *call_id) {
                self.posix_special_assignment_persisted
                    .push((name.to_string(), *call_id));
            }
        }
    }

    pub(crate) fn posix_special_assignment_affects_call(&self, name: &str, call_id: u64) -> bool {
        self.posix_special_assignment_persisted
            .iter()
            .any(|(stored_name, stored_call_id)| stored_name == name && *stored_call_id == call_id)
    }

    pub(crate) fn clear_posix_special_assignments_for_call(&mut self, call_id: u64) {
        self.posix_special_assignment_persisted
            .retain(|(_, affected_call_id)| *affected_call_id != call_id);
    }

    pub(crate) fn reuse_current_subshell_for_next_dispatch(&mut self) {
        self.reuse_current_subshell = true;
    }

    pub(crate) fn with_abort_line_boundary<F>(&mut self, f: F) -> i32
    where
        F: FnOnce(&mut Self) -> i32,
    {
        self.abort_line_depth += 1;
        let status = f(self);
        self.abort_line_depth -= 1;
        status
    }

    fn close_proc_subst_since(&mut self, mark: usize) {
        for handle in self.proc_subst.drain(mark..).rev() {
            if handle.fd >= 0 {
                unsafe {
                    libc::close(handle.fd);
                }
            }
        }
    }

    fn maybe_errexit(&mut self, command: &Command) {
        if self.errexit_suppressed > 0 {
            return;
        }
        if self.pending.is_some() {
            return;
        }
        if !self.env.option("errexit") {
            return;
        }
        if command.flags & CMD_INVERT_RETURN != 0 {
            return;
        }
        if matches!(
            &command.data,
            CommandData::If(_)
                | CommandData::While(_)
                | CommandData::Until(_)
                | CommandData::FunctionDef(_)
        ) {
            return;
        }
        if let CommandData::Connection(conn) = &command.data {
            if matches!(
                conn.connector,
                CONN_AND_AND | CONN_OR_OR | CONN_SEMI | CONN_NEWLINE | CONN_AMP
            ) {
                return;
            }
        }
        if self.last_status != 0 {
            self.pending = Some(Unwind::Exit(self.last_status));
        }
    }

    fn maybe_run_err_trap(&mut self, command: &Command, mode: ExecMode, status: i32) {
        if status == 0 || mode != ExecMode::Parent || self.suppress_err_traps {
            return;
        }
        if self.errexit_suppressed > 0 {
            return;
        }
        if command.flags & (CMD_IGNORE_RETURN | CMD_INVERT_RETURN) != 0 {
            return;
        }
        match &command.data {
            CommandData::Subshell(_) => {}
            CommandData::Arith(_) | CommandData::Cond(_) => {}
            CommandData::For(_)
            | CommandData::While(_)
            | CommandData::Until(_)
            | CommandData::ArithFor(_)
            | CommandData::Select(_)
            | CommandData::Case(_)
            | CommandData::Group(_) => {}
            CommandData::Connection(conn) if matches!(conn.connector, CONN_PIPE | CONN_BAR_AND) => {
            }
            _ => return,
        }
        if self.function_depth > 0 && !self.env.option("errtrace") && !self.env.option("extdebug") {
            return;
        }
        crate::trap::run_err_trap(self);
    }

    pub(crate) fn dispatch(&mut self, command: &Command, mode: ExecMode) -> i32 {
        match &command.data {
            CommandData::Simple(c) => simple::execute(self, c, command.flags, mode),
            CommandData::Connection(c) => controlflow::execute_connection(self, c, mode),
            CommandData::Subshell(c) => subshell::execute(self, c),
            CommandData::Group(c) => self
                .with_abort_line_boundary(|ctx| ctx.execute_command(&c.command, ExecMode::Parent)),
            CommandData::FunctionDef(c) => function::define(self, c),
            CommandData::If(c) => self
                .with_abort_line_boundary(|ctx| controlflow::execute_if(ctx, c, ExecMode::Parent)),
            CommandData::While(c) => self.with_abort_line_boundary(|ctx| {
                controlflow::execute_while_or_until(
                    ctx,
                    &c.test,
                    &c.action,
                    ExecMode::Parent,
                    false,
                )
            }),
            CommandData::Until(c) => self.with_abort_line_boundary(|ctx| {
                controlflow::execute_while_or_until(ctx, &c.test, &c.action, ExecMode::Parent, true)
            }),
            CommandData::For(c) => self
                .with_abort_line_boundary(|ctx| controlflow::execute_for(ctx, c, ExecMode::Parent)),
            CommandData::Case(c) => {
                self.run_debug_trap_for_command(mode);
                self.with_abort_line_boundary(|ctx| {
                    controlflow::execute_case(ctx, c, ExecMode::Parent)
                })
            }
            CommandData::Select(c) => {
                self.run_debug_trap_for_command(mode);
                self.with_abort_line_boundary(|ctx| {
                    controlflow::execute_select(ctx, c, ExecMode::Parent)
                })
            }
            CommandData::Arith(c) => {
                self.run_debug_trap_for_command(mode);
                let expr = c.expression.text.as_str();
                let trimmed = expr.trim();
                if trimmed.is_empty() {
                    return 1;
                }
                let mut runner = crate::runner::ExecRunner::with_functions_mut_at_depth(
                    &mut self.functions,
                    &mut self.function_sources,
                    self.function_depth,
                    self.source_depth,
                );
                let result = if self.env.option("xtrace") {
                    let expanded_result =
                        cherubsh_expander::expand_for_arith_with_text(expr, self.env, &mut runner);
                    match expanded_result {
                        Ok((expanded, value)) => {
                            xtrace::trace(self, &format!("(( {} ))", expanded));
                            Ok(value)
                        }
                        Err(err) => Err(err),
                    }
                } else {
                    cherubsh_expander::expand_for_arith(expr, self.env, &mut runner)
                };
                match result {
                    Ok(v) => {
                        if v == 0 {
                            1
                        } else {
                            0
                        }
                    }
                    Err(err) => {
                        report_arith_command_error(self.env, err, expr);
                        1
                    }
                }
            }
            CommandData::ArithFor(c) => {
                self.run_debug_trap_for_command(mode);
                self.with_abort_line_boundary(|ctx| {
                    controlflow::execute_arith_for(ctx, c, ExecMode::Parent)
                })
            }
            CommandData::Cond(c) => {
                self.run_debug_trap_for_command(mode);
                let mut runner = crate::runner::ExecRunner::with_functions_mut_at_depth(
                    &mut self.functions,
                    &mut self.function_sources,
                    self.function_depth,
                    self.source_depth,
                );
                let status = if self.env.option("xtrace") {
                    let mut traces = Vec::new();
                    let result = cherubsh_builtins::cond::evaluate_with_runner_and_tracer_result(
                        c,
                        self.env,
                        &mut runner,
                        Some(&mut |line| traces.push(line)),
                    );
                    for line in traces {
                        xtrace::trace(self, &line);
                    }
                    match result {
                        Ok(true) => 0,
                        Ok(false) => 1,
                        Err(msg) => {
                            cherubsh_builtins::cond::report_cond_error(self.env, &msg);
                            2
                        }
                    }
                } else {
                    cherubsh_builtins::cond::evaluate_with_runner(c, self.env, &mut runner)
                };
                if status == 2 && self.env.option("nounset") {
                    self.pending = Some(Unwind::Exit(1));
                    1
                } else {
                    status
                }
            }
            CommandData::Coproc(c) => coproc::execute(self, c),
        }
    }

    pub(crate) fn run_debug_trap_for_command(&mut self, mode: ExecMode) -> Option<i32> {
        let debug_trap_in_scope = mode == ExecMode::Parent
            && !self.suppress_debug_traps
            && ((self.function_depth == 0 && self.source_depth == 0)
                || self.env.option("functrace")
                || self.debug_trap_scopes.last().copied().unwrap_or(false));
        if !debug_trap_in_scope {
            return None;
        }
        crate::trap::run_debug_trap(self)
    }
}

pub(crate) fn command_redirects_stdin(command: &Command) -> bool {
    if command.redirects.iter().any(redirect_targets_stdin)
        || matches!(
            &command.data,
            CommandData::Simple(simple) if simple.redirects.iter().any(redirect_targets_stdin)
        )
    {
        return true;
    }
    matches!(
        &command.data,
        CommandData::Connection(connection)
            if matches!(connection.connector, CONN_PIPE | CONN_BAR_AND)
                && command_redirects_stdin(&connection.first)
    )
}

fn redirect_targets_stdin(redirect: &Redirect) -> bool {
    matches!(redirect.redirector, Redirector::Fd(libc::STDIN_FILENO))
}

fn redirection_error_runs_err_trap(command: &Command) -> bool {
    matches!(
        &command.data,
        CommandData::For(_)
            | CommandData::While(_)
            | CommandData::Until(_)
            | CommandData::ArithFor(_)
            | CommandData::Select(_)
            | CommandData::Case(_)
            | CommandData::Group(_)
            | CommandData::Subshell(_)
    )
}

fn redirection_error_report_line(
    command: &Command,
    env: &dyn cherubsh_common::Environment,
) -> Option<u32> {
    if env.subshell_level() == 0 {
        return None;
    }
    if matches!(
        &command.data,
        CommandData::For(_)
            | CommandData::While(_)
            | CommandData::Until(_)
            | CommandData::ArithFor(_)
    ) {
        env.diagnostic_line().map(|line| line.saturating_add(1))
    } else {
        None
    }
}

pub(crate) fn command_label(command: &Command) -> String {
    match &command.data {
        CommandData::Simple(simple) => render_simple_command(simple, &command.redirects),
        CommandData::For(for_cmd) => {
            let mut line = format!("for {}", raw_word(&for_cmd.name));
            if let Some(words) = for_cmd.map_list.as_ref() {
                line.push_str(" in");
                for word in words {
                    line.push(' ');
                    line.push_str(&raw_word(word));
                }
            }
            line
        }
        CommandData::Arith(arith) => format!("(( {} ))", arith.expression.text.trim()),
        CommandData::Cond(cond) => format!("[[ {} ]]", render_cond_command(cond)),
        CommandData::Subshell(_) => "( ... )".to_string(),
        CommandData::Group(_) => "{ ...; }".to_string(),
        CommandData::FunctionDef(function) => format!("{} ()", raw_word(&function.name)),
        CommandData::Connection(conn) => {
            let op = match conn.connector {
                CONN_AND_AND => "&&",
                CONN_OR_OR => "||",
                CONN_PIPE => "|",
                CONN_BAR_AND => "|&",
                CONN_SEMI | CONN_NEWLINE => ";",
                CONN_AMP => "&",
                _ => "",
            };
            if op.is_empty() {
                String::new()
            } else {
                format!(
                    "{} {} {}",
                    command_label(&conn.first),
                    op,
                    command_label(&conn.second)
                )
            }
        }
        CommandData::If(_) => "if".to_string(),
        CommandData::While(_) => "while".to_string(),
        CommandData::Until(_) => "until".to_string(),
        CommandData::Case(case_cmd) => format!("case {}", raw_word(&case_cmd.word)),
        CommandData::Select(select_cmd) => format!("select {}", raw_word(&select_cmd.name)),
        CommandData::ArithFor(_) => "for (( ... ))".to_string(),
        CommandData::Coproc(coproc) => coproc
            .name
            .as_ref()
            .map(|name| format!("coproc {}", raw_word(name)))
            .unwrap_or_else(|| "coproc".to_string()),
    }
}

fn render_simple_command(simple: &SimpleCommand, trailing_redirects: &[Redirect]) -> String {
    let mut parts = simple.words.iter().map(raw_word).collect::<Vec<_>>();
    parts.extend(simple.redirects.iter().map(render_redirect));
    parts.extend(trailing_redirects.iter().map(render_redirect));
    parts.join(" ")
}

pub(crate) fn simple_command_label(simple: &SimpleCommand) -> String {
    render_simple_command(simple, &[])
}

fn render_cond_command(cmd: &CondCommand) -> String {
    match cmd.cond_type {
        CondType::And => format!(
            "{} && {}",
            render_cond_command(cmd.left.as_deref().unwrap()),
            render_cond_command(cmd.right.as_deref().unwrap())
        ),
        CondType::Or => format!(
            "{} || {}",
            render_cond_command(cmd.left.as_deref().unwrap()),
            render_cond_command(cmd.right.as_deref().unwrap())
        ),
        CondType::Unary => {
            let op = cmd.op.as_ref().map(raw_word).unwrap_or_default();
            let arg = cmd
                .left
                .as_deref()
                .and_then(|left| left.term.as_ref())
                .map(raw_word)
                .unwrap_or_default();
            format!("{op} {arg}")
        }
        CondType::Binary => {
            let lhs = cmd
                .left
                .as_deref()
                .and_then(|left| left.term.as_ref())
                .map(raw_word)
                .unwrap_or_default();
            let op = cmd.op.as_ref().map(raw_word).unwrap_or_default();
            let rhs = cmd
                .right
                .as_deref()
                .and_then(|right| right.term.as_ref())
                .map(raw_word)
                .unwrap_or_default();
            format!("{lhs} {op} {rhs}")
        }
        CondType::Term => {
            if let Some(inner) = cmd.left.as_deref() {
                format!("! {}", render_cond_command(inner))
            } else {
                cmd.term.as_ref().map(raw_word).unwrap_or_default()
            }
        }
        CondType::Expr => format!("( {} )", render_cond_command(cmd.left.as_deref().unwrap())),
    }
}

fn raw_word(word: &WordDesc) -> String {
    word.raw.clone().unwrap_or_else(|| word.text.clone())
}

fn render_redirect(redir: &Redirect) -> String {
    let op = match redir.instruction {
        RedirectInstruction::OutputDirection => ">",
        RedirectInstruction::InputDirection | RedirectInstruction::InputaDirection => "<",
        RedirectInstruction::AppendingTo => ">>",
        RedirectInstruction::ReadingUntil => "<<",
        RedirectInstruction::ReadingString => "<<<",
        RedirectInstruction::DuplicatingInput
        | RedirectInstruction::DuplicatingInputWord
        | RedirectInstruction::MoveInput
        | RedirectInstruction::MoveInputWord => "<&",
        RedirectInstruction::DuplicatingOutput
        | RedirectInstruction::DuplicatingOutputWord
        | RedirectInstruction::MoveOutput
        | RedirectInstruction::MoveOutputWord => ">&",
        RedirectInstruction::DeblankReadingUntil => "<<-",
        RedirectInstruction::CloseThis => match redir.redirector {
            Redirector::Fd(0) => "<&",
            _ => ">&",
        },
        RedirectInstruction::ErrAndOut => "&>",
        RedirectInstruction::InputOutput => "<>",
        RedirectInstruction::OutputForce => ">|",
        RedirectInstruction::AppendErrAndOut => "&>>",
    };
    let prefix = match &redir.redirector {
        Redirector::Fd(fd) if *fd == default_redirect_fd_for_instruction(&redir.instruction) => {
            String::new()
        }
        Redirector::Fd(fd) => fd.to_string(),
        Redirector::Var(name) => format!("{{{name}}}"),
    };
    let target = match &redir.redirectee {
        Redirectee::Fd(fd) if *fd < 0 => "-".to_string(),
        Redirectee::Fd(fd) => fd.to_string(),
        Redirectee::Word(word) => raw_word(word),
    };
    format!("{prefix}{op} {target}")
}

fn default_redirect_fd_for_instruction(instruction: &RedirectInstruction) -> i32 {
    match instruction {
        RedirectInstruction::InputDirection
        | RedirectInstruction::InputaDirection
        | RedirectInstruction::ReadingUntil
        | RedirectInstruction::ReadingString
        | RedirectInstruction::DuplicatingInput
        | RedirectInstruction::DuplicatingInputWord
        | RedirectInstruction::DeblankReadingUntil
        | RedirectInstruction::InputOutput
        | RedirectInstruction::MoveInput
        | RedirectInstruction::MoveInputWord => 0,
        _ => 1,
    }
}

fn is_pipeline_command(command: &Command) -> bool {
    matches!(
        &command.data,
        CommandData::Connection(conn) if matches!(conn.connector, CONN_PIPE | CONN_BAR_AND)
    )
}

fn redirection_error_exits_for_command(command: &Command) -> bool {
    let CommandData::Simple(simple) = &command.data else {
        return false;
    };
    let Some(word) = simple
        .words
        .iter()
        .find(|word| word.flags & W_ASSIGNMENT == 0)
    else {
        return false;
    };
    cherubsh_builtins::is_special(&word.text)
}

fn child_command_can_exec_direct(command: &Command) -> bool {
    matches!(command.data, CommandData::Simple(_))
        && command.flags & (CMD_INVERT_RETURN | CMD_TIME_PIPELINE) == 0
}

fn report_pipeline_time(env: &mut dyn Environment, elapsed: Duration, posix: bool) {
    let format = if posix {
        "real %2R\nuser %2U\nsys %2S".to_string()
    } else {
        env.get("TIMEFORMAT")
            .unwrap_or_else(|| "\nreal\t%3lR\nuser\t%3lU\nsys\t%3lS".to_string())
    };
    if format.is_empty() {
        return;
    }
    match render_time_format(&format, elapsed) {
        Ok(rendered) => eprintln!("{rendered}"),
        Err(ch) => eprintln!("cherubsh: TIMEFORMAT: `{ch}': invalid format character"),
    }
}

fn render_time_format(format: &str, elapsed: Duration) -> Result<String, char> {
    let mut rendered = String::with_capacity(format.len() + 32);
    let mut chars = format.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' || chars.peek().is_none() {
            rendered.push(ch);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            rendered.push('%');
            continue;
        }
        if chars.peek() == Some(&'P') {
            chars.next();
            rendered.push_str("0.00");
            continue;
        }

        let mut precision = 3usize;
        if chars.peek().is_some_and(char::is_ascii_digit) {
            precision = chars.next().unwrap().to_digit(10).unwrap() as usize;
            precision = precision.min(6);
        }
        let long = if chars.peek() == Some(&'l') {
            chars.next();
            true
        } else {
            false
        };
        let kind = chars.next().ok_or('%')?;
        let seconds = match kind {
            'R' | 'E' => elapsed.as_secs_f64(),
            'U' | 'S' => 0.0,
            other => return Err(other),
        };
        rendered.push_str(&format_time_value(seconds, precision, long));
    }
    Ok(rendered)
}

fn format_time_value(seconds: f64, precision: usize, long: bool) -> String {
    if long {
        let minutes = (seconds / 60.0).floor() as u64;
        let remainder = seconds - minutes as f64 * 60.0;
        format!("{minutes}m{remainder:.precision$}s")
    } else {
        format!("{seconds:.precision$}")
    }
}

pub(crate) fn report_arith_command_error(env: &dyn Environment, err: ExpandError, expr: &str) {
    if err.already_reported() {
        return;
    }
    let message = err.into_shell_error(None).message;
    let message = if arith_command_error_is_for_outer_expr(expr, &message)
        || message.starts_with('`') && message.contains("not a valid identifier")
    {
        format!("((: {message}")
    } else {
        message
    };
    let message = normalize_quoted_assoc_arith_error(message);
    let message = if expr.trim() == "--" {
        message
            .replace("((: --:", "((: -- :")
            .replace("(error token is \"-\")", "(error token is \"- \")")
    } else {
        message
    };
    let message = normalize_bash53_arith_error(expr, message);
    let repeat = if expr.contains("[$") && message.ends_with(": bad array subscript") {
        2
    } else {
        1
    };
    match (env.diagnostic_source_name(), env.diagnostic_line()) {
        (Some(source), Some(line)) => {
            for _ in 0..repeat {
                eprintln!("{source}: line {line}: {message}");
            }
        }
        _ => {
            for _ in 0..repeat {
                eprintln!("cherubsh: {message}");
            }
        }
    }
}

fn normalize_bash53_arith_error(expr: &str, message: String) -> String {
    let trimmed = expr.trim();
    if expr.trim_end().len() != expr.len()
        && message.contains("attempted assignment to non-variable")
    {
        return message
            .replace(&format!("((: {trimmed}:"), &format!("((: {trimmed} :"))
            .replace(
                &format!("(error token is \"{}\")", assignment_error_token(trimmed)),
                &format!("(error token is \"{} \")", assignment_error_token(trimmed)),
            );
    }
    if trimmed.ends_with("++") || trimmed.ends_with("--") {
        let mut normalized =
            message.replace(&format!("((: {trimmed}:"), &format!("((: {trimmed} :"));
        if !normalized.contains("arithmetic syntax error: operand expected") {
            normalized = normalized.replace(
                "syntax error: operand expected",
                "arithmetic syntax error: operand expected",
            );
        }
        return normalized
            .replace("(error token is \"+\")", "(error token is \"+ \")")
            .replace("(error token is \"-\")", "(error token is \"- \")");
    }
    if trimmed.ends_with('=') {
        let mut normalized = message;
        if !normalized.contains("arithmetic syntax error: operand expected") {
            normalized = normalized.replace(
                "syntax error: operand expected",
                "arithmetic syntax error: operand expected",
            );
        }
        return normalized.replace("(error token is \"\")", "(error token is \"=\")");
    }
    message
}

fn assignment_error_token(expr: &str) -> &str {
    expr.find('=').map(|idx| &expr[idx..]).unwrap_or("")
}

fn normalize_quoted_assoc_arith_error(message: String) -> String {
    if !message.contains("((: 'assoc[") {
        return message;
    }
    message
        .replace("\\[$(", "\\[\\$(")
        .replace("\\\\]", "\\]")
        .replace("\\\\[", "\\[")
}

fn arith_command_error_is_for_outer_expr(expr: &str, message: &str) -> bool {
    let expr = expr.trim_start();
    !expr.is_empty()
        && (message.starts_with(expr)
            || message.starts_with(expr.trim_end())
            || (expr.contains('"') && message.contains("arithmetic syntax error"))
            || (expr.starts_with('\'') && message.starts_with('\''))
            || (expr.starts_with('"') && message.starts_with('"')))
}

#[cfg(test)]
mod tests {
    use super::render_time_format;
    use std::time::Duration;

    #[test]
    fn time_format_supports_precision_and_long_output() {
        let elapsed = Duration::from_millis(1_234);
        assert_eq!(
            render_time_format("real %2R; long %3lR; %% %P", elapsed),
            Ok("real 1.23; long 0m1.234s; % 0.00".to_string())
        );
    }

    #[test]
    fn time_format_rejects_unknown_conversions() {
        assert_eq!(render_time_format("%Q", Duration::ZERO), Err('Q'));
    }
}
