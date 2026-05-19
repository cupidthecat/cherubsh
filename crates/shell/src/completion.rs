//! Programmable completion engine.
//!
//! Consumes `CompSpec` entries stored in `ShellState` and produces match
//! lists for the line editor. Bash `complete`/`compgen`/`compopt` builtins
//! mutate the spec table through the `Environment` surface; this module is
//! the runtime side that runs at Tab time.

use std::path::PathBuf;

use cherubsh_common::completion::{CompAction, CompOpts, CompSlot, CompSpec};
use cherubsh_common::Environment;
use cherubsh_lexer::Lexer;
use cherubsh_parser::Parser;

use crate::state::ShellState;

/// One completion request as the editor sees it.
pub struct CompRequest<'a> {
    pub line: &'a str,
    pub point: usize,
    pub words: Vec<String>,
    pub cword: usize,
    pub current: String,
}

/// Run a completion query, returning the candidate matches.
pub fn complete(state: &mut ShellState, req: &CompRequest<'_>) -> Vec<String> {
    let cmd = req.words.first().map(|s| s.as_str()).unwrap_or("");
    let spec = pick_spec(state, cmd, req).unwrap_or_default();
    run_spec(state, &spec, req)
}

fn pick_spec(state: &ShellState, cmd: &str, req: &CompRequest<'_>) -> Option<CompSpec> {
    if cmd.is_empty() {
        return state.compspec_get(CompSlot::Empty, None);
    }
    if let Some(s) = state.compspec_get(CompSlot::Command, Some(cmd)) {
        return Some(s);
    }
    if req.cword == 0 {
        if let Some(s) = state.compspec_get(CompSlot::Initial, None) {
            return Some(s);
        }
    }
    state.compspec_get(CompSlot::Default, None)
}

fn run_spec(state: &mut ShellState, spec: &CompSpec, req: &CompRequest<'_>) -> Vec<String> {
    let mut matches: Vec<String> = Vec::new();

    // -A actions
    for action in &spec.actions {
        matches.extend(action_matches(state, *action, &req.current));
    }
    // -G glob
    if let Some(pat) = &spec.glob_pattern {
        matches.extend(glob_matches(pat, &req.current));
    }
    // -W wordlist
    if let Some(words) = &spec.wordlist {
        matches.extend(wordlist_matches(words, &req.current));
    }
    // -F function
    if let Some(func) = &spec.function {
        matches.extend(function_matches(state, func, req));
    }
    // -C command
    if let Some(cmd) = &spec.command {
        matches.extend(command_matches(cmd, req));
    }
    // -X filter
    if let Some(filter) = &spec.filterpat {
        matches.retain(|m| !pattern_matches(filter, m));
    }
    // -P prefix / -S suffix
    if let Some(prefix) = &spec.prefix {
        for m in matches.iter_mut() {
            *m = format!("{prefix}{m}");
        }
    }
    if let Some(suffix) = &spec.suffix {
        for m in matches.iter_mut() {
            *m = format!("{m}{suffix}");
        }
    }
    // -o nosort: bash returns in spec order; otherwise sort+dedup.
    if !spec.options.contains(CompOpts::NOSORT) {
        matches.sort();
        matches.dedup();
    }
    matches
}

fn action_matches(state: &dyn Environment, action: CompAction, word: &str) -> Vec<String> {
    use CompAction::*;
    let prefix = word;
    match action {
        Alias => state
            .alias_iter()
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| k.starts_with(prefix))
            .collect(),
        Builtin | Enabled => cherubsh_builtins::iter_builtins()
            .map(|b| b.name().to_string())
            .filter(|n| n.starts_with(prefix))
            .filter(|n| state.builtin_enabled(n))
            .collect(),
        Disabled => cherubsh_builtins::iter_builtins()
            .map(|b| b.name().to_string())
            .filter(|n| n.starts_with(prefix))
            .filter(|n| !state.builtin_enabled(n))
            .collect(),
        Command => path_executables(state, prefix),
        Directory => list_dir_entries(prefix, true),
        File => list_dir_entries(prefix, false),
        Function => state
            .function_names()
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect(),
        Variable | Export => state
            .iter_vars()
            .into_iter()
            .filter(|s| s.name.starts_with(prefix))
            .filter(|s| {
                matches!(action, Variable) || s.attrs.contains(cherubsh_common::VarAttrs::EXPORT)
            })
            .map(|s| s.name)
            .collect(),
        ArrayVar => state
            .iter_vars()
            .into_iter()
            .filter(|s| s.name.starts_with(prefix))
            .filter(|s| {
                s.kind == cherubsh_common::VarKind::Indexed
                    || s.kind == cherubsh_common::VarKind::Assoc
            })
            .map(|s| s.name)
            .collect(),
        Keyword => SHELL_KEYWORDS
            .iter()
            .filter(|k| k.starts_with(prefix))
            .map(|k| (*k).to_string())
            .collect(),
        Signal => (1..32)
            .filter_map(crate::state::signal_short_name)
            .filter(|n| n.starts_with(prefix))
            .map(|n| n.to_string())
            .collect(),
        HostName => parse_hostnames()
            .into_iter()
            .filter(|h| h.starts_with(prefix))
            .collect(),
        User => parse_passwd_field(0)
            .into_iter()
            .filter(|u| u.starts_with(prefix))
            .collect(),
        Group => parse_passwd_field_groups()
            .into_iter()
            .filter(|g| g.starts_with(prefix))
            .collect(),
        Service => parse_services()
            .into_iter()
            .filter(|s| s.starts_with(prefix))
            .collect(),
        ShOpt => SHOPT_NAMES
            .iter()
            .filter(|n| n.starts_with(prefix))
            .map(|n| (*n).to_string())
            .collect(),
        SetOpt => SETOPT_NAMES
            .iter()
            .filter(|n| n.starts_with(prefix))
            .map(|n| (*n).to_string())
            .collect(),
        HelpTopic => cherubsh_builtins::iter_builtins()
            .map(|b| b.name().to_string())
            .filter(|n| n.starts_with(prefix))
            .collect(),
        Job | Running | Stopped => job_specs(state, action, prefix),
        Binding => Vec::new(), // populated from active keymap if needed
    }
}

static SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "{", "}", "!", "[[", "]]", "coproc",
];

static SHOPT_NAMES: &[&str] = &[
    "autocd",
    "cdable_vars",
    "cdspell",
    "checkhash",
    "checkjobs",
    "checkwinsize",
    "cmdhist",
    "compat31",
    "complete_fullquote",
    "direxpand",
    "dirspell",
    "dotglob",
    "execfail",
    "expand_aliases",
    "extdebug",
    "extglob",
    "extquote",
    "failglob",
    "force_fignore",
    "globasciiranges",
    "globskipdots",
    "globstar",
    "gnu_errfmt",
    "histappend",
    "histreedit",
    "histverify",
    "hostcomplete",
    "huponexit",
    "inherit_errexit",
    "interactive_comments",
    "lastpipe",
    "lithist",
    "localvar_inherit",
    "localvar_unset",
    "login_shell",
    "mailwarn",
    "no_empty_cmd_completion",
    "nocaseglob",
    "nocasematch",
    "nullglob",
    "patsub_replacement",
    "progcomp",
    "promptvars",
    "restricted_shell",
    "shift_verbose",
    "sourcepath",
    "varredir_close",
    "xpg_echo",
];

static SETOPT_NAMES: &[&str] = &[
    "allexport",
    "braceexpand",
    "emacs",
    "errexit",
    "errtrace",
    "functrace",
    "hashall",
    "histexpand",
    "history",
    "ignoreeof",
    "interactive-comments",
    "keyword",
    "monitor",
    "noclobber",
    "noexec",
    "noglob",
    "nolog",
    "notify",
    "nounset",
    "onecmd",
    "physical",
    "pipefail",
    "posix",
    "privileged",
    "verbose",
    "vi",
    "xtrace",
];

fn path_executables(state: &dyn Environment, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let path = state.get("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(prefix) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                use std::os::unix::fs::PermissionsExt;
                if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                    out.push(name);
                }
            }
        }
    }
    out
}

fn list_dir_entries(prefix: &str, dirs_only: bool) -> Vec<String> {
    let (dir_part, file_part) = split_dir_prefix(prefix);
    let Ok(entries) = std::fs::read_dir(dir_part.as_path()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&file_part) {
            continue;
        }
        if dirs_only {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_dir() {
                continue;
            }
        }
        let rendered = if dir_part.as_os_str() == "." && !prefix.starts_with("./") {
            name
        } else {
            format!("{}{}", dir_part.display(), name)
        };
        out.push(rendered);
    }
    out
}

fn split_dir_prefix(prefix: &str) -> (PathBuf, String) {
    if let Some(idx) = prefix.rfind('/') {
        (
            PathBuf::from(&prefix[..=idx]),
            prefix[idx + 1..].to_string(),
        )
    } else {
        (PathBuf::from("."), prefix.to_string())
    }
}

fn glob_matches(pat: &str, prefix: &str) -> Vec<String> {
    let (dir_part, _file_part) = split_dir_prefix(pat);
    // Simple glob with `*` and `?`. Use the expander's glob facility if
    // accessible; fall back to a basic implementation here.
    let Ok(entries) = std::fs::read_dir(dir_part.as_path()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if simple_glob(pat, &name) && name.starts_with(prefix) {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

fn simple_glob(pat: &str, s: &str) -> bool {
    // Subset of glob: *, ?, literal. Sufficient for `-G` filtering.
    fn helper(p: &[u8], s: &[u8]) -> bool {
        if p.is_empty() {
            return s.is_empty();
        }
        match p[0] {
            b'*' => {
                if helper(&p[1..], s) {
                    return true;
                }
                if !s.is_empty() && helper(p, &s[1..]) {
                    return true;
                }
                false
            }
            b'?' => !s.is_empty() && helper(&p[1..], &s[1..]),
            c => !s.is_empty() && s[0] == c && helper(&p[1..], &s[1..]),
        }
    }
    helper(pat.as_bytes(), s.as_bytes())
}

fn wordlist_matches(list: &str, prefix: &str) -> Vec<String> {
    list.split_whitespace()
        .filter(|w| w.starts_with(prefix))
        .map(|w| w.to_string())
        .collect()
}

fn function_matches(state: &mut ShellState, func: &str, req: &CompRequest<'_>) -> Vec<String> {
    // Set COMP_LINE/POINT/WORDS/CWORD/TYPE/KEY then call the function;
    // read COMPREPLY array on return.
    use cherubsh_common::Environment;
    state.set("COMP_LINE", req.line.to_string());
    state.set("COMP_POINT", req.point.to_string());
    state.set("COMP_TYPE", "9".to_string()); // 9 = TAB
    state.set("COMP_KEY", "9".to_string());
    state.set("COMP_CWORD", req.cword.to_string());
    state.set_array("COMP_WORDS", req.words.clone());
    state.set_array("COMPREPLY", Vec::new());

    let cmd_line = format!("{} {}", func, shell_quote(&req.current));
    let mut lexer = Lexer::new(&cmd_line);
    lexer.set_extglob_patterns(state.option("extglob"));
    lexer.set_posix_mode(state.option("posix"));
    let mut tokens = Vec::new();
    while let Some(t) = lexer.next_token() {
        tokens.push(t);
    }
    let mut parser = Parser::new(tokens, &cmd_line);
    if let Ok(ast) = parser.parse() {
        let _ = cherubsh_exec::execute_in(&cherubsh_parser::Ast { root: ast.root }, state);
    }
    state.get_array("COMPREPLY").unwrap_or_default()
}

fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn command_matches(cmd: &str, _req: &CompRequest<'_>) -> Vec<String> {
    let output = std::process::Command::new("sh").arg("-c").arg(cmd).output();
    let Ok(o) = output else { return Vec::new() };
    String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

fn pattern_matches(pat: &str, s: &str) -> bool {
    // `-X` filterpat - bash uses glob semantics with leading `!` negation.
    if let Some(rest) = pat.strip_prefix('!') {
        !simple_glob(rest, s)
    } else {
        simple_glob(pat, s)
    }
}

fn parse_hostnames() -> Vec<String> {
    // Read /etc/hosts (bash falls back to readline hostname list - same idea).
    let Ok(contents) = std::fs::read_to_string("/etc/hosts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in contents.lines() {
        if let Some(stripped) = line.split('#').next() {
            for tok in stripped.split_whitespace().skip(1) {
                out.push(tok.to_string());
            }
        }
    }
    out
}

fn parse_passwd_field(field: usize) -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string("/etc/passwd") else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|l| l.split(':').nth(field).map(|s| s.to_string()))
        .collect()
}

fn parse_passwd_field_groups() -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string("/etc/group") else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|l| l.split(':').next().map(|s| s.to_string()))
        .collect()
}

fn parse_services() -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string("/etc/services") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in contents.lines() {
        if let Some(name) = line.split_whitespace().next() {
            if !name.starts_with('#') {
                out.push(name.to_string());
            }
        }
    }
    out
}

fn job_specs(env: &dyn Environment, filter: CompAction, prefix: &str) -> Vec<String> {
    let Some(table) = env.jobs_table() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for job in table.list() {
        if filter == CompAction::Running && job.state != cherubsh_common::JobState::Running {
            continue;
        }
        if filter == CompAction::Stopped && job.state != cherubsh_common::JobState::Stopped {
            continue;
        }
        let label = format!("%{}", job.id.raw());
        if label.starts_with(prefix) {
            out.push(label);
        }
    }
    out
}
