use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cherubsh_common::completion::{
    hostname_names, locale_sort, matching_service_names, signal_names, system_group_names,
    system_user_names, CompAction, CompOpts, CompSlot, CompSpec,
};
use cherubsh_common::{Environment, JobState, VarAttrs, VarKind};
use cherubsh_exec::ExecState;
use cherubsh_expander::pattern::{fnmatch, GlobOpts};
use cherubsh_lineedit::Completion;

use crate::state::ShellState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompletionQuote {
    #[default]
    None,
    Single,
    Double,
}

pub struct CompRequest<'a> {
    pub line: &'a str,
    pub point: usize,
    pub words: Vec<String>,
    pub cword: usize,
    pub command: String,
    pub current: String,
    pub previous: String,
    pub replace_start: usize,
    pub quote: CompletionQuote,
}

struct SpecRun {
    matches: Vec<String>,
    filenames: bool,
    retry: bool,
}

pub fn complete(
    state: &mut ShellState,
    exec_state: &mut ExecState,
    req: &CompRequest<'_>,
) -> Completion {
    let mut matches = Vec::new();
    let mut filenames = false;
    let mut options = CompOpts::empty();
    let mut used_spec = false;

    if state.option("progcomp") {
        for _ in 0..32 {
            let Some(spec) = pick_spec(state, req) else {
                break;
            };
            used_spec = true;
            let saved_options = state.active_completion_options.take();
            let saved_command = state.active_completion_command.take();
            state.active_completion_options = Some(spec.options);
            state.active_completion_command = Some(req.command.clone());
            let run = run_spec(state, exec_state, &spec, req);
            options = state
                .active_completion_options
                .take()
                .unwrap_or(spec.options);
            state.active_completion_options = saved_options;
            state.active_completion_command = saved_command;
            matches = run.matches;
            filenames = run.filenames || options.contains(CompOpts::FILENAMES);
            if !run.retry {
                break;
            }
        }
    }

    if used_spec {
        if matches.is_empty() && options.contains(CompOpts::DIRNAMES) {
            matches = file_matches(state, &req.current, true);
            filenames = true;
        } else if options.contains(CompOpts::PLUSDIRS) {
            matches.extend(file_matches(state, &req.current, true));
            filenames = true;
        }
        if matches.is_empty() && options.contains(CompOpts::BASHDEFAULT) {
            let fallback = bash_default_matches(state, exec_state, req);
            filenames |= fallback.1;
            matches = fallback.0;
        }
        if matches.is_empty() && options.contains(CompOpts::DEFAULT) {
            matches = file_matches(state, &req.current, false);
            filenames = true;
        }
    } else if !(req.command.is_empty()
        && req.current.is_empty()
        && state.option("no_empty_cmd_completion"))
    {
        let fallback = bash_default_matches(state, exec_state, req);
        matches = fallback.0;
        filenames = fallback.1;
    }

    if filenames {
        matches = apply_fignore(state, matches);
    }
    normalize_matches(&mut matches, options.contains(CompOpts::NOSORT));
    let explicit_space =
        options.contains(CompOpts::NOSPACE) && matches.len() == 1 && matches[0].ends_with(' ');
    if explicit_space {
        matches[0].pop();
    }
    let quote_matches = options.contains(CompOpts::FULLQUOTE)
        || !options.contains(CompOpts::NOQUOTE)
            && (filenames || state.option("complete_fullquote"));
    let single_match = matches.len() == 1;
    if quote_matches {
        for item in &mut matches {
            *item = quote_match(item, req.quote, single_match);
        }
    }

    Completion {
        matches,
        replace_start: req.replace_start,
        suppress_append: options.contains(CompOpts::NOSPACE) && !explicit_space,
        append_character: Some(' '),
        filenames,
    }
}

fn pick_spec(state: &ShellState, req: &CompRequest<'_>) -> Option<CompSpec> {
    if req.command.is_empty() {
        if req.line[..req.point].trim().is_empty() {
            if let Some(spec) = state.compspec_get(CompSlot::Empty, None) {
                return Some(spec);
            }
        }
        return state.compspec_get(CompSlot::Initial, None);
    }
    if req.cword == 0 {
        if let Some(spec) = state.compspec_get(CompSlot::Initial, None) {
            return Some(spec);
        }
    }
    if let Some(spec) = state.compspec_get(CompSlot::Command, Some(&req.command)) {
        return Some(spec);
    }
    if let Some(base) = Path::new(&req.command)
        .file_name()
        .and_then(|name| name.to_str())
    {
        if base != req.command {
            if let Some(spec) = state.compspec_get(CompSlot::Command, Some(base)) {
                return Some(spec);
            }
        }
    }
    if state.option("progcomp_alias") {
        let mut command = req.command.clone();
        let mut seen = HashSet::new();
        for _ in 0..32 {
            if !seen.insert(command.clone()) {
                break;
            }
            let Some(alias) = state.alias_get(&command) else {
                break;
            };
            let Some(next) = first_shell_word(&alias) else {
                break;
            };
            command = next;
            if let Some(spec) = state.compspec_get(CompSlot::Command, Some(&command)) {
                return Some(spec);
            }
        }
    }
    state.compspec_get(CompSlot::Default, None)
}

fn run_spec(
    state: &mut ShellState,
    exec_state: &mut ExecState,
    spec: &CompSpec,
    req: &CompRequest<'_>,
) -> SpecRun {
    let mut matches = Vec::new();
    let mut filenames = false;
    for action in CompAction::GENERATION_ORDER
        .into_iter()
        .filter(|action| spec.actions.contains(action))
    {
        matches.extend(action_matches(state, exec_state, action, &req.current));
        filenames |= matches!(action, CompAction::File | CompAction::Directory);
    }
    if let Some(pattern) = &spec.glob_pattern {
        matches.extend(glob_matches(state, exec_state, pattern));
        filenames = true;
    }
    if let Some(words) = &spec.wordlist {
        matches.extend(wordlist_matches(state, exec_state, words, &req.current));
    }
    let mut retry = false;
    if let Some(function) = &spec.function {
        let (function_matches, status) = function_matches(state, exec_state, function, req);
        matches.extend(function_matches);
        retry = status == 124;
    }
    if let Some(command) = &spec.command {
        matches.extend(command_matches(state, exec_state, command, req));
    }
    if let Some(filter) = &spec.filterpat {
        matches.retain(|item| !filter_matches(state, filter, &req.current, item));
    }
    if let Some(prefix) = &spec.prefix {
        for item in &mut matches {
            item.insert_str(0, prefix);
        }
    }
    if let Some(suffix) = &spec.suffix {
        for item in &mut matches {
            item.push_str(suffix);
        }
    }
    SpecRun {
        matches,
        filenames,
        retry,
    }
}

fn action_matches(
    state: &ShellState,
    exec_state: &ExecState,
    action: CompAction,
    prefix: &str,
) -> Vec<String> {
    use CompAction::*;
    match action {
        Alias => filter_prefix(state.alias_iter().into_iter().map(|(name, _)| name), prefix),
        ArrayVar => filter_prefix(
            state
                .iter_vars()
                .into_iter()
                .filter(|var| matches!(var.kind, VarKind::Indexed | VarKind::Assoc))
                .map(|var| var.name),
            prefix,
        ),
        Binding => filter_prefix(
            cherubsh_builtins::bind::known_function_names()
                .iter()
                .filter(|name| {
                    state.interactive || !cherubsh_builtins::bind::bash_line_function(name)
                })
                .map(|name| (*name).to_string()),
            prefix,
        ),
        Builtin => filter_prefix(
            cherubsh_builtins::iter_builtins().map(|item| item.name().to_string()),
            prefix,
        ),
        Enabled => filter_prefix(
            cherubsh_builtins::iter_builtins()
                .filter(|item| state.builtin_enabled(item.name()))
                .map(|item| item.name().to_string()),
            prefix,
        ),
        Disabled => filter_prefix(
            cherubsh_builtins::iter_builtins()
                .filter(|item| !state.builtin_enabled(item.name()))
                .map(|item| item.name().to_string()),
            prefix,
        ),
        Command => command_names(state, exec_state, prefix),
        Directory => file_matches(state, prefix, true),
        Export => filter_prefix(
            state
                .iter_vars()
                .into_iter()
                .filter(|var| var.attrs.contains(VarAttrs::EXPORT))
                .map(|var| var.name),
            prefix,
        ),
        File => file_matches(state, prefix, false),
        Function => filter_prefix(exec_state.function_names(), prefix),
        Group => filter_prefix(system_group_names(), prefix),
        HelpTopic => filter_prefix(HELP_TOPICS.iter().map(|name| (*name).to_string()), prefix),
        HostName => filter_prefix(completion_hostnames(state), prefix),
        Job | Running | Stopped => job_names(state, action, prefix),
        Keyword => filter_prefix(
            SHELL_KEYWORDS.iter().map(|name| (*name).to_string()),
            prefix,
        ),
        Service => matching_service_names(prefix),
        SetOpt => filter_prefix(
            cherubsh_builtins::options::SET_OPTIONS
                .iter()
                .map(|option| option.long.to_string()),
            prefix,
        ),
        ShOpt => filter_prefix(
            cherubsh_builtins::shopt_table::SHOPT_OPTIONS
                .iter()
                .map(|option| option.name.to_string()),
            prefix,
        ),
        Signal => filter_prefix(signal_names(), prefix),
        User => filter_prefix(system_user_names(), prefix),
        Variable => filter_prefix(state.iter_vars().into_iter().map(|var| var.name), prefix),
    }
}

fn command_names(state: &ShellState, exec_state: &ExecState, prefix: &str) -> Vec<String> {
    if prefix.contains('/') {
        return file_matches(state, prefix, false)
            .into_iter()
            .filter(|name| {
                let path = expand_tilde_path(state, name);
                std::fs::metadata(path)
                    .map(|metadata| {
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    })
                    .unwrap_or(false)
            })
            .collect();
    }
    let mut out = Vec::new();
    out.extend(state.alias_iter().into_iter().map(|(name, _)| name));
    out.extend(
        cherubsh_builtins::iter_builtins()
            .filter(|item| state.builtin_enabled(item.name()))
            .map(|item| item.name().to_string()),
    );
    out.extend(exec_state.function_names());
    out.extend(SHELL_KEYWORDS.iter().map(|name| (*name).to_string()));
    out.extend(path_executables(state));
    filter_prefix(out, prefix)
}

fn bash_default_matches(
    state: &ShellState,
    exec_state: &ExecState,
    req: &CompRequest<'_>,
) -> (Vec<String>, bool) {
    if req.current.starts_with('$') {
        let prefix = req.current.trim_start_matches('$');
        let matches = filter_prefix(
            state
                .iter_vars()
                .into_iter()
                .map(|var| format!("${}", var.name)),
            &format!("${prefix}"),
        );
        return (matches, false);
    }
    if let Some(prefix) = req
        .current
        .strip_prefix('~')
        .filter(|text| !text.contains('/'))
    {
        let matches = filter_prefix(system_user_names(), prefix)
            .into_iter()
            .map(|name| format!("~{name}/"))
            .collect();
        return (matches, true);
    }
    if state.option("hostcomplete") {
        if let Some(at) = req.current.rfind('@') {
            let host_prefix = &req.current[at + 1..];
            let head = &req.current[..=at];
            let matches = filter_prefix(completion_hostnames(state), host_prefix)
                .into_iter()
                .map(|host| format!("{head}{host}"))
                .collect();
            return (matches, false);
        }
    }
    if req.cword == 0 {
        return (command_names(state, exec_state, &req.current), false);
    }
    (file_matches(state, &req.current, false), true)
}

fn wordlist_matches(
    state: &mut ShellState,
    exec_state: &mut ExecState,
    words: &str,
    prefix: &str,
) -> Vec<String> {
    let expanded = exec_state
        .expand_words_source(words, state)
        .unwrap_or_else(|_| split_quoted_words(words));
    filter_prefix(expanded, prefix)
}

fn glob_matches(state: &mut ShellState, exec_state: &mut ExecState, pattern: &str) -> Vec<String> {
    exec_state
        .expand_words_source(pattern, state)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item != pattern || !has_glob_meta(pattern))
        .collect()
}

fn function_matches(
    state: &mut ShellState,
    exec_state: &mut ExecState,
    function: &str,
    req: &CompRequest<'_>,
) -> (Vec<String>, i32) {
    bind_completion_variables(state, req, false);
    state.set_array("COMPREPLY", Vec::new());
    let source = format!(
        "{} {} {} {}",
        shell_quote(function),
        shell_quote(&req.command),
        shell_quote(&req.current),
        shell_quote(&req.previous)
    );
    let status = exec_state
        .execute_source(&source, state)
        .map(|result| result.status)
        .unwrap_or(127);
    let matches = state.get_array("COMPREPLY").unwrap_or_default();
    unbind_completion_variables(state, true);
    (matches, status)
}

fn command_matches(
    state: &mut ShellState,
    exec_state: &mut ExecState,
    command: &str,
    req: &CompRequest<'_>,
) -> Vec<String> {
    bind_completion_variables(state, req, true);
    let source = format!(
        "{} {} {} {}",
        command,
        shell_quote(&req.command),
        shell_quote(&req.current),
        shell_quote(&req.previous)
    );
    let output = exec_state
        .capture_subshell(&source, state)
        .unwrap_or_default();
    unbind_completion_variables(state, false);
    split_command_output(&String::from_utf8_lossy(&output))
}

fn bind_completion_variables(state: &mut ShellState, req: &CompRequest<'_>, exported: bool) {
    let point_chars = req.line[..req.point].chars().count();
    for (name, value) in [
        ("COMP_LINE", req.line.to_string()),
        ("COMP_POINT", point_chars.to_string()),
        ("COMP_TYPE", "9".to_string()),
        ("COMP_KEY", "9".to_string()),
    ] {
        state.set(name, value);
        if exported {
            state.export(name);
        }
    }
    if !exported {
        state.set("COMP_CWORD", req.cword.to_string());
        state.set_array("COMP_WORDS", req.words.clone());
    }
}

fn unbind_completion_variables(state: &mut ShellState, compreply: bool) {
    for name in [
        "COMP_LINE",
        "COMP_POINT",
        "COMP_TYPE",
        "COMP_KEY",
        "COMP_CWORD",
        "COMP_WORDS",
    ] {
        state.unset(name);
    }
    if compreply {
        state.unset("COMPREPLY");
    }
}

fn split_command_output(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = output.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'\n') {
            chars.next();
            current.push('\n');
        } else if ch == '\n' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            while chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn file_matches(state: &ShellState, prefix: &str, directories_only: bool) -> Vec<String> {
    let (display_dir, file_prefix) = split_path_prefix(prefix);
    let scan_dir = expand_tilde_path(state, &display_dir);
    let Ok(entries) = std::fs::read_dir(&scan_dir) else {
        return Vec::new();
    };
    let show_dot = file_prefix.starts_with('.');
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (!show_dot && name.starts_with('.')) || !name.starts_with(&file_prefix) {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        if directories_only && !is_dir {
            continue;
        }
        let mut rendered = if display_dir == "." && !prefix.starts_with("./") {
            name
        } else {
            format!("{display_dir}{name}")
        };
        if is_dir {
            rendered.push('/');
        }
        out.push(rendered);
    }
    out
}

fn split_path_prefix(prefix: &str) -> (String, String) {
    match prefix.rfind('/') {
        Some(index) => (
            prefix[..=index].to_string(),
            prefix[index + 1..].to_string(),
        ),
        None => (".".to_string(), prefix.to_string()),
    }
}

fn expand_tilde_path(state: &ShellState, path: &str) -> PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = state.get("HOME") {
            return PathBuf::from(home).join(path.trim_start_matches("~/"));
        }
    }
    if let Some(rest) = path.strip_prefix('~') {
        let (user, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some(home) = passwd_home(user) {
            return PathBuf::from(home).join(tail);
        }
    }
    PathBuf::from(path)
}

fn passwd_home(user: &str) -> Option<String> {
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            (fields.first().copied() == Some(user))
                .then(|| fields.get(5).copied().unwrap_or_default().to_string())
        })
}

fn path_executables(state: &ShellState) -> Vec<String> {
    let mut out = Vec::new();
    for directory in state.get("PATH").unwrap_or_default().split(':') {
        let directory = if directory.is_empty() { "." } else { directory };
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                out.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    out
}

fn apply_fignore(state: &ShellState, matches: Vec<String>) -> Vec<String> {
    let suffixes: Vec<String> = state
        .get("FIGNORE")
        .unwrap_or_default()
        .split(':')
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_string)
        .collect();
    if suffixes.is_empty() {
        return matches;
    }
    let filtered: Vec<String> = matches
        .iter()
        .filter(|item| item.ends_with('/') || !suffixes.iter().any(|suffix| item.ends_with(suffix)))
        .cloned()
        .collect();
    if filtered.is_empty() && !state.option("force_fignore") {
        matches
    } else {
        filtered
    }
}

fn filter_matches(state: &ShellState, pattern: &str, word: &str, item: &str) -> bool {
    let pattern = replace_filter_ampersand(pattern, word);
    let extglob = state.option("extglob");
    let (negated, pattern) = if pattern.starts_with("!(") && extglob {
        (false, pattern.as_str())
    } else if let Some(rest) = pattern.strip_prefix('!') {
        (true, rest)
    } else {
        (false, pattern.as_str())
    };
    let matched = fnmatch(
        pattern.as_bytes(),
        item.as_bytes(),
        GlobOpts {
            nocaseglob: state.option("nocasematch"),
            extglob,
            globasciiranges: state.option("globasciiranges"),
        },
    );
    if negated {
        !matched
    } else {
        matched
    }
}

fn replace_filter_ampersand(pattern: &str, word: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in pattern.chars() {
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

fn completion_hostnames(state: &ShellState) -> Vec<String> {
    let source = state
        .get("HOSTFILE")
        .or_else(|| state.get("hostname_completion_file"));
    hostname_names(source.as_deref())
}

fn job_names(state: &ShellState, action: CompAction, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for job in state.jobs.list().iter().rev() {
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
            out.push(name.to_string());
        }
    }
    out
}

fn filter_prefix<I>(items: I, prefix: &str) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    items
        .into_iter()
        .filter(|item| item.starts_with(prefix))
        .collect()
}

fn normalize_matches(matches: &mut Vec<String>, preserve_order: bool) {
    if preserve_order {
        let mut seen = HashSet::new();
        matches.retain(|item| seen.insert(item.clone()));
    } else {
        locale_sort(matches);
        matches.dedup();
    }
}

fn quote_match(item: &str, quote: CompletionQuote, close_quote: bool) -> String {
    match quote {
        CompletionQuote::Single => {
            let mut out = item.replace('\'', "'\\''");
            if close_quote {
                out.push('\'');
            }
            out
        }
        CompletionQuote::Double => {
            let mut out = String::new();
            for ch in item.chars() {
                if matches!(ch, '$' | '`' | '"' | '\\') {
                    out.push('\\');
                }
                out.push(ch);
            }
            if close_quote {
                out.push('"');
            }
            out
        }
        CompletionQuote::None => {
            let mut out = String::new();
            for ch in item.chars() {
                if ch.is_whitespace()
                    || matches!(
                        ch,
                        '\\' | '\''
                            | '"'
                            | '`'
                            | '$'
                            | '@'
                            | '>'
                            | '<'
                            | '='
                            | ';'
                            | '|'
                            | '&'
                            | '('
                            | ')'
                            | ':'
                            | '*'
                            | '?'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '!'
                    )
                {
                    out.push('\\');
                }
                out.push(ch);
            }
            out
        }
    }
}

fn shell_quote(value: &str) -> String {
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn first_shell_word(value: &str) -> Option<String> {
    split_quoted_words(value).into_iter().next()
}

fn split_quoted_words(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' && !single {
            escaped = true;
        } else if ch == '\'' && !double {
            single = !single;
        } else if ch == '"' && !single {
            double = !double;
        } else if ch.is_whitespace() && !single && !double {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn has_glob_meta(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'*' | b'?' | b'['))
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
];

#[cfg(test)]
mod tests {
    use super::{
        complete, quote_match, replace_filter_ampersand, split_command_output, CompRequest,
        CompletionQuote,
    };
    use crate::state::ShellState;
    use cherubsh_common::completion::{CompSlot, CompSpec};
    use cherubsh_common::Environment;
    use cherubsh_exec::ExecState;

    #[test]
    fn command_output_is_split_only_at_unescaped_newlines() {
        assert_eq!(
            split_command_output("one two\nthree\\\nfour\n"),
            vec!["one two", "three\nfour"]
        );
    }

    #[test]
    fn filter_ampersands_honor_backslashes() {
        assert_eq!(replace_filter_ampersand("*&*\\&", "txt"), "*txt*&");
    }

    #[test]
    fn quoted_completion_closes_a_unique_open_quote() {
        assert_eq!(
            quote_match("two words", CompletionQuote::Double, true),
            "two words\""
        );
        assert_eq!(
            quote_match("it's", CompletionQuote::Single, true),
            "it'\\''s'"
        );
    }

    #[test]
    fn completion_function_runs_in_the_live_execution_state() {
        let mut state = ShellState::default();
        let mut exec_state = ExecState::default();
        exec_state
            .execute_source("function _fruit() { COMPREPLY=(banana band); }", &mut state)
            .unwrap();
        state.compspec_set(
            CompSlot::Command,
            Some("cmd"),
            CompSpec {
                function: Some("_fruit".to_string()),
                ..CompSpec::default()
            },
        );
        let request = CompRequest {
            line: "cmd ba",
            point: 6,
            words: vec!["cmd".to_string(), "ba".to_string()],
            cword: 1,
            command: "cmd".to_string(),
            current: "ba".to_string(),
            previous: "cmd".to_string(),
            replace_start: 4,
            quote: CompletionQuote::None,
        };
        let result = complete(&mut state, &mut exec_state, &request);
        assert_eq!(result.matches, ["banana", "band"]);
    }
}
