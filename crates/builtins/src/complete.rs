//! `complete` / `compgen` / `compopt` builtins.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use cherubsh_common::completion::{CompAction, CompOpts, CompSlot, CompSpec};
use cherubsh_common::{Environment, VarAttrs, VarKind, W_QUOTED};
use cherubsh_expander::pattern::{fnmatch, GlobOpts};

use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx};

#[derive(Default)]
struct ParsedFlags {
    spec: CompSpec,
    remove: bool,
    print: bool,
    default: bool,
    initial: bool,
    empty: bool,
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

fn parse_complete_flags(args: &[String]) -> Result<(ParsedFlags, Vec<String>, usize), String> {
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
            while j < chars.len() {
                let c = chars[j];
                match c {
                    'r' => flags.remove = true,
                    'p' => flags.print = true,
                    'D' => flags.default = true,
                    'I' => flags.initial = true,
                    'E' => flags.empty = true,
                    'A' => {
                        let v = option_arg(args, &chars, &mut i, j, 'A')?;
                        let action =
                            CompAction::parse(&v).ok_or(format!("invalid action: {}", v))?;
                        flags.spec.actions.push(action);
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
                        let bit = CompOpts::parse(&name).ok_or(format!("invalid -o: {}", name))?;
                        flags.spec.options |= bit;
                        break;
                    }
                    other => {
                        if let Some(act) = short_to_action(other) {
                            flags.spec.actions.push(act);
                        } else {
                            return Err(format!("invalid option -{other}"));
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

pub struct Complete;
pub static COMPLETE: Complete = Complete;

impl Builtin for Complete {
    fn name(&self) -> &'static str {
        "complete"
    }
    fn synopsis(&self) -> &'static str {
        "complete [-abcdefgjksuv] [-pr] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] name [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let (flags, names, _) = match parse_complete_flags(ctx.args) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("cherubsh: complete: {e}");
                return 2;
            }
        };
        if flags.print {
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
            if flags.default || flags.initial || flags.empty {
                let slots = [
                    (flags.default, CompSlot::Default),
                    (flags.initial, CompSlot::Initial),
                    (flags.empty, CompSlot::Empty),
                ];
                for (_, slot) in slots.into_iter().filter(|(enabled, _)| *enabled) {
                    ctx.env().compspec_remove(slot, None);
                }
                return 0;
            }
            if names.is_empty() {
                let specs = ctx.env_ref().compspec_iter();
                for (slot, key, _) in specs {
                    ctx.env().compspec_remove(slot, key.as_deref());
                }
                return 0;
            }
            let had_any_specs = !ctx.env_ref().compspec_iter().is_empty();
            let mut status = 0;
            for n in &names {
                if !ctx.env().compspec_remove(CompSlot::Command, Some(n)) && had_any_specs {
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
        if flags.default {
            ctx.env().compspec_set(CompSlot::Default, None, flags.spec);
            return 0;
        }
        if flags.initial {
            ctx.env().compspec_set(CompSlot::Initial, None, flags.spec);
            return 0;
        }
        if flags.empty {
            ctx.env().compspec_set(CompSlot::Empty, None, flags.spec);
            return 0;
        }
        if names.is_empty() {
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
        CompSlot::Command => key.unwrap_or(""),
        CompSlot::Default => "-D",
        CompSlot::Initial => "-I",
        CompSlot::Empty => "-E",
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

pub struct Compgen;
pub static COMPGEN: Compgen = Compgen;
impl Builtin for Compgen {
    fn name(&self) -> &'static str {
        "compgen"
    }
    fn synopsis(&self) -> &'static str {
        "compgen [-abcdefgjksuv] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [word]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let (flags, rest, rest_start) = match parse_complete_flags(ctx.args) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("cherubsh: compgen: {e}");
                return 2;
            }
        };
        let word = rest.first().cloned().unwrap_or_default();
        let word_quoted = ctx
            .arg_flags
            .get(rest_start)
            .is_some_and(|flags| flags & W_QUOTED != 0);
        let matches = compgen_eval(ctx, &flags.spec, &word, word_quoted);
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
        for w in expand_wordlist(ctx.env(), words).split_whitespace() {
            if w.starts_with(word) {
                out.push(w.to_string());
            }
        }
    }
    let home = ctx.env_ref().get("HOME");
    for action in &spec.actions {
        out.extend(action_compgen(
            ctx,
            *action,
            word,
            word_quoted,
            home.as_deref(),
        ));
    }
    if let Some(pat) = &spec.glob_pattern {
        out.extend(glob_compgen(pat, word));
    }
    if let Some(filter) = &spec.filterpat {
        out.retain(|m| !filter_matches(filter, word, m, ctx.env_ref().option("extglob")));
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
    if out.is_empty() && spec.options.contains(CompOpts::DIRNAMES) {
        out.extend(list_entries(word, true, word_quoted, home.as_deref()));
    }
    if out.is_empty() && spec.options.contains(CompOpts::DEFAULT) {
        out.extend(list_entries(word, false, word_quoted, home.as_deref()));
    }
    if spec.options.contains(CompOpts::PLUSDIRS) {
        out.extend(list_entries(word, true, word_quoted, home.as_deref()));
    }
    if let Some(func) = &spec.function {
        run_completion_function(ctx, func, word);
        out.extend(ctx.env_ref().get_array("COMPREPLY").unwrap_or_default());
    }
    out
}

fn expand_wordlist(env: &mut dyn Environment, words: &str) -> String {
    let mut runner = cherubsh_expander::NullRunner::default();
    cherubsh_expander::expand_string_to_string(words, env, &mut runner)
        .unwrap_or_else(|_| words.to_string())
}

fn run_completion_function(ctx: &mut BuiltinCtx<'_>, func: &str, word: &str) {
    let env = ctx.env();
    env.set("COMP_LINE", word.to_string());
    env.set("COMP_POINT", word.len().to_string());
    env.set("COMP_TYPE", "0".to_string());
    env.set("COMP_KEY", "0".to_string());
    env.set("COMP_CWORD", "0".to_string());
    env.set_array("COMP_WORDS", vec![word.to_string()]);
    env.set_array("COMPREPLY", Vec::new());
    let _ = ctx.shell.run_source(&shell_quote(func));
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
        File | Directory => list_entries(prefix, matches!(action, Directory), quoted, home),
        Alias => ctx
            .env_ref()
            .alias_iter()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| name.starts_with(prefix))
            .collect(),
        Binding => static_names(BINDING_NAMES.iter().copied(), prefix),
        Builtin | Enabled => crate::iter_builtins()
            .map(|builtin| builtin.name().to_string())
            .filter(|name| name.starts_with(prefix))
            .filter(|name| ctx.env_ref().builtin_enabled(name))
            .collect(),
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
        _ => Vec::new(), // richer cases require Environment access; the
                         // runtime engine in crates/shell/src/completion.rs
                         // is the production path.
    }
}

fn command_names(ctx: &BuiltinCtx<'_>, prefix: &str) -> Vec<String> {
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

static BINDING_NAMES: &[&str] = &[
    "beginning-of-line",
    "end-of-line",
    "forward-char",
    "backward-char",
    "forward-word",
    "backward-word",
    "delete-char",
    "backward-delete-char",
    "self-insert",
    "tab-insert",
    "transpose-chars",
    "transpose-words",
    "upcase-word",
    "downcase-word",
    "capitalize-word",
    "kill-line",
    "backward-kill-line",
    "kill-word",
    "backward-kill-word",
    "unix-word-rubout",
    "unix-line-discard",
    "yank",
    "yank-pop",
    "previous-history",
    "next-history",
    "operate-and-get-next",
    "beginning-of-history",
    "end-of-history",
    "reverse-search-history",
    "forward-search-history",
    "accept-line",
    "complete",
    "possible-completions",
    "menu-complete",
    "menu-complete-backward",
    "undo",
    "revert-line",
    "clear-screen",
    "abort",
    "vi-movement-mode",
    "vi-insertion-mode",
    "vi-append-mode",
    "vi-append-eol",
];

fn path_executables(env: &dyn Environment, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let path = env.get("PATH").unwrap_or_default();
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
            let Ok(meta) = std::fs::metadata(entry.path()) else {
                continue;
            };
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                out.push(name);
            }
        }
    }
    out
}

fn list_entries(prefix: &str, dirs_only: bool, quoted: bool, home: Option<&str>) -> Vec<String> {
    let expanded_prefix = expand_tilde_prefix(prefix, quoted, home);
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
        if !name.starts_with(&file) {
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

fn glob_compgen(pat: &str, prefix: &str) -> Vec<String> {
    list_entries(pat, false, false, None)
        .into_iter()
        .filter(|m| m.starts_with(prefix))
        .collect()
}

struct ExpandedPrefix {
    listing: String,
    render_dir: String,
}

fn expand_tilde_prefix(prefix: &str, quoted: bool, home: Option<&str>) -> Option<ExpandedPrefix> {
    if quoted && prefix.starts_with("~/") {
        let home = home?;
        let suffix = &prefix[1..];
        return Some(ExpandedPrefix {
            listing: format!("{home}{suffix}"),
            render_dir: "~/".to_string(),
        });
    }
    None
}

fn filter_matches(pat: &str, word: &str, s: &str, extglob: bool) -> bool {
    let pat = replace_filter_ampersand(pat, word);
    if pat.starts_with("!(") {
        pattern_match(&pat, s, extglob)
    } else if let Some(rest) = pat.strip_prefix('!') {
        !pattern_match(rest, s, extglob)
    } else {
        pattern_match(&pat, s, extglob)
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

fn pattern_match(pat: &str, s: &str, extglob: bool) -> bool {
    fnmatch(
        pat.as_bytes(),
        s.as_bytes(),
        GlobOpts {
            extglob,
            ..GlobOpts::default()
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
        "compopt [-o|+o option] [-DE] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut set_opts = CompOpts::empty();
        let mut clear_opts = CompOpts::empty();
        let mut targets: Vec<String> = Vec::new();
        let mut default_slot = false;
        let mut empty_slot = false;
        let mut i = 0;
        while i < ctx.args.len() {
            let a = &ctx.args[i];
            match a.as_str() {
                "-D" => default_slot = true,
                "-E" => empty_slot = true,
                "-o" => {
                    i += 1;
                    let name = match ctx.args.get(i) {
                        Some(n) => n,
                        None => {
                            eprintln!("cherubsh: compopt: -o requires an argument");
                            return 2;
                        }
                    };
                    if let Some(bit) = CompOpts::parse(name) {
                        set_opts |= bit;
                    }
                }
                "+o" => {
                    i += 1;
                    let name = match ctx.args.get(i) {
                        Some(n) => n,
                        None => return 2,
                    };
                    if let Some(bit) = CompOpts::parse(name) {
                        clear_opts |= bit;
                    }
                }
                _ if a.starts_with('-') => {
                    eprintln!("cherubsh: compopt: invalid option {a}");
                    return 2;
                }
                _ => targets.push(a.clone()),
            }
            i += 1;
        }
        let slot = if default_slot {
            CompSlot::Default
        } else if empty_slot {
            CompSlot::Empty
        } else {
            CompSlot::Command
        };
        if targets.is_empty() && slot != CompSlot::Command {
            if let Some(mut spec) = ctx.env_ref().compspec_get(slot, None) {
                spec.options |= set_opts;
                spec.options &= !clear_opts;
                ctx.env().compspec_set(slot, None, spec);
            }
            return 0;
        }
        for name in targets {
            if let Some(mut spec) = ctx.env_ref().compspec_get(slot, Some(&name)) {
                spec.options |= set_opts;
                spec.options &= !clear_opts;
                ctx.env().compspec_set(slot, Some(&name), spec);
            }
        }
        0
    }
}
