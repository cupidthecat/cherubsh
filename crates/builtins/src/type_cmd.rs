use std::path::PathBuf;

use crate::common::{is_executable, report_diagnostic, search_path};
use crate::getopt::{GetOpt, OptParser};
use crate::{lookup_raw, Builtin, BuiltinCtx};
use cherubsh_common::{
    CASEPAT_FALLTHROUGH, CASEPAT_TESTNEXT, CMD_INVERT_RETURN, CMD_TIME_PIPELINE, CMD_TIME_POSIX,
};
use cherubsh_parser::{
    Command, CommandData, CondCommand, CondType, PatternList, Redirect, RedirectInstruction,
    Redirectee, Redirector, SimpleCommand, WordDesc, CONN_AMP, CONN_AND_AND, CONN_BAR_AND,
    CONN_NEWLINE, CONN_OR_OR, CONN_PIPE, CONN_SEMI,
};

pub struct Type;
pub static TYPE: Type = Type;

impl Builtin for Type {
    fn name(&self) -> &'static str {
        "type"
    }
    fn synopsis(&self) -> &'static str {
        "type [-afptP] name [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut all_paths = false;
        let mut force_path = false;
        let mut path_only = false;
        let mut type_only = false;
        let mut suppress_function_lookup = false;
        let mut parser = OptParser::new(ctx.args, "afptP");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'a', .. } => all_paths = true,
                GetOpt::Opt { ch: 'f', .. } => suppress_function_lookup = true,
                GetOpt::Opt { ch: 'p', .. } => path_only = true,
                GetOpt::Opt { ch: 't', .. } => type_only = true,
                GetOpt::Opt { ch: 'P', .. } => force_path = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "type", &format!("-{ch}: invalid option"));
                    eprintln!("type: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "type",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("type: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            return 0;
        }
        let mut status = 0;
        for name in rest {
            let mut found_any = false;

            if path_only
                && !force_path
                && command_exists_without_path(ctx, name, suppress_function_lookup)
            {
                found_any = true;
            }

            if ctx.env_ref().aliases_enabled()
                && !suppress_function_lookup
                && !force_path
                && !path_only
            {
                if let Some(value) = ctx.env_ref().alias_get(name) {
                    if type_only {
                        println!("alias");
                    } else {
                        println!("{name} is aliased to `{value}'");
                    }
                    found_any = true;
                    if !all_paths {
                        continue;
                    }
                }
            }

            if !force_path && !path_only && is_keyword(name) {
                if type_only {
                    println!("keyword");
                } else {
                    println!("{name} is a shell keyword");
                }
                found_any = true;
                if !all_paths {
                    continue;
                }
            }

            if !suppress_function_lookup && !force_path && !path_only {
                if let Some(function) = ctx.shell.function_get(name) {
                    if type_only {
                        println!("function");
                    } else {
                        println!("{name} is a function");
                        print_function_definition(name, &function);
                    }
                    found_any = true;
                    if !all_paths {
                        continue;
                    }
                }
            }

            if !force_path
                && !path_only
                && lookup_raw(name).is_some()
                && ctx.env_ref().builtin_enabled(name)
            {
                if type_only {
                    println!("builtin");
                } else {
                    println!("{name} is a shell builtin");
                }
                found_any = true;
                if !all_paths {
                    continue;
                }
            }

            // Hash + PATH lookup.
            let hashed = if !all_paths {
                ctx.env().hash_get_with_hit(name)
            } else {
                ctx.env_ref().hash_get(name)
            };
            if !all_paths {
                if let Some(path) = hashed {
                    if type_only {
                        println!("file");
                    } else if path_only || force_path {
                        println!("{}", path.display());
                    } else {
                        println!("{name} is hashed ({})", path.display());
                    }
                    continue;
                }
            }

            let env_ref: &dyn cherubsh_common::Environment = ctx.env_ref();
            if all_paths {
                for path in search_all_paths(name, env_ref) {
                    if type_only {
                        println!("file");
                    } else if path_only || force_path {
                        println!("{}", path.display());
                    } else {
                        println!("{name} is {}", path.display());
                    }
                    found_any = true;
                }
            } else if let Some(path) = search_path(name, env_ref) {
                if type_only {
                    println!("file");
                } else if path_only || force_path {
                    println!("{}", path.display());
                } else {
                    println!("{name} is {}", path.display());
                }
                found_any = true;
            }

            if !found_any {
                if !type_only && !path_only && !force_path {
                    report_diagnostic(ctx.env_ref(), "type", &format!("{name}: not found"));
                }
                status = 1;
            }
        }
        status
    }
}

fn command_exists_without_path(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    suppress_function_lookup: bool,
) -> bool {
    (ctx.env_ref().aliases_enabled() && ctx.env_ref().alias_get(name).is_some())
        || is_keyword(name)
        || (!suppress_function_lookup && ctx.shell.function_get(name).is_some())
        || (lookup_raw(name).is_some() && ctx.env_ref().builtin_enabled(name))
}

fn search_all_paths(name: &str, env: &dyn cherubsh_common::Environment) -> Vec<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return if is_executable(&path) {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let mut out = Vec::new();
    let path = env.get("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let mut candidate = PathBuf::from(dir);
        candidate.push(name);
        if is_executable(&candidate) {
            out.push(candidate);
        }
    }
    out
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
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
            | "{"
            | "}"
            | "!"
            | "[["
            | "]]"
            | "coproc"
    )
}

pub(crate) fn print_function_definition(name: &str, command: &Command) {
    println!("{name} () ");
    println!("{{ ");
    for line in render_function_body(command) {
        match line {
            RenderLine::Indented(line) if line.is_empty() => println!(),
            RenderLine::Indented(line) => println!("    {line}"),
            RenderLine::Raw(line) => println!("{line}"),
        }
    }
    println!("{}", function_close_line(command, false));
}

pub(crate) fn function_export_value(command: &Command) -> String {
    let mut out = String::from("() {\n");
    for line in render_function_body(command) {
        match line {
            RenderLine::Indented(line) if line.is_empty() => out.push('\n'),
            RenderLine::Indented(line) => {
                out.push_str("    ");
                out.push_str(&line);
                out.push('\n');
            }
            RenderLine::Raw(line) => {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out.push_str(&function_close_line(command, false));
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RenderLine {
    Indented(String),
    Raw(String),
}

fn render_function_body(command: &Command) -> Vec<RenderLine> {
    function_body_inner_lines(command)
}

fn function_body_inner_lines(command: &Command) -> Vec<RenderLine> {
    if let CommandData::Group(group) = &command.data {
        render_function_body_terminated(&group.command, false)
    } else {
        render_function_body_terminated(command, false)
    }
}

fn function_close_line(command: &Command, terminated: bool) -> String {
    let redirects: &[Redirect] = if matches!(command.data, CommandData::Group(_)) {
        &command.redirects
    } else {
        &[]
    };
    terminate_compound_line("}".to_string(), redirects, terminated)
}

fn render_function_body_terminated(command: &Command, terminated: bool) -> Vec<RenderLine> {
    match &command.data {
        CommandData::Group(group) => {
            let mut out = vec![RenderLine::Indented("{ ".to_string())];
            out.extend(
                render_function_body_terminated(&group.command, false)
                    .into_iter()
                    .map(indent_render_line),
            );
            out.push(RenderLine::Indented(terminate_compound_line(
                "}".to_string(),
                &command.redirects,
                terminated,
            )));
            out
        }
        CommandData::Connection(conn)
            if conn.connector == CONN_SEMI || conn.connector == CONN_NEWLINE =>
        {
            let mut out = render_function_body_terminated(&conn.first, true);
            out.extend(render_function_body_terminated(&conn.second, terminated));
            out
        }
        CommandData::Connection(conn) if conn.connector == CONN_AMP => {
            render_background_connection(&conn.first, &conn.second, terminated)
        }
        CommandData::For(for_cmd) => {
            let words = for_cmd
                .map_list
                .as_ref()
                .map(|words| {
                    words
                        .iter()
                        .map(|word| pretty_word(&word.text))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let header = if words.is_empty() {
                format!("for {};", for_cmd.name.text)
            } else {
                format!("for {} in {words};", for_cmd.name.text)
            };
            let mut out = vec![
                RenderLine::Indented(header),
                RenderLine::Indented("do".to_string()),
            ];
            out.extend(
                render_function_body_terminated(&for_cmd.action, true)
                    .into_iter()
                    .map(indent_render_line),
            );
            let close = if terminated && !command_contains_heredoc(&for_cmd.action) {
                "done;".to_string()
            } else {
                "done".to_string()
            };
            out.push(RenderLine::Indented(close));
            out
        }
        CommandData::ArithFor(for_cmd) => {
            let init = render_arith_for_clause(for_cmd.init.as_ref());
            let test = render_arith_for_clause(for_cmd.test.as_ref());
            let step = render_arith_for_clause(for_cmd.step.as_ref());
            let close = if for_cmd.step.is_some() { " ))" } else { "))" };
            let mut out = vec![
                RenderLine::Indented(format!("for (({init}; {test}; {step}{close}")),
                RenderLine::Indented("do".to_string()),
            ];
            out.extend(
                render_function_body_terminated(&for_cmd.action, true)
                    .into_iter()
                    .map(indent_render_line),
            );
            out.push(RenderLine::Indented(if terminated {
                "done;".to_string()
            } else {
                "done".to_string()
            }));
            out
        }
        CommandData::If(if_cmd) => {
            let mut out = vec![RenderLine::Indented(format!(
                "if {}; then",
                render_if_test(&if_cmd.test)
            ))];
            out.extend(
                render_function_body_terminated(&if_cmd.true_case, true)
                    .into_iter()
                    .map(indent_render_line),
            );
            if let Some(false_case) = &if_cmd.false_case {
                out.push(RenderLine::Indented("else".to_string()));
                out.extend(
                    render_function_body_terminated(false_case, true)
                        .into_iter()
                        .map(indent_render_line),
                );
            }
            out.push(RenderLine::Indented(terminate_compound_line(
                "fi".to_string(),
                &command.redirects,
                terminated,
            )));
            out
        }
        CommandData::While(while_cmd) => {
            let mut out = vec![RenderLine::Indented(format!(
                "while {}; do",
                render_loop_test(&while_cmd.test)
            ))];
            out.extend(
                render_function_body_terminated(&while_cmd.action, true)
                    .into_iter()
                    .map(indent_render_line),
            );
            out.push(RenderLine::Indented(terminate_compound_line(
                "done".to_string(),
                &command.redirects,
                terminated,
            )));
            out
        }
        CommandData::Until(until_cmd) => {
            let mut out = vec![RenderLine::Indented(format!(
                "until {}; do",
                render_loop_test(&until_cmd.test)
            ))];
            out.extend(
                render_function_body_terminated(&until_cmd.action, true)
                    .into_iter()
                    .map(indent_render_line),
            );
            out.push(RenderLine::Indented(terminate_compound_line(
                "done".to_string(),
                &command.redirects,
                terminated,
            )));
            out
        }
        CommandData::Case(case_cmd) => {
            let mut out = vec![RenderLine::Indented(format!(
                "case {} in ",
                pretty_word(&case_cmd.word.text)
            ))];
            for clause in &case_cmd.clauses {
                out.extend(
                    render_case_clause(clause)
                        .into_iter()
                        .map(indent_render_line),
                );
            }
            out.push(RenderLine::Indented(terminate_compound_line(
                "esac".to_string(),
                &command.redirects,
                terminated,
            )));
            out
        }
        CommandData::FunctionDef(function) => {
            let mut out = vec![
                RenderLine::Indented(format!("function {} () ", function.name.text)),
                RenderLine::Indented("{ ".to_string()),
            ];
            out.extend(
                function_body_inner_lines(&function.command)
                    .into_iter()
                    .map(indent_render_line),
            );
            out.push(RenderLine::Indented(function_close_line(
                &function.command,
                terminated,
            )));
            out
        }
        CommandData::Subshell(subshell) => {
            let mut inner = render_function_body_terminated(&subshell.command, false);
            if inner.is_empty() {
                return vec![RenderLine::Indented(if terminated {
                    "(  );".to_string()
                } else {
                    "(  )".to_string()
                })];
            }
            let first = inner.remove(0);
            let mut out = Vec::new();
            match first {
                RenderLine::Indented(line) => out.push(RenderLine::Indented(format!("( {line}"))),
                RenderLine::Raw(line) => {
                    out.push(RenderLine::Indented("(".to_string()));
                    out.push(RenderLine::Raw(line));
                }
            }
            let raw_body = out
                .iter()
                .chain(inner.iter())
                .any(|line| matches!(line, RenderLine::Raw(_)));
            if raw_body
                && matches!(inner.last(), Some(RenderLine::Indented(line)) if line.is_empty())
            {
                inner.pop();
            }
            out.extend(inner);
            let close = terminate_compound_line(
                " )".to_string(),
                &command.redirects,
                terminated && !raw_body,
            );
            if raw_body {
                out.push(RenderLine::Raw(close));
            } else if let Some(RenderLine::Indented(last)) = out.last_mut() {
                last.push_str(&close);
            } else {
                out.push(RenderLine::Indented(close));
            }
            out
        }
        CommandData::Coproc(coproc) => {
            let name = coproc
                .name
                .as_ref()
                .map(|word| word.text.as_str())
                .unwrap_or("COPROC");
            if let CommandData::Group(group) = &coproc.command.data {
                let mut out = vec![RenderLine::Indented(format!("coproc {name} {{ "))];
                out.extend(
                    render_function_body_terminated(&group.command, false)
                        .into_iter()
                        .map(indent_render_line),
                );
                let mut close = "}".to_string();
                append_redirects(&mut close, &command.redirects);
                out.push(RenderLine::Indented(close));
                return out;
            }
            let mut inner = render_function_body_terminated(&coproc.command, false);
            if inner.is_empty() {
                return vec![RenderLine::Indented(format!("coproc {name}"))];
            }
            let first = inner.remove(0);
            let mut out = Vec::new();
            match first {
                RenderLine::Indented(line) => {
                    out.push(RenderLine::Indented(format!("coproc {name} {line}")))
                }
                RenderLine::Raw(line) => {
                    out.push(RenderLine::Indented(format!("coproc {name}")));
                    out.push(RenderLine::Raw(line));
                }
            }
            out.extend(inner);
            out
        }
        CommandData::Simple(simple) => render_simple_command_lines(command, simple, terminated),
        _ => vec![RenderLine::Indented(terminate_rendered_line(
            render_command_line(command),
            terminated,
            false,
        ))],
    }
}

fn indent_render_line(line: RenderLine) -> RenderLine {
    match line {
        RenderLine::Indented(line) if line.is_empty() => RenderLine::Indented(line),
        RenderLine::Indented(line) => RenderLine::Indented(format!("    {line}")),
        RenderLine::Raw(line) => RenderLine::Raw(line),
    }
}

fn render_background_connection(
    first: &Command,
    second: &Command,
    terminated: bool,
) -> Vec<RenderLine> {
    if let CommandData::Connection(left) = &first.data {
        if left.connector == CONN_SEMI || left.connector == CONN_NEWLINE {
            let mut out = render_function_body_terminated(&left.first, true);
            out.extend(render_background_connection(
                &left.second,
                second,
                terminated,
            ));
            return out;
        }
    }

    let mut line = format!("{} &", render_command_line(first));
    if !is_null_command(second) {
        line.push(' ');
        line.push_str(&render_command_line(second));
    }
    vec![RenderLine::Indented(terminate_rendered_line(
        line, terminated, false,
    ))]
}

fn render_arith_for_clause(word: Option<&WordDesc>) -> String {
    word.map(|word| word.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "1".to_string())
}

fn render_loop_test(command: &Command) -> String {
    render_command_line(command)
        .trim_end_matches(';')
        .trim_end()
        .to_string()
}

fn render_if_test(command: &Command) -> String {
    render_command_line(command)
        .trim_end_matches(';')
        .trim_end()
        .to_string()
}

fn render_case_clause(clause: &PatternList) -> Vec<RenderLine> {
    let pattern = clause
        .patterns
        .iter()
        .map(|word| pretty_word(&word.text))
        .collect::<Vec<_>>()
        .join("|");
    let mut out = vec![RenderLine::Indented(format!("{pattern})"))];
    if let Some(action) = &clause.action {
        out.extend(
            render_function_body_terminated(action, false)
                .into_iter()
                .map(indent_render_line),
        );
    }
    let terminator = if clause.flags & CASEPAT_FALLTHROUGH != 0 {
        ";&"
    } else if clause.flags & CASEPAT_TESTNEXT != 0 {
        ";;&"
    } else {
        ";;"
    };
    out.push(RenderLine::Indented(terminator.to_string()));
    out
}

fn terminate_compound_line(line: String, redirects: &[Redirect], terminated: bool) -> String {
    let mut line = line;
    append_redirects(&mut line, redirects);
    terminate_rendered_line(line, terminated, false)
}

fn terminate_rendered_line(line: String, terminated: bool, has_heredoc: bool) -> String {
    if terminated && line != ":" && !has_heredoc {
        format!("{line};")
    } else {
        line
    }
}

fn render_command_line(command: &Command) -> String {
    let mut out = String::new();
    if command.flags & CMD_INVERT_RETURN != 0 {
        out.push_str("! ");
    }
    if command.flags & CMD_TIME_PIPELINE != 0 {
        out.push_str("time");
        if command.flags & CMD_TIME_POSIX != 0 {
            out.push_str(" -p");
        }
    }
    match &command.data {
        CommandData::Simple(simple) => {
            append_simple_command(&mut out, simple);
            if simple.words.is_empty() && out == "time" {
                out.push(' ');
            }
        }
        CommandData::Arith(arith) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("(( ");
            out.push_str(arith.expression.text.trim());
            out.push_str(" ))");
        }
        CommandData::Cond(cond) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("[[ ");
            out.push_str(&render_cond(cond));
            out.push_str(" ]]");
        }
        CommandData::Subshell(subshell) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("( ");
            out.push_str(&render_command_line(&subshell.command));
            out.push_str(" )");
        }
        CommandData::Group(group) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("{ ");
            out.push_str(&render_command_line(&group.command));
            out.push_str(" }");
        }
        CommandData::Connection(conn) => {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&render_command_line(&conn.first));
            out.push(' ');
            out.push_str(match conn.connector {
                CONN_AND_AND => "&&",
                CONN_OR_OR => "||",
                CONN_PIPE => "|",
                CONN_BAR_AND => "|&",
                CONN_AMP => "&",
                _ => ";",
            });
            out.push(' ');
            out.push_str(&render_command_line(&conn.second));
        }
        _ => {
            if out.is_empty() {
                out.push(':');
            }
        }
    }
    append_redirects(&mut out, &command.redirects);
    if out.is_empty() {
        ":".to_string()
    } else {
        out
    }
}

fn render_simple_command_lines(
    command: &Command,
    simple: &SimpleCommand,
    terminated: bool,
) -> Vec<RenderLine> {
    let mut out = String::new();
    if command.flags & CMD_INVERT_RETURN != 0 {
        out.push_str("! ");
    }
    if command.flags & CMD_TIME_PIPELINE != 0 {
        out.push_str("time");
        if command.flags & CMD_TIME_POSIX != 0 {
            out.push_str(" -p");
        }
    }
    append_simple_command(&mut out, simple);
    if simple.words.is_empty() && out == "time" {
        out.push(' ');
    }
    append_redirects(&mut out, &command.redirects);
    let has_heredoc = simple.redirects.iter().any(is_heredoc_redirect)
        || command.redirects.iter().any(is_heredoc_redirect);
    let mut lines = vec![RenderLine::Indented(terminate_rendered_line(
        if out.is_empty() { ":".to_string() } else { out },
        terminated,
        has_heredoc,
    ))];
    append_heredoc_lines(&mut lines, &simple.redirects);
    append_heredoc_lines(&mut lines, &command.redirects);
    lines
}

fn append_simple_command(out: &mut String, simple: &SimpleCommand) {
    if !simple.words.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(
            &simple
                .words
                .iter()
                .map(|word| pretty_word(&word.text))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    append_redirects(out, &simple.redirects);
}

fn append_redirects(out: &mut String, redirects: &[Redirect]) {
    for redirect in redirects {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&render_redirect(redirect));
    }
}

fn render_redirect(redirect: &Redirect) -> String {
    let prefix = match &redirect.redirector {
        Redirector::Fd(fd) => {
            let move_redirect = matches!(
                redirect.instruction,
                RedirectInstruction::MoveInput
                    | RedirectInstruction::MoveOutput
                    | RedirectInstruction::MoveInputWord
                    | RedirectInstruction::MoveOutputWord
            );
            let default = match redirect.instruction {
                RedirectInstruction::OutputDirection
                | RedirectInstruction::AppendingTo
                | RedirectInstruction::OutputForce
                | RedirectInstruction::ErrAndOut
                | RedirectInstruction::AppendErrAndOut => 1,
                RedirectInstruction::InputDirection
                | RedirectInstruction::ReadingUntil
                | RedirectInstruction::ReadingString
                | RedirectInstruction::DeblankReadingUntil
                | RedirectInstruction::InputOutput => 0,
                _ => -1,
            };
            if default >= 0 && *fd == default && !move_redirect {
                String::new()
            } else {
                fd.to_string()
            }
        }
        Redirector::Var(name) => format!("{{{name}}}"),
    };
    let op = match redirect.instruction {
        RedirectInstruction::OutputDirection => ">",
        RedirectInstruction::InputDirection => "<",
        RedirectInstruction::InputaDirection => "<&",
        RedirectInstruction::AppendingTo => ">>",
        RedirectInstruction::ReadingUntil => "<<",
        RedirectInstruction::ReadingString => "<<<",
        RedirectInstruction::DuplicatingInput | RedirectInstruction::DuplicatingInputWord => "<&",
        RedirectInstruction::DuplicatingOutput | RedirectInstruction::DuplicatingOutputWord => ">&",
        RedirectInstruction::DeblankReadingUntil => "<<-",
        RedirectInstruction::CloseThis if matches!(redirect.redirector, Redirector::Var(_)) => ">&",
        RedirectInstruction::CloseThis => "<&",
        RedirectInstruction::ErrAndOut => "&>",
        RedirectInstruction::InputOutput => "<>",
        RedirectInstruction::OutputForce => ">|",
        RedirectInstruction::MoveInput | RedirectInstruction::MoveInputWord => "<&",
        RedirectInstruction::MoveOutput | RedirectInstruction::MoveOutputWord => ">&",
        RedirectInstruction::AppendErrAndOut => "&>>",
    };
    let target = match &redirect.redirectee {
        Redirectee::Word(word) => word.text.clone(),
        Redirectee::Fd(fd) if *fd < 0 => "-".to_string(),
        Redirectee::Fd(fd)
            if matches!(
                redirect.instruction,
                RedirectInstruction::MoveInput
                    | RedirectInstruction::MoveOutput
                    | RedirectInstruction::MoveInputWord
                    | RedirectInstruction::MoveOutputWord
            ) =>
        {
            format!("{fd}-")
        }
        Redirectee::Fd(fd) => fd.to_string(),
    };
    if is_heredoc_redirect(redirect) {
        format!("{prefix}{op}{target}")
    } else if redirect_spacing_elided(redirect) {
        format!("{prefix}{op}{target}")
    } else {
        format!("{prefix}{op} {target}")
    }
}

fn redirect_spacing_elided(redirect: &Redirect) -> bool {
    matches!(
        redirect.instruction,
        RedirectInstruction::DuplicatingInput
            | RedirectInstruction::DuplicatingOutput
            | RedirectInstruction::DuplicatingInputWord
            | RedirectInstruction::DuplicatingOutputWord
            | RedirectInstruction::MoveInput
            | RedirectInstruction::MoveOutput
            | RedirectInstruction::MoveInputWord
            | RedirectInstruction::MoveOutputWord
            | RedirectInstruction::CloseThis
    ) && matches!(&redirect.redirectee, Redirectee::Fd(_))
        || matches!(
            redirect.instruction,
            RedirectInstruction::DuplicatingInputWord
                | RedirectInstruction::DuplicatingOutputWord
                | RedirectInstruction::MoveInputWord
                | RedirectInstruction::MoveOutputWord
        )
}

fn append_heredoc_lines(out: &mut Vec<RenderLine>, redirects: &[Redirect]) {
    for redirect in redirects
        .iter()
        .filter(|redirect| is_heredoc_redirect(redirect))
    {
        let body = redirect.here_doc_body.as_deref().unwrap_or_default();
        for line in body.lines() {
            let line = if matches!(
                redirect.instruction,
                RedirectInstruction::DeblankReadingUntil
            ) {
                line.trim_start_matches('\t')
            } else {
                line
            };
            out.push(RenderLine::Raw(line.to_string()));
        }
        out.push(RenderLine::Raw(
            redirect
                .here_doc_eof
                .clone()
                .unwrap_or_else(|| redirect_target_text(redirect)),
        ));
        out.push(RenderLine::Indented(String::new()));
    }
}

fn is_heredoc_redirect(redirect: &Redirect) -> bool {
    matches!(
        redirect.instruction,
        RedirectInstruction::ReadingUntil | RedirectInstruction::DeblankReadingUntil
    )
}

fn redirect_target_text(redirect: &Redirect) -> String {
    match &redirect.redirectee {
        Redirectee::Word(word) => word.text.clone(),
        Redirectee::Fd(fd) if *fd < 0 => "-".to_string(),
        Redirectee::Fd(fd) => fd.to_string(),
    }
}

fn render_cond(cmd: &CondCommand) -> String {
    match cmd.cond_type {
        CondType::And => format!(
            "{} && {}",
            render_cond(cmd.left.as_deref().unwrap()),
            render_cond(cmd.right.as_deref().unwrap())
        ),
        CondType::Or => format!(
            "{} || {}",
            render_cond(cmd.left.as_deref().unwrap()),
            render_cond(cmd.right.as_deref().unwrap())
        ),
        CondType::Unary => {
            let op = cmd.op.as_ref().map(|word| word.text.as_str()).unwrap_or("");
            let arg = cmd
                .left
                .as_deref()
                .and_then(|left| left.term.as_ref())
                .map(|word| pretty_word(&word.text))
                .unwrap_or_default();
            format!("{op} {arg}")
        }
        CondType::Binary => {
            let left = cmd
                .left
                .as_deref()
                .and_then(|left| left.term.as_ref())
                .map(|word| pretty_word(&word.text))
                .unwrap_or_default();
            let op = cmd.op.as_ref().map(|word| word.text.as_str()).unwrap_or("");
            let right = cmd
                .right
                .as_deref()
                .and_then(|right| right.term.as_ref())
                .map(|word| pretty_word(&word.text))
                .unwrap_or_default();
            format!("{left} {op} {right}")
        }
        CondType::Term => {
            if let Some(inner) = cmd.left.as_deref() {
                format!("! {}", render_cond(inner))
            } else {
                cmd.term
                    .as_ref()
                    .map(|word| pretty_word(&word.text))
                    .unwrap_or_default()
            }
        }
        CondType::Expr => format!("( {} )", render_cond(cmd.left.as_deref().unwrap())),
    }
}

fn is_null_command(command: &Command) -> bool {
    matches!(
        &command.data,
        CommandData::Simple(simple) if simple.words.is_empty() && simple.redirects.is_empty()
    ) && command.redirects.is_empty()
}

fn command_contains_heredoc(command: &Command) -> bool {
    if command.redirects.iter().any(is_heredoc_redirect) {
        return true;
    }
    match &command.data {
        CommandData::Simple(simple) => simple.redirects.iter().any(is_heredoc_redirect),
        CommandData::Group(group) => command_contains_heredoc(&group.command),
        CommandData::Connection(conn) => {
            command_contains_heredoc(&conn.first) || command_contains_heredoc(&conn.second)
        }
        CommandData::For(for_cmd) => command_contains_heredoc(&for_cmd.action),
        CommandData::Select(select_cmd) => command_contains_heredoc(&select_cmd.action),
        CommandData::While(while_cmd) => {
            command_contains_heredoc(&while_cmd.test) || command_contains_heredoc(&while_cmd.action)
        }
        CommandData::Until(until_cmd) => {
            command_contains_heredoc(&until_cmd.test) || command_contains_heredoc(&until_cmd.action)
        }
        CommandData::If(if_cmd) => {
            command_contains_heredoc(&if_cmd.test)
                || command_contains_heredoc(&if_cmd.true_case)
                || if_cmd
                    .false_case
                    .as_deref()
                    .map(command_contains_heredoc)
                    .unwrap_or(false)
        }
        CommandData::Case(case_cmd) => case_cmd
            .clauses
            .iter()
            .filter_map(|clause| clause.action.as_deref())
            .any(command_contains_heredoc),
        CommandData::ArithFor(for_cmd) => command_contains_heredoc(&for_cmd.action),
        CommandData::Subshell(subshell) => command_contains_heredoc(&subshell.command),
        CommandData::Coproc(coproc) => command_contains_heredoc(&coproc.command),
        CommandData::FunctionDef(def) => command_contains_heredoc(&def.command),
        CommandData::Arith(_) | CommandData::Cond(_) => false,
    }
}

fn pretty_word(word: &str) -> String {
    let word = normalize_cmdsub_spacing(word);
    let word = normalize_cmdsub_redir_spacing(&word);
    decode_ansi_c_word(&word).unwrap_or(word)
}

fn normalize_cmdsub_spacing(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut i = 0;
    while i < word.len() {
        let rest = &word[i..];
        if rest.starts_with("$((") {
            copy_arith_expansion(word, &mut i, &mut out);
            continue;
        }
        if rest.starts_with("$(<") {
            out.push_str("$(<");
            i += 3;
            continue;
        }
        if rest.starts_with("$(") {
            out.push_str("$(");
            i += 2;
            while let Some(ch) = word[i..].chars().next() {
                if ch.is_whitespace() {
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
            while i < word.len() {
                let ch = word[i..].chars().next().unwrap();
                if ch == ')' {
                    while out.ends_with(char::is_whitespace) {
                        out.pop();
                    }
                    out.push(')');
                    i += 1;
                    break;
                }
                out.push(ch);
                i += ch.len_utf8();
            }
            continue;
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn copy_arith_expansion(word: &str, i: &mut usize, out: &mut String) {
    out.push_str("$((");
    *i += 3;
    let mut depth = 1usize;
    while *i < word.len() {
        let rest = &word[*i..];
        let ch = rest.chars().next().unwrap();
        match ch {
            '(' => {
                depth += 1;
                out.push(ch);
                *i += ch.len_utf8();
            }
            ')' => {
                depth = depth.saturating_sub(1);
                out.push(ch);
                *i += ch.len_utf8();
                if depth == 0 {
                    if word[*i..].starts_with(')') {
                        out.push(')');
                        *i += 1;
                    }
                    break;
                }
            }
            '\\' => {
                out.push(ch);
                *i += ch.len_utf8();
                if *i < word.len() {
                    let next = word[*i..].chars().next().unwrap();
                    out.push(next);
                    *i += next.len_utf8();
                }
            }
            '\'' | '"' | '`' => {
                let quote = ch;
                out.push(ch);
                *i += ch.len_utf8();
                while *i < word.len() {
                    let quoted = word[*i..].chars().next().unwrap();
                    out.push(quoted);
                    *i += quoted.len_utf8();
                    if quoted == '\\' && *i < word.len() {
                        let escaped = word[*i..].chars().next().unwrap();
                        out.push(escaped);
                        *i += escaped.len_utf8();
                    } else if quoted == quote {
                        break;
                    }
                }
            }
            _ => {
                out.push(ch);
                *i += ch.len_utf8();
            }
        }
    }
}

fn normalize_cmdsub_redir_spacing(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut i = 0;
    while i < word.len() {
        let rest = &word[i..];
        if rest.starts_with("$(<") {
            out.push_str("$(<");
            i += 3;
            if let Some(ch) = word[i..].chars().next() {
                if !ch.is_whitespace() && ch != ')' {
                    out.push(' ');
                }
            }
            continue;
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn decode_ansi_c_word(word: &str) -> Option<String> {
    let start = word.find("$'")?;
    let mut chars = word[start + 2..].char_indices().peekable();
    let mut decoded = String::new();
    let mut consumed_end = None;
    while let Some((idx, ch)) = chars.next() {
        if ch == '\'' {
            consumed_end = Some(start + 2 + idx + 1);
            break;
        }
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let Some((_, esc)) = chars.next() else {
            decoded.push('\\');
            break;
        };
        match esc {
            'a' => decoded.push('\x07'),
            'b' => decoded.push('\x08'),
            'e' | 'E' => decoded.push('\x1b'),
            'f' => decoded.push('\x0c'),
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            'v' => decoded.push('\x0b'),
            '\\' => decoded.push('\\'),
            '\'' => decoded.push('\''),
            '0'..='7' => {
                let mut value = esc.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().copied() {
                        Some((_, c @ '0'..='7')) => {
                            chars.next();
                            value = value * 8 + c.to_digit(8).unwrap_or(0);
                        }
                        _ => break,
                    }
                }
                decoded.push(char::from_u32(value).unwrap_or('\0'));
            }
            _ => decoded.push(esc),
        }
    }
    let end = consumed_end?;
    let mut out = String::new();
    out.push_str(&word[..start]);
    out.push('\'');
    out.push_str(&decoded);
    out.push('\'');
    out.push_str(&word[end..]);
    Some(out)
}
