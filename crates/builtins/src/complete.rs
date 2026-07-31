//! `complete` / `compgen` / `compopt` builtins.

use std::path::PathBuf;

use cherubsh_common::completion::{
    hostname_names, locale_sort, matching_service_names, signal_names, system_group_names,
    system_user_names, CompAction, CompOpts, CompSlot, CompSpec,
};
use cherubsh_common::{Environment, JobState, VarAttrs, VarKind, W_QUOTED};
use cherubsh_expander::pattern::{fnmatch, GlobOpts};

use crate::common::{is_executable, is_valid_name, report_diagnostic};
use crate::{Builtin, BuiltinCtx};

#[derive(Default)]
struct ParsedFlags {
    spec: CompSpec,
    remove: bool,
    print: bool,
    default: bool,
    initial: bool,
    empty: bool,
    variable: Option<String>,
    option_seen: bool,
}

fn short_to_action(c: char) -> Option<CompAction> {
    Some(match c {
        'a' => CompAction::Alias,
        'b' => CompAction::Builtin,
        'c' => CompAction::Command,
        'd' => CompAction::Directory,
        'e' => CompAction::Export,
        'f' => CompAction::File,
        'g' => CompAction::Group,
        'j' => CompAction::Job,
        'k' => CompAction::Keyword,
        's' => CompAction::Service,
        'u' => CompAction::User,
        'v' => CompAction::Variable,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionBuiltin {
    Complete,
    Compgen,
}

fn parse_complete_flags(
    args: &[String],
    builtin: CompletionBuiltin,
) -> Result<(ParsedFlags, Vec<String>, usize), String> {
    let mut flags = ParsedFlags::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if let Some(rest) = arg.strip_prefix('-') {
            if rest.is_empty() {
                i += 1;
                continue;
            }
            let mut j = 0;
            let chars: Vec<char> = rest.chars().collect();
            flags.option_seen = true;
            while j < chars.len() {
                let c = chars[j];
                match c {
                    'r' if builtin == CompletionBuiltin::Complete => flags.remove = true,
                    'p' if builtin == CompletionBuiltin::Complete => flags.print = true,
                    'D' if builtin == CompletionBuiltin::Complete => flags.default = true,
                    'I' if builtin == CompletionBuiltin::Complete => flags.initial = true,
                    'E' if builtin == CompletionBuiltin::Complete => flags.empty = true,
                    'V' if builtin == CompletionBuiltin::Compgen => {
                        flags.variable = Some(option_arg(args, &chars, &mut i, j, 'V')?);
                        break;
                    }
                    'A' => {
                        let v = option_arg(args, &chars, &mut i, j, 'A')?;
                        let action = CompAction::parse(&v)
                            .ok_or_else(|| format!("{v}: invalid action name"))?;
                        if !flags.spec.actions.contains(&action) {
                            flags.spec.actions.push(action);
                        }
                        break;
                    }
                    'F' => {
                        flags.spec.function = Some(option_arg(args, &chars, &mut i, j, 'F')?);
                        break;
                    }
                    'C' => {
                        flags.spec.command = Some(option_arg(args, &chars, &mut i, j, 'C')?);
                        break;
                    }
                    'G' => {
                        flags.spec.glob_pattern = Some(option_arg(args, &chars, &mut i, j, 'G')?);
                        break;
                    }
                    'W' => {
                        flags.spec.wordlist = Some(option_arg(args, &chars, &mut i, j, 'W')?);
                        break;
                    }
                    'X' => {
                        flags.spec.filterpat = Some(option_arg(args, &chars, &mut i, j, 'X')?);
                        break;
                    }
                    'P' => {
                        flags.spec.prefix = Some(option_arg(args, &chars, &mut i, j, 'P')?);
                        break;
                    }
                    'S' => {
                        flags.spec.suffix = Some(option_arg(args, &chars, &mut i, j, 'S')?);
                        break;
                    }
                    'o' => {
                        let name = option_arg(args, &chars, &mut i, j, 'o')?;
                        let bit = CompOpts::parse(&name)
                            .ok_or_else(|| format!("{name}: invalid option name"))?;
                        flags.spec.options |= bit;
                        break;
                    }
                    other => {
                        if let Some(act) = short_to_action(other) {
                            if !flags.spec.actions.contains(&act) {
                                flags.spec.actions.push(act);
                            }
                        } else {
                            return Err(format!("-{other}: invalid option"));
                        }
                    }
                }
                j += 1;
            }
            i += 1;
        } else {
            break;
        }
    }
    Ok((flags, args[i..].to_vec(), i))
}

fn option_arg(
    args: &[String],
    chars: &[char],
    i: &mut usize,
    j: usize,
    opt: char,
) -> Result<String, String> {
    if j + 1 < chars.len() {
        return Ok(chars[j + 1..].iter().collect());
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("missing arg to -{opt}"))
}

fn requested_slot(flags: &ParsedFlags) -> Option<CompSlot> {
    if flags.default {
        Some(CompSlot::Default)
    } else if flags.empty {
        Some(CompSlot::Empty)
    } else if flags.initial {
        Some(CompSlot::Initial)
    } else {
        None
    }
}

fn slot_name(slot: CompSlot) -> &'static str {
    match slot {
        CompSlot::Command => "",
        CompSlot::Default => "-D",
        CompSlot::Initial => "-I",
        CompSlot::Empty => "-E",
    }
}

pub struct Complete;
pub static COMPLETE: Complete = Complete;

impl Builtin for Complete {
    fn name(&self) -> &'static str {
        "complete"
    }
    fn synopsis(&self) -> &'static str {
        "complete [-abcdefgjksuv] [-pr] [-DEI] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let (flags, names, _) = match parse_complete_flags(ctx.args, CompletionBuiltin::Complete) {
            Ok(x) => x,
            Err(e) => {
                report_diagnostic(ctx.env_ref(), "complete", &e);
                if e.starts_with('-') || e.contains("missing arg") {
                    eprintln!("complete: usage: {}", self.synopsis());
                }
                return 2;
            }
        };
        if let Some(function) = flags.spec.function.as_deref() {
            if !is_valid_name(function) {
                report_diagnostic(
                    ctx.env_ref(),
                    "complete",
                    &format!("`{function}': not a valid identifier"),
                );
                return 2;
            }
        }
        if flags.print {
            if let Some(slot) = requested_slot(&flags) {
                if let Some(spec) = ctx.env_ref().compspec_get(slot, None) {
                    print_one(slot, None, &spec);
                    return 0;
                }
                report_diagnostic(
                    ctx.env_ref(),
                    "complete",
                    &format!("{}: no completion specification", slot_name(slot)),
                );
                return 1;
            }
            if names.is_empty() {
                for (slot, key, spec) in ctx.env_ref().compspec_iter() {
                    print_one(slot, key.as_deref(), &spec);
                }
                return 0;
            }
            let mut status = 0;
            for name in &names {
                if let Some(spec) = ctx.env_ref().compspec_get(CompSlot::Command, Some(name)) {
                    print_one(CompSlot::Command, Some(name), &spec);
                } else {
                    report_diagnostic(
                        ctx.env_ref(),
                        "complete",
                        &format!("{name}: no completion specification"),
                    );
                    status = 1;
                }
            }
            return status;
        }
        if flags.remove {
            if let Some(slot) = requested_slot(&flags) {
                if ctx.env().compspec_remove(slot, None) {
                    return 0;
                }
                report_diagnostic(
                    ctx.env_ref(),
                    "complete",
                    &format!("{}: no completion specification", slot_name(slot)),
                );
                return 1;
            }
            if names.is_empty() {
                let specs = ctx.env_ref().compspec_iter();
                for (slot, key, _) in specs {
                    ctx.env().compspec_remove(slot, key.as_deref());
                }
                return 0;
            }
            let mut status = 0;
            for n in &names {
                if !ctx.env().compspec_remove(CompSlot::Command, Some(n)) {
                    report_diagnostic(
                        ctx.env_ref(),
                        "complete",
                        &format!("{n}: no completion specification"),
                    );
                    status = 1;
                }
            }
            return status;
        }
        if let Some(slot) = requested_slot(&flags) {
            ctx.env().compspec_set(slot, None, flags.spec);
            return 0;
        }
        if names.is_empty() {
            if !flags.spec.is_empty() {
                eprintln!("complete: usage: {}", self.synopsis());
                return 2;
            }
            for (slot, key, spec) in ctx.env_ref().compspec_iter() {
                print_one(slot, key.as_deref(), &spec);
            }
            return 0;
        }
        for name in names {
            ctx.env()
                .compspec_set(CompSlot::Command, Some(&name), flags.spec.clone());
        }
        0
    }
}

fn print_one(slot: CompSlot, key: Option<&str>, spec: &CompSpec) {
    let tag = match slot {
        CompSlot::Command => render_completion_name(key.unwrap_or("")),
        CompSlot::Default => "-D".to_string(),
        CompSlot::Initial => "-I".to_string(),
        CompSlot::Empty => "-E".to_string(),
    };
    println!(
        "complete{}{}",
        spec.render_flags(),
        if tag.is_empty() {
            String::new()
        } else {
            format!(" {tag}")
        }
    );
}

fn render_completion_name(name: &str) -> String {
    if name.is_empty() {
        "''".to_string()
    } else if name
        .chars()
        .any(|ch| ch.is_whitespace() || "|&;()<>$`\\\"'*!?[]{}".contains(ch))
    {
        shell_quote(name)
    } else {
        name.to_string()
    }
}

pub struct Compgen;
pub static COMPGEN: Compgen = Compgen;
impl Builtin for Compgen {
    fn name(&self) -> &'static str {
        "compgen"
    }
    fn synopsis(&self) -> &'static str {
        "compgen [-V varname] [-abcdefgjksuv] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [word]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let (flags, rest, rest_start) =
            match parse_complete_flags(ctx.args, CompletionBuiltin::Compgen) {
                Ok(x) => x,
                Err(e) => {
                    report_diagnostic(ctx.env_ref(), "compgen", &e);
                    if e.starts_with('-') || e.contains("missing arg") {
                        eprintln!("compgen: usage: {}", self.synopsis());
                    }
                    return 2;
                }
            };
        if !flags.option_seen {
            return 0;
        }
        if let Some(varname) = flags.variable.as_deref() {
            if !is_valid_name(varname) {
                report_diagnostic(
                    ctx.env_ref(),
                    "compgen",
                    &format!("`{varname}': not a valid identifier"),
                );
                return 2;
            }
        }
        if let Some(function) = flags.spec.function.as_deref() {
            if !is_valid_name(function) {
                report_diagnostic(
                    ctx.env_ref(),
                    "compgen",
                    &format!("`{function}': not a valid identifier"),
                );
                return 2;
            }
            report_diagnostic(
                ctx.env_ref(),
                "compgen",
                "warning: -F option may not work as you expect",
            );
        }
        if flags.spec.command.is_some() {
            report_diagnostic(
                ctx.env_ref(),
                "compgen",
                "warning: -C option may not work as you expect",
            );
        }
        let word = rest.first().cloned().unwrap_or_default();
        let word_quoted = ctx
            .arg_flags
            .get(rest_start)
            .is_some_and(|flags| flags & W_QUOTED != 0);
        let matches = compgen_eval(ctx, &flags.spec, &word, word_quoted);
        if let Some(varname) = flags.variable.as_deref() {
            let status = if matches.is_empty() { 1 } else { 0 };
            ctx.env().set_array(varname, matches);
            return status;
        }
        if matches.is_empty() {
            return 1;
        }
        for m in matches {
            println!("{}", m);
        }
        0
    }
}

fn compgen_eval(
    ctx: &mut BuiltinCtx<'_>,
    spec: &CompSpec,
    word: &str,
    word_quoted: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(words) = &spec.wordlist {
        let expanded = ctx
            .shell
            .expand_completion_words(words, false)
            .unwrap_or_else(|| words.split_whitespace().map(str::to_string).collect());
        for w in expanded {
            if w.starts_with(word) {
                out.push(w);
            }
        }
    }
    let home = ctx.env_ref().get("HOME");
    for action in CompAction::GENERATION_ORDER
        .into_iter()
        .filter(|action| spec.actions.contains(action))
    {
        out.extend(action_compgen(
            ctx,
            action,
            word,
            word_quoted,
            home.as_deref(),
        ));
    }
    if let Some(pat) = &spec.glob_pattern {
        let mut matches = ctx
            .shell
            .expand_completion_words(pat, true)
            .unwrap_or_default();
        if has_glob_meta(pat) {
            matches.retain(|item| item != pat);
        }
        out.extend(matches);
    }
    if let Some(func) = &spec.function {
        let (matches, status) = run_completion_function(ctx, func, word);
        if status == 124 {
            return Vec::new();
        }
        out.extend(matches);
    }
    if let Some(command) = &spec.command {
        out.extend(run_completion_command(ctx, command, word));
    }
    if let Some(filter) = &spec.filterpat {
        out.retain(|m| {
            !filter_matches(
                filter,
                word,
                m,
                ctx.env_ref().option("extglob"),
                ctx.env_ref().option("nocasematch"),
                ctx.env_ref().option("globasciiranges"),
            )
        });
    }
    if let Some(prefix) = &spec.prefix {
        for m in out.iter_mut() {
            *m = format!("{prefix}{m}");
        }
    }
    if let Some(suffix) = &spec.suffix {
        for m in out.iter_mut() {
            *m = format!("{m}{suffix}");
        }
    }
    if spec.options.contains(CompOpts::PLUSDIRS)
        || (out.is_empty() && spec.options.contains(CompOpts::DIRNAMES))
    {
        out.extend(list_entries(
            word,
            true,
            word_quoted,
            home.as_deref(),
            false,
        ));
    }
    if out.is_empty() && spec.options.contains(CompOpts::BASHDEFAULT) {
        out.extend(bashdefault_compgen(ctx.env_ref(), word));
    }
    if out.is_empty() && spec.options.contains(CompOpts::DEFAULT) {
        out.extend(list_entries(
            word,
            false,
            word_quoted,
            home.as_deref(),
            false,
        ));
    }
    out
}

fn bashdefault_compgen(env: &dyn Environment, word: &str) -> Vec<String> {
    if let Some(prefix) = word.strip_prefix("${") {
        return env
            .iter_vars()
            .into_iter()
            .filter(|var| var.name.starts_with(prefix))
            .map(|var| format!("${{{}}}", var.name))
            .collect();
    }
    if let Some(prefix) = word.strip_prefix('$') {
        return env
            .iter_vars()
            .into_iter()
            .filter(|var| var.name.starts_with(prefix))
            .map(|var| format!("${}", var.name))
            .collect();
    }
    if let Some(prefix) = word.strip_prefix('~').filter(|value| !value.contains('/')) {
        return system_user_names()
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| format!("~{name}"))
            .collect();
    }
    if let Some(at) = word.rfind('@') {
        let prefix = &word[at + 1..];
        let source = env
            .get("HOSTFILE")
            .or_else(|| env.get("hostname_completion_file"));
        return hostname_names(source.as_deref())
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| format!("{}{}", &word[..=at], name))
            .collect();
    }
    Vec::new()
}

fn run_completion_function(ctx: &mut BuiltinCtx<'_>, func: &str, word: &str) -> (Vec<String>, i32) {
    bind_compgen_variables(ctx, false);
    ctx.env().set("COMP_CWORD", "-1".to_string());
    ctx.env().unset("COMP_WORDS");
    let status = if ctx.shell.function_get(func).is_some() {
        let source = format!(
            "{} {} {} ''",
            shell_quote(func),
            shell_quote("compgen"),
            shell_quote(word)
        );
        ctx.shell.run_source(&source)
    } else {
        report_diagnostic(
            ctx.env_ref(),
            "completion",
            &format!("function `{func}' not found"),
        );
        127
    };
    let matches = if status == 124 {
        Vec::new()
    } else {
        ctx.env_ref().get_array("COMPREPLY").unwrap_or_default()
    };
    unbind_compgen_variables(ctx, true);
    (matches, status)
}

fn run_completion_command(ctx: &mut BuiltinCtx<'_>, command: &str, word: &str) -> Vec<String> {
    bind_compgen_variables(ctx, true);
    ctx.env().unset("COMP_CWORD");
    ctx.env().unset("COMP_WORDS");
    let source = format!(
        "{command} {} {} ''",
        shell_quote("compgen"),
        shell_quote(word)
    );
    let output = ctx.shell.capture_source(&source).unwrap_or_default();
    unbind_compgen_variables(ctx, false);
    split_command_output(&String::from_utf8_lossy(&output))
}

fn bind_compgen_variables(ctx: &mut BuiltinCtx<'_>, exported: bool) {
    for (name, value) in [
        ("COMP_LINE", ""),
        ("COMP_POINT", "0"),
        ("COMP_TYPE", "0"),
        ("COMP_KEY", "0"),
    ] {
        ctx.env().set(name, value.to_string());
        if exported {
            ctx.env().export(name);
        }
    }
}

fn unbind_compgen_variables(ctx: &mut BuiltinCtx<'_>, compreply: bool) {
    for name in [
        "COMP_LINE",
        "COMP_POINT",
        "COMP_TYPE",
        "COMP_KEY",
        "COMP_CWORD",
        "COMP_WORDS",
    ] {
        ctx.env().unset(name);
    }
    if compreply {
        ctx.env().unset("COMPREPLY");
    }
}

fn split_command_output(output: &str) -> Vec<String> {
    let mut matches = Vec::new();
    let mut current = String::new();
    let mut chars = output.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'\n') {
            chars.next();
            current.push('\n');
        } else if ch == '\n' {
            if !current.is_empty() {
                matches.push(std::mem::take(&mut current));
            }
            while chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        matches.push(current);
    }
    matches
}

fn shell_quote(s: &str) -> String {
    let mut out = String::from("'");
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn action_compgen(
    ctx: &BuiltinCtx<'_>,
    action: CompAction,
    prefix: &str,
    quoted: bool,
    home: Option<&str>,
) -> Vec<String> {
    use CompAction::*;
    match action {
        File | Directory => list_entries(prefix, matches!(action, Directory), quoted, home, true),
        Alias => ctx
            .env_ref()
            .alias_iter()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| name.starts_with(prefix))
            .collect(),
        Binding => {
            let mut names = crate::bind::known_function_names()
                .iter()
                .copied()
                .filter(|name| {
                    ctx.env_ref().option("interactive") || !crate::bind::bash_line_function(name)
                })
                .filter(|name| name.starts_with(prefix))
                .map(str::to_string)
                .collect::<Vec<_>>();
            locale_sort(&mut names);
            names
        }
        Builtin | Enabled => {
            let mut names = crate::iter_builtins()
                .map(|builtin| builtin.name().to_string())
                .filter(|name| name.starts_with(prefix))
                .filter(|name| ctx.env_ref().builtin_enabled(name))
                .collect::<Vec<_>>();
            names.sort();
            names
        }
        Disabled => crate::iter_builtins()
            .map(|builtin| builtin.name().to_string())
            .filter(|name| name.starts_with(prefix))
            .filter(|name| !ctx.env_ref().builtin_enabled(name))
            .collect(),
        Command => command_names(ctx, prefix),
        Function => ctx
            .shell
            .function_names()
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .collect(),
        Variable | Export => ctx
            .env_ref()
            .iter_vars()
            .into_iter()
            .filter(|snap| snap.name.starts_with(prefix))
            .filter(|snap| matches!(action, Variable) || snap.attrs.contains(VarAttrs::EXPORT))
            .map(|snap| snap.name)
            .collect(),
        ArrayVar => ctx
            .env_ref()
            .iter_vars()
            .into_iter()
            .filter(|snap| snap.name.starts_with(prefix))
            .filter(|snap| matches!(snap.kind, VarKind::Indexed | VarKind::Assoc))
            .map(|snap| snap.name)
            .collect(),
        Keyword => static_names(SHELL_KEYWORDS.iter().copied(), prefix),
        SetOpt => static_names(crate::options::SET_OPTIONS.iter().map(|o| o.long), prefix),
        ShOpt => static_names(
            crate::shopt_table::SHOPT_OPTIONS.iter().map(|o| o.name),
            prefix,
        ),
        HelpTopic => static_names(HELP_TOPICS.iter().copied(), prefix),
        Group => filter_prefix(system_group_names(), prefix),
        HostName => {
            let source = ctx
                .env_ref()
                .get("HOSTFILE")
                .or_else(|| ctx.env_ref().get("hostname_completion_file"));
            filter_prefix(hostname_names(source.as_deref()), prefix)
        }
        Job | Running | Stopped => job_names(ctx.env_ref(), action, prefix),
        Service => matching_service_names(prefix),
        Signal => filter_prefix(signal_names(), prefix),
        User => filter_prefix(system_user_names(), prefix),
    }
}

fn filter_prefix<I>(names: I, prefix: &str) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    names
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .collect()
}

fn job_names(env: &dyn Environment, action: CompAction, prefix: &str) -> Vec<String> {
    let Some(table) = env.jobs_table() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for job in table.list().iter().rev() {
        if action == CompAction::Running && job.state != JobState::Running {
            continue;
        }
        if action == CompAction::Stopped && job.state != JobState::Stopped {
            continue;
        }
        let name = job
            .command_line
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if name.starts_with(prefix) {
            names.push(name.to_string());
        }
    }
    names
}

fn command_names(ctx: &BuiltinCtx<'_>, prefix: &str) -> Vec<String> {
    if prefix.contains('/') {
        return list_entries(prefix, false, false, None, false)
            .into_iter()
            .filter(|name| is_executable(std::path::Path::new(name)))
            .collect();
    }
    let mut out = Vec::new();
    out.extend(
        crate::iter_builtins()
            .map(|builtin| builtin.name().to_string())
            .filter(|name| name.starts_with(prefix))
            .filter(|name| ctx.env_ref().builtin_enabled(name)),
    );
    out.extend(static_names(SHELL_KEYWORDS.iter().copied(), prefix));
    out.extend(
        ctx.shell
            .function_names()
            .into_iter()
            .filter(|name| name.starts_with(prefix)),
    );
    out.extend(path_executables(ctx.env_ref(), prefix));
    out
}

fn static_names<I>(names: I, prefix: &str) -> Vec<String>
where
    I: IntoIterator<Item = &'static str>,
{
    names
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

static SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "{", "}", "!", "[[", "]]", "coproc",
];

static HELP_TOPICS: &[&str] = &[
    "!",
    "%",
    "(( ... ))",
    ".",
    ":",
    "[",
    "[[ ... ]]",
    "alias",
    "bg",
    "bind",
    "break",
    "builtin",
    "caller",
    "case",
    "cd",
    "command",
    "compgen",
    "complete",
    "compopt",
    "continue",
    "coproc",
    "declare",
    "dirs",
    "disown",
    "echo",
    "enable",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "for",
    "for ((",
    "function",
    "getopts",
    "hash",
    "help",
    "history",
    "if",
    "jobs",
    "kill",
    "let",
    "local",
    "logout",
    "mapfile",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "select",
    "set",
    "shift",
    "shopt",
    "source",
    "suspend",
    "test",
    "time",
    "times",
    "trap",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unset",
    "until",
    "variables",
    "wait",
    "while",
    "{ ... }",
];

fn path_executables(env: &dyn Environment, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let path = env.get("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(prefix) {
                continue;
            }
            if is_executable(&entry.path()) {
                out.push(name);
            }
        }
    }
    out
}

fn list_entries(
    prefix: &str,
    dirs_only: bool,
    quoted: bool,
    home: Option<&str>,
    include_hidden: bool,
) -> Vec<String> {
    let expanded_prefix = expand_completion_prefix(prefix, quoted, home);
    let listing_prefix = expanded_prefix
        .as_ref()
        .map(|expanded| expanded.listing.as_str())
        .unwrap_or(prefix);
    let (dir, file) = if let Some(idx) = listing_prefix.rfind('/') {
        (
            PathBuf::from(&listing_prefix[..=idx]),
            listing_prefix[idx + 1..].to_string(),
        )
    } else {
        (PathBuf::from("."), listing_prefix.to_string())
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if (!include_hidden && !file.starts_with('.') && name.starts_with('.'))
            || !name.starts_with(&file)
        {
            continue;
        }
        if dirs_only {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_dir() {
                continue;
            }
        }
        let rendered = if let Some(expanded) = &expanded_prefix {
            format!("{}{}", expanded.render_dir, name)
        } else if dir.as_os_str() == "." && !listing_prefix.starts_with("./") {
            name
        } else {
            format!("{}{}", dir.display(), name)
        };
        out.push(rendered);
    }
    out
}

fn has_glob_meta(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'*' | b'?' | b'['))
}

struct ExpandedPrefix {
    listing: String,
    render_dir: String,
}

fn expand_completion_prefix(
    prefix: &str,
    quoted: bool,
    home: Option<&str>,
) -> Option<ExpandedPrefix> {
    if quoted && prefix.starts_with("~/") {
        let home = home?;
        let suffix = &prefix[1..];
        return Some(ExpandedPrefix {
            listing: format!("{home}{suffix}"),
            render_dir: "~/".to_string(),
        });
    }
    if quoted && prefix.starts_with("$HOME/") {
        let home = home?;
        let suffix = &prefix["$HOME".len()..];
        return Some(ExpandedPrefix {
            listing: format!("{home}{suffix}"),
            render_dir: render_quoted_dir(prefix),
        });
    }
    if quoted && prefix.starts_with("${HOME}/") {
        let home = home?;
        let suffix = &prefix["${HOME}".len()..];
        return Some(ExpandedPrefix {
            listing: format!("{home}{suffix}"),
            render_dir: render_quoted_dir(prefix),
        });
    }
    None
}

fn render_quoted_dir(prefix: &str) -> String {
    prefix
        .rfind('/')
        .map(|idx| prefix[..=idx].to_string())
        .unwrap_or_else(|| prefix.to_string())
}

fn filter_matches(
    pat: &str,
    word: &str,
    s: &str,
    extglob: bool,
    nocasematch: bool,
    globasciiranges: bool,
) -> bool {
    let pat = replace_filter_ampersand(pat, word);
    if pat.starts_with("!(") {
        pattern_match(&pat, s, extglob, nocasematch, globasciiranges)
    } else if let Some(rest) = pat.strip_prefix('!') {
        !pattern_match(rest, s, extglob, nocasematch, globasciiranges)
    } else {
        pattern_match(&pat, s, extglob, nocasematch, globasciiranges)
    }
}

fn replace_filter_ampersand(pat: &str, word: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in pat.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '&' {
            out.push_str(word);
        } else {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

fn pattern_match(
    pat: &str,
    s: &str,
    extglob: bool,
    nocasematch: bool,
    globasciiranges: bool,
) -> bool {
    fnmatch(
        pat.as_bytes(),
        s.as_bytes(),
        GlobOpts {
            nocaseglob: nocasematch,
            extglob,
            globasciiranges,
        },
    )
}

pub struct Compopt;
pub static COMPOPT: Compopt = Compopt;
impl Builtin for Compopt {
    fn name(&self) -> &'static str {
        "compopt"
    }
    fn synopsis(&self) -> &'static str {
        "compopt [-o|+o option] [-DEI] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut set_opts = CompOpts::empty();
        let mut clear_opts = CompOpts::empty();
        let mut default_slot = false;
        let mut empty_slot = false;
        let mut initial_slot = false;
        let mut i = 0;
        while i < ctx.args.len() {
            let a = &ctx.args[i];
            if a == "--" {
                i += 1;
                break;
            }
            let Some(sign) = a.chars().next().filter(|ch| matches!(ch, '-' | '+')) else {
                break;
            };
            if a.len() == 1 {
                break;
            }
            let chars = a[1..].chars().collect::<Vec<_>>();
            let mut j = 0;
            while j < chars.len() {
                match chars[j] {
                    'D' if sign == '-' => default_slot = true,
                    'E' if sign == '-' => empty_slot = true,
                    'I' if sign == '-' => initial_slot = true,
                    'o' => {
                        let name = if j + 1 < chars.len() {
                            chars[j + 1..].iter().collect::<String>()
                        } else {
                            i += 1;
                            match ctx.args.get(i) {
                                Some(name) => name.clone(),
                                None => {
                                    report_diagnostic(
                                        ctx.env_ref(),
                                        "compopt",
                                        &format!("{sign}o: option requires an argument"),
                                    );
                                    eprintln!("compopt: usage: {}", self.synopsis());
                                    return 2;
                                }
                            }
                        };
                        let Some(bit) = CompOpts::parse(&name) else {
                            report_diagnostic(
                                ctx.env_ref(),
                                "compopt",
                                &format!("{name}: invalid option name"),
                            );
                            return 2;
                        };
                        if sign == '-' {
                            set_opts |= bit;
                        } else {
                            clear_opts |= bit;
                        }
                        break;
                    }
                    option => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "compopt",
                            &format!("-{option}: invalid option"),
                        );
                        eprintln!("compopt: usage: {}", self.synopsis());
                        return 2;
                    }
                }
                j += 1;
            }
            i += 1;
        }
        let targets = ctx.args[i..].to_vec();
        let slot = if default_slot {
            Some(CompSlot::Default)
        } else if empty_slot {
            Some(CompSlot::Empty)
        } else if initial_slot {
            Some(CompSlot::Initial)
        } else {
            None
        };
        if targets.is_empty() && slot.is_none() {
            if set_opts.is_empty() && clear_opts.is_empty() {
                if let Some((command, options)) = ctx.env_ref().completion_options_current() {
                    print_compopts(&command, options);
                    return 0;
                }
                report_diagnostic(
                    ctx.env_ref(),
                    "compopt",
                    "not currently executing completion function",
                );
                return 1;
            }
            if ctx.env().completion_options_update(set_opts, clear_opts) {
                return 0;
            }
            report_diagnostic(
                ctx.env_ref(),
                "compopt",
                "not currently executing completion function",
            );
            return 1;
        }
        let entries = if let Some(slot) = slot {
            vec![(slot, None, slot_name(slot).to_string())]
        } else {
            targets
                .into_iter()
                .map(|name| (CompSlot::Command, Some(name.clone()), name))
                .collect()
        };
        let mut status = 0;
        for (slot, key, display) in entries {
            if let Some(mut spec) = ctx.env_ref().compspec_get(slot, key.as_deref()) {
                if set_opts.is_empty() && clear_opts.is_empty() {
                    print_compopts(&display, spec.options);
                    continue;
                }
                spec.options |= set_opts;
                spec.options &= !clear_opts;
                ctx.env().compspec_set(slot, key.as_deref(), spec);
            } else {
                report_diagnostic(
                    ctx.env_ref(),
                    "compopt",
                    &format!("{display}: no completion specification"),
                );
                status = 1;
            }
        }
        status
    }
}

fn print_compopts(command: &str, options: CompOpts) {
    print!("compopt ");
    for (bit, name) in [
        (CompOpts::BASHDEFAULT, "bashdefault"),
        (CompOpts::DEFAULT, "default"),
        (CompOpts::DIRNAMES, "dirnames"),
        (CompOpts::FILENAMES, "filenames"),
        (CompOpts::FULLQUOTE, "fullquote"),
        (CompOpts::NOQUOTE, "noquote"),
        (CompOpts::NOSORT, "nosort"),
        (CompOpts::NOSPACE, "nospace"),
        (CompOpts::PLUSDIRS, "plusdirs"),
    ] {
        print!("{}o {name} ", if options.contains(bit) { '-' } else { '+' });
    }
    println!("{}", render_completion_name(command));
}
