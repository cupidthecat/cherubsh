//! Adapter that exposes `ExecContext` to the builtins crate as a
//! `cherubsh_builtins::ShellOps` implementor.

use cherubsh_builtins::ShellOps;
use cherubsh_common::{expand_aliases_for_parse, Environment, Span, W_ASSIGNMENT, W_COMPASSIGN};
use cherubsh_expander::assignment::expand_assignment_word;
use cherubsh_lexer::{Lexer, TokenKind};
use cherubsh_parser::{Command, ParseError, Parser, WordDesc};

use crate::runner::ExecRunner;
use crate::util::{apply_assignment, report_expand_error, search_path};
use crate::{ExecContext, ExecMode, Unwind};

pub struct ExecAdapter<'a, 'b> {
    pub ctx: &'a mut ExecContext<'b>,
}

impl<'a, 'b> ShellOps for ExecAdapter<'a, 'b> {
    fn env(&mut self) -> &mut dyn Environment {
        &mut *self.ctx.env
    }

    fn env_ref(&self) -> &dyn Environment {
        &*self.ctx.env
    }

    fn function_define(&mut self, name: &str, body: Command) {
        self.ctx.functions.insert(name.to_string(), body);
    }

    fn function_get(&self, name: &str) -> Option<Command> {
        self.ctx.functions.get(name).cloned()
    }

    fn function_remove(&mut self, name: &str) -> bool {
        self.ctx.function_traced.remove(name);
        self.ctx.functions.remove(name).is_some()
    }

    fn function_names(&self) -> Vec<String> {
        self.ctx.functions.keys().cloned().collect()
    }

    fn function_set_trace(&mut self, name: &str, on: bool) {
        if on {
            self.ctx.function_traced.insert(name.to_string());
        } else {
            self.ctx.function_traced.remove(name);
        }
    }

    fn function_is_trace(&self, name: &str) -> bool {
        self.ctx.function_traced.contains(name)
    }

    fn set_debug_trap_scope_active(&mut self, on: bool) {
        if let Some(scope) = self.ctx.debug_trap_scopes.last_mut() {
            *scope = on;
        }
    }

    fn run_source(&mut self, src: &str) -> i32 {
        run_source_inner(self, src, false, None)
    }

    fn run_source_named(&mut self, src: &str, source_name: &str) -> i32 {
        run_source_inner(self, src, false, Some(source_name))
    }

    fn run_eval(&mut self, src: &str) -> i32 {
        run_source_inner(self, src, true, None)
    }

    fn run_command(&mut self, cmd: &Command) -> i32 {
        self.ctx.execute_command(cmd, ExecMode::Parent)
    }

    fn request_exit(&mut self, status: i32) {
        self.ctx.pending = Some(Unwind::Exit(status));
    }
    fn request_return(&mut self, status: i32) {
        self.ctx.pending = Some(Unwind::Return(status));
    }
    fn request_break(&mut self, levels: u32) {
        self.ctx.pending = Some(Unwind::Break(levels));
    }
    fn request_continue(&mut self, levels: u32) {
        self.ctx.pending = Some(Unwind::Continue(levels));
    }

    fn function_depth(&self) -> u32 {
        self.ctx.function_depth
    }
    fn source_depth(&self) -> u32 {
        self.ctx.source_depth
    }
    fn loop_depth(&self) -> u32 {
        self.ctx.loop_depth
    }

    fn evaluate_arith(&mut self, expr: &str) -> Result<i64, String> {
        let mut runner = ExecRunner::with_functions(&self.ctx.functions);
        if self.ctx.env.option("assoc_expand_once") {
            cherubsh_expander::arith::eval(
                expr,
                &mut cherubsh_expander::ExpCtx::new(self.ctx.env, &mut runner),
            )
            .map_err(|e| e.to_string())
        } else {
            cherubsh_expander::expand_for_arith(expr, self.ctx.env, &mut runner)
                .map_err(|e| e.to_string())
        }
    }

    fn apply_assignment_arg(&mut self, arg: &str, compound: bool) -> i32 {
        let word = WordDesc {
            text: arg.to_string(),
            flags: W_ASSIGNMENT | if compound { W_COMPASSIGN } else { 0 },
            span: Span::dummy(),
        };
        let mut runner = ExecRunner::with_functions(&self.ctx.functions);
        match expand_assignment_word(&word, self.ctx.env, &mut runner) {
            Ok(Some(assignment)) => apply_assignment(self.ctx.env, &assignment),
            Ok(None) => 1,
            Err(err) => {
                report_expand_error(self.ctx.env, err, Some(word.span));
                1
            }
        }
    }

    fn resolve_command(&mut self, name: &str) -> Option<std::path::PathBuf> {
        if let Some(p) = self.ctx.env.hash_get_with_hit(name) {
            return Some(p);
        }
        if let Some(p) = search_path(name, self.ctx.env) {
            self.ctx.env.hash_set(name, p.clone());
            return Some(p);
        }
        None
    }

    fn record_local(&mut self, name: &str, prior: Option<String>) -> bool {
        let Some(frame) = self.ctx.local_stack.last_mut() else {
            return false;
        };
        if !frame.contains_key(name) {
            frame.insert(name.to_string(), prior);
        }
        true
    }
    fn local_recorded(&self, name: &str) -> bool {
        self.ctx
            .local_stack
            .last()
            .map(|f| f.contains_key(name))
            .unwrap_or(false)
    }

    fn note_exported(&mut self, name: &str) {
        if let Some(call_id) = self.ctx.function_call_stack.last().copied() {
            self.ctx
                .explicit_function_exports
                .insert((name.to_string(), call_id));
        }
    }

    fn current_function_prefix_assignment(&self, name: &str) -> bool {
        self.ctx
            .function_prefix_assignment_stack
            .last()
            .is_some_and(|names| names.contains(name))
    }
}

fn run_source_inner(
    adapter: &mut ExecAdapter<'_, '_>,
    src: &str,
    offset_to_current_line: bool,
    source_name: Option<&str>,
) -> i32 {
    let parse_src = expand_aliases_for_parse(src, adapter.ctx.env);
    let mut lex = Lexer::new(&parse_src);
    lex.set_extglob_patterns(adapter.ctx.env.option("extglob"));
    lex.set_posix_mode(adapter.ctx.env.option("posix"));
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
    match parser.parse_input_unit() {
        Ok(Some(mut ast)) => {
            if offset_to_current_line {
                if let Some(line) = adapter.ctx.env.diagnostic_line() {
                    offset_command_lines(&mut ast.root, line.saturating_sub(1));
                }
            }
            let source_debug_scope = source_name.is_some() && adapter.ctx.env.option("functrace");
            adapter.ctx.source_depth += 1;
            if let Some(source_name) = source_name {
                adapter.ctx.env.source_frame_push(source_name);
            }
            if source_name.is_some() {
                adapter.ctx.debug_trap_scopes.push(source_debug_scope);
            }
            let saved_abort_line_depth = adapter.ctx.abort_line_depth;
            adapter.ctx.abort_line_depth = 0;
            let status = adapter.ctx.execute_command(&ast.root, ExecMode::Parent);
            if source_name.is_some() {
                adapter.ctx.env.source_frame_pop();
                adapter.ctx.debug_trap_scopes.pop();
            }
            adapter.ctx.source_depth -= 1;
            let pending = adapter.ctx.pending.take();
            adapter.ctx.abort_line_depth = saved_abort_line_depth;
            match pending {
                Some(Unwind::Return(n)) => n,
                Some(Unwind::AbortLine(_)) => status,
                other => {
                    adapter.ctx.pending = other;
                    if source_name.is_some()
                        && (adapter.ctx.function_depth == 0 || source_debug_scope)
                    {
                        let saved_suppression = adapter.ctx.suppress_debug_traps;
                        if !source_debug_scope {
                            adapter.ctx.suppress_debug_traps = true;
                        }
                        crate::trap::run_return_trap(adapter.ctx);
                        adapter.ctx.suppress_debug_traps = saved_suppression;
                    }
                    status
                }
            }
        }
        Ok(None) => 0,
        Err(err) => {
            report_eval_parse_error(adapter.ctx.env, src, &err);
            2
        }
    }
}

fn offset_command_lines(command: &mut Command, delta: u32) {
    if command.line > 0 {
        command.line = command.line.saturating_add(delta);
    }
    match &mut command.data {
        cherubsh_parser::CommandData::For(c) => {
            if c.line > 0 {
                c.line = c.line.saturating_add(delta);
            }
            offset_command_lines(&mut c.action, delta);
        }
        cherubsh_parser::CommandData::Case(c) => {
            if c.line > 0 {
                c.line = c.line.saturating_add(delta);
            }
            for clause in &mut c.clauses {
                if let Some(action) = &mut clause.action {
                    offset_command_lines(action, delta);
                }
            }
        }
        cherubsh_parser::CommandData::While(c) => {
            offset_command_lines(&mut c.test, delta);
            offset_command_lines(&mut c.action, delta);
        }
        cherubsh_parser::CommandData::Until(c) => {
            offset_command_lines(&mut c.test, delta);
            offset_command_lines(&mut c.action, delta);
        }
        cherubsh_parser::CommandData::If(c) => {
            offset_command_lines(&mut c.test, delta);
            offset_command_lines(&mut c.true_case, delta);
            if let Some(false_case) = &mut c.false_case {
                offset_command_lines(false_case, delta);
            }
        }
        cherubsh_parser::CommandData::Connection(c) => {
            offset_command_lines(&mut c.first, delta);
            offset_command_lines(&mut c.second, delta);
        }
        cherubsh_parser::CommandData::FunctionDef(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        cherubsh_parser::CommandData::Group(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        cherubsh_parser::CommandData::Select(c) => {
            if c.line > 0 {
                c.line = c.line.saturating_add(delta);
            }
            offset_command_lines(&mut c.action, delta);
        }
        cherubsh_parser::CommandData::ArithFor(c) => {
            offset_command_lines(&mut c.action, delta);
        }
        cherubsh_parser::CommandData::Subshell(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        cherubsh_parser::CommandData::Coproc(c) => {
            offset_command_lines(&mut c.command, delta);
        }
        cherubsh_parser::CommandData::Simple(_)
        | cherubsh_parser::CommandData::Arith(_)
        | cherubsh_parser::CommandData::Cond(_) => {}
    }
}

fn report_eval_parse_error(env: &dyn Environment, input_text: &str, err: &ParseError) {
    let offset = err.span.as_ref().map(|span| span.start).unwrap_or(0);
    let eval_line = line_number_for_offset(input_text, offset).unwrap_or(1);
    let message = if err.message == "expected ')'" {
        "syntax error: unexpected end of file"
    } else {
        &err.message
    };
    let prefix = match (env.diagnostic_source_name(), env.diagnostic_line()) {
        (Some(source), Some(line)) if message == "syntax error: unexpected end of file" => {
            let line = line
                .saturating_add(eval_unexpected_eof_line(input_text))
                .saturating_sub(1);
            format!("{source}: eval: line {line}")
        }
        (Some(source), Some(line)) => format!("{source}: eval: line {line}"),
        _ if message == "syntax error: unexpected end of file" => {
            format!(
                "cherubsh: eval: line {}",
                eval_unexpected_eof_line(input_text)
            )
        }
        _ => format!("cherubsh: eval: line {eval_line}"),
    };

    if let Some(token) = syntax_error_token(input_text, err) {
        eprintln!("{prefix}: syntax error near unexpected token `{token}'");
        if let Some(line) = source_line_for_offset(input_text, offset) {
            eprintln!("{prefix}: `{line}'");
        }
        return;
    }

    eprintln!("{prefix}: {message}");
}

fn eval_unexpected_eof_line(input_text: &str) -> u32 {
    let mut line = input_text.bytes().filter(|b| *b == b'\n').count() as u32 + 2;
    if has_odd_trailing_backslashes(input_text) {
        line = line.saturating_add(1);
    }
    line
}

fn has_odd_trailing_backslashes(input_text: &str) -> bool {
    let mut count = 0usize;
    for byte in input_text.as_bytes().iter().rev() {
        if *byte == b'\\' {
            count += 1;
        } else {
            break;
        }
    }
    count % 2 == 1
}

fn syntax_error_token(input_text: &str, err: &ParseError) -> Option<String> {
    if let Some(token) = err
        .message
        .strip_prefix("unexpected token '")
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Some(token.to_string());
    }

    let is_expected_word_error = err.message.starts_with("expected '")
        || matches!(
            err.message.as_str(),
            "expected for name"
                | "expected command"
                | "expected list"
                | "function body must be a compound command"
        );
    if !is_expected_word_error {
        return None;
    }

    err.span.as_ref().and_then(|span| {
        input_text
            .get(span.start.min(input_text.len())..span.end.min(input_text.len()))
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn line_number_for_offset(input_text: &str, offset: usize) -> Option<u32> {
    if input_text.is_empty() {
        return Some(1);
    }
    let end = offset.min(input_text.len());
    Some(1 + input_text[..end].bytes().filter(|b| *b == b'\n').count() as u32)
}

fn source_line_for_offset(input_text: &str, offset: usize) -> Option<String> {
    if input_text.is_empty() {
        return None;
    }
    let offset = offset.min(input_text.len());
    let start = input_text[..offset]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let end = input_text[offset..]
        .find('\n')
        .map(|idx| offset + idx)
        .unwrap_or(input_text.len());
    let line = input_text[start..end].trim_end_matches('\r');
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}
