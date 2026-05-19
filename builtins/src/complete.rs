//! `complete` / `compgen` / `compopt` builtins.

use cherubsh_common::completion::{CompAction, CompOpts, CompSlot, CompSpec};

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

fn parse_complete_flags(args: &[String]) -> Result<(ParsedFlags, Vec<String>), String> {
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
                        i += 1;
                        let v = args.get(i).cloned().ok_or("missing arg to -A")?;
                        let action =
                            CompAction::parse(&v).ok_or(format!("invalid action: {}", v))?;
                        flags.spec.actions.push(action);
                        break;
                    }
                    'F' => {
                        i += 1;
                        flags.spec.function =
                            Some(args.get(i).cloned().ok_or("missing arg to -F")?);
                        break;
                    }
                    'C' => {
                        i += 1;
                        flags.spec.command = Some(args.get(i).cloned().ok_or("missing arg to -C")?);
                        break;
                    }
                    'G' => {
                        i += 1;
                        flags.spec.glob_pattern =
                            Some(args.get(i).cloned().ok_or("missing arg to -G")?);
                        break;
                    }
                    'W' => {
                        i += 1;
                        flags.spec.wordlist =
                            Some(args.get(i).cloned().ok_or("missing arg to -W")?);
                        break;
                    }
                    'X' => {
                        i += 1;
                        flags.spec.filterpat =
                            Some(args.get(i).cloned().ok_or("missing arg to -X")?);
                        break;
                    }
                    'P' => {
                        i += 1;
                        flags.spec.prefix = Some(args.get(i).cloned().ok_or("missing arg to -P")?);
                        break;
                    }
                    'S' => {
                        i += 1;
                        flags.spec.suffix = Some(args.get(i).cloned().ok_or("missing arg to -S")?);
                        break;
                    }
                    'o' => {
                        i += 1;
                        let name = args.get(i).cloned().ok_or("missing arg to -o")?;
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
    Ok((flags, args[i..].to_vec()))
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
        let (flags, names) = match parse_complete_flags(ctx.args) {
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
        let (flags, rest) = match parse_complete_flags(ctx.args) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("cherubsh: compgen: {e}");
                return 2;
            }
        };
        let word = rest.first().cloned().unwrap_or_default();
        let matches = compgen_eval(&flags.spec, &word);
        if matches.is_empty() {
            return 1;
        }
        for m in matches {
            println!("{}", m);
        }
        0
    }
}

fn compgen_eval(spec: &CompSpec, word: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(words) = &spec.wordlist {
        for w in words.split_whitespace() {
            if w.starts_with(word) {
                out.push(w.to_string());
            }
        }
    }
    for action in &spec.actions {
        out.extend(action_compgen(*action, word));
    }
    if let Some(pat) = &spec.glob_pattern {
        out.extend(glob_compgen(pat, word));
    }
    if let Some(filter) = &spec.filterpat {
        out.retain(|m| !filter_matches(filter, m));
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
    if !spec.options.contains(CompOpts::NOSORT) {
        out.sort();
        out.dedup();
    }
    out
}

fn action_compgen(action: CompAction, prefix: &str) -> Vec<String> {
    use CompAction::*;
    match action {
        File | Directory => list_entries(prefix, matches!(action, Directory)),
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

fn list_entries(prefix: &str, dirs_only: bool) -> Vec<String> {
    let (dir, file) = if let Some(idx) = prefix.rfind('/') {
        (
            std::path::PathBuf::from(&prefix[..=idx]),
            prefix[idx + 1..].to_string(),
        )
    } else {
        (std::path::PathBuf::from("."), prefix.to_string())
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
        out.push(name);
    }
    out
}

fn glob_compgen(pat: &str, prefix: &str) -> Vec<String> {
    list_entries(pat, false)
        .into_iter()
        .filter(|m| m.starts_with(prefix))
        .collect()
}

fn filter_matches(pat: &str, s: &str) -> bool {
    if let Some(rest) = pat.strip_prefix('!') {
        !simple_glob(rest, s)
    } else {
        simple_glob(pat, s)
    }
}

fn simple_glob(pat: &str, s: &str) -> bool {
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
