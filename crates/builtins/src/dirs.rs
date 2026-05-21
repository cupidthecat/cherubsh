use std::path::Path;

use cherubsh_common::Environment;

use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx};

pub struct Dirs;
pub static DIRS: Dirs = Dirs;

#[derive(Clone, Copy)]
pub enum StackIndex {
    Left(usize),
    Right(usize),
}

pub fn resolve_index(len: usize, index: StackIndex) -> Option<usize> {
    match index {
        StackIndex::Left(n) if n < len => Some(n),
        StackIndex::Right(n) if n < len => Some(len - 1 - n),
        _ => None,
    }
}

impl Builtin for Dirs {
    fn name(&self) -> &'static str {
        "dirs"
    }
    fn synopsis(&self) -> &'static str {
        "dirs [-clpv] [+N] [-N]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut opts = DirsOptions::default();
        let mut selected = None;
        for arg in ctx.args {
            if arg == "--" {
                break;
            }
            match parse_dirs_arg(arg) {
                ParsedDirsArg::Clear => opts.clear = true,
                ParsedDirsArg::NoTilde => opts.no_tilde = true,
                ParsedDirsArg::OnePerLine => opts.one_per_line = true,
                ParsedDirsArg::Numbered => opts.numbered = true,
                ParsedDirsArg::Index(index) => selected = Some(index),
                ParsedDirsArg::InvalidNumber => {
                    report_diagnostic(ctx.env_ref(), "dirs", &format!("{arg}: invalid number"));
                    usage("dirs", "dirs [-clpv] [+N] [-N]");
                    return 2;
                }
                ParsedDirsArg::InvalidOption => {
                    report_diagnostic(ctx.env_ref(), "dirs", &format!("{arg}: invalid option"));
                    usage("dirs", "dirs [-clpv] [+N] [-N]");
                    return 2;
                }
            }
        }
        if opts.clear {
            ctx.env().dirs_clear();
            return 0;
        }
        print_dirs(ctx.env_ref(), opts, selected, true)
    }
}

#[derive(Clone, Copy, Default)]
pub struct DirsOptions {
    pub clear: bool,
    pub no_tilde: bool,
    pub one_per_line: bool,
    pub numbered: bool,
}

enum ParsedDirsArg {
    Clear,
    NoTilde,
    OnePerLine,
    Numbered,
    Index(StackIndex),
    InvalidNumber,
    InvalidOption,
}

fn parse_dirs_arg(arg: &str) -> ParsedDirsArg {
    if let Some(rest) = arg.strip_prefix('+') {
        return parse_signed_index(rest, false);
    }
    if let Some(rest) = arg.strip_prefix('-') {
        if rest.is_empty() {
            return ParsedDirsArg::InvalidNumber;
        }
        if rest.chars().all(|ch| ch.is_ascii_digit()) {
            return parse_signed_index(rest, true);
        }
        let mut parsed = None;
        for ch in rest.chars() {
            let next = match ch {
                'c' => ParsedDirsArg::Clear,
                'l' => ParsedDirsArg::NoTilde,
                'p' => ParsedDirsArg::OnePerLine,
                'v' => ParsedDirsArg::Numbered,
                _ => return ParsedDirsArg::InvalidNumber,
            };
            parsed = Some(next);
        }
        return parsed.unwrap_or(ParsedDirsArg::InvalidNumber);
    }
    ParsedDirsArg::InvalidOption
}

fn parse_signed_index(rest: &str, right: bool) -> ParsedDirsArg {
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) {
        return ParsedDirsArg::InvalidNumber;
    }
    let Ok(n) = rest.parse::<usize>() else {
        return ParsedDirsArg::InvalidNumber;
    };
    if right {
        ParsedDirsArg::Index(StackIndex::Right(n))
    } else {
        ParsedDirsArg::Index(StackIndex::Left(n))
    }
}

pub fn print_dirs(
    env: &dyn Environment,
    opts: DirsOptions,
    selected: Option<StackIndex>,
    source_diagnostics: bool,
) -> i32 {
    let stack = env.dirs_iter();
    let home = env.get("HOME").unwrap_or_default();
    if let Some(index) = selected {
        let Some(i) = resolve_index(stack.len(), index) else {
            let n = match index {
                StackIndex::Left(n) | StackIndex::Right(n) => n,
            };
            if source_diagnostics {
                report_diagnostic(
                    env,
                    "dirs",
                    &format!("{n}: directory stack index out of range"),
                );
            } else {
                eprintln!("cherubsh: dirs: {n}: directory stack index out of range");
            }
            return 1;
        };
        print_dir_entry(i, &stack[i], &home, opts);
        return 0;
    }
    if opts.one_per_line || opts.numbered {
        for (i, p) in stack.iter().enumerate() {
            print_dir_entry(i, p, &home, opts);
        }
    } else {
        let line = stack
            .iter()
            .map(|p| render(p, &home, opts.no_tilde))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{line}");
    }
    0
}

fn print_dir_entry(index: usize, path: &Path, home: &str, opts: DirsOptions) {
    if opts.numbered {
        println!("{:>2}  {}", index, render(path, home, opts.no_tilde));
    } else {
        println!("{}", render(path, home, opts.no_tilde));
    }
}

fn render(p: &Path, home: &str, no_tilde: bool) -> String {
    let raw = p.display().to_string();
    if no_tilde || home.is_empty() {
        raw
    } else if let Some(rest) = raw.strip_prefix(home) {
        if rest.is_empty() {
            "~".to_string()
        } else if rest.starts_with('/') {
            format!("~{rest}")
        } else {
            raw
        }
    } else {
        raw
    }
}

pub fn usage(name: &str, synopsis: &str) {
    eprintln!("{name}: usage: {synopsis}");
}
