use std::path::PathBuf;

use cherubsh_builtins::{options as set_options, shopt_table};
use cherubsh_common::{Environment, ShellError};

use crate::state::ShellState;

/// Parse GNU long options (--login, --norc, --rcfile <f>, etc.). Returns the
/// new argument index after consuming options. Mirrors shell.c:838-890.
pub fn parse_long_options(
    args: &[String],
    mut arg_index: usize,
    state: &mut ShellState,
) -> Result<usize, ShellError> {
    while arg_index < args.len() {
        let arg = &args[arg_index];
        if arg == "--" {
            arg_index += 1;
            break;
        }
        if !arg.starts_with("--") || arg.len() <= 2 {
            break;
        }
        let name = &arg[2..];
        match name {
            "debug" => state.debugging = true,
            "debugger" => {
                state.debugging = true;
                state.debugger_mode = true;
            }
            "dump-po-strings" => {
                state.dump_po_strings = true;
                state.dump_translatable_strings = true;
                state.noexec = true;
            }
            "dump-strings" => {
                state.dump_translatable_strings = true;
                state.noexec = true;
            }
            "help" => state.want_initial_help = true,
            "init-file" | "rcfile" => {
                arg_index += 1;
                let value = args.get(arg_index).ok_or_else(|| {
                    ShellError::with_code(format!("--{name} requires an argument"), 2)
                })?;
                state.rc_file = PathBuf::from(value);
            }
            "login" => state.make_login_shell = true,
            "noediting" => state.no_line_editing = true,
            "noprofile" => state.no_profile = true,
            "norc" => state.no_rc = true,
            "posix" => state.posixly_correct = true,
            "pretty-print" => state.pretty_print_mode = true,
            "protected" | "wordexp" => {
                return Err(ShellError::with_code(
                    format!("--{name}: invalid option"),
                    2,
                ));
            }
            "restricted" => state.restricted = true,
            "verbose" => state.verbose_flag = true,
            "version" => state.do_version = true,
            _ => {
                return Err(ShellError::with_code(
                    format!("--{name}: invalid option"),
                    2,
                ));
            }
        }
        arg_index += 1;
    }
    Ok(arg_index)
}

/// Parses short shell options such as `-c`, `-s`, `-i`, and `-e`.
///
/// A `+` prefix clears a flag. Parsing stops at the first non-option argument.
pub fn parse_shell_options(
    args: &[String],
    mut arg_index: usize,
    state: &mut ShellState,
) -> Result<usize, ShellError> {
    while arg_index < args.len() {
        let arg = args[arg_index].clone();
        if arg == "-" || arg == "--" {
            arg_index += 1;
            if arg == "-" {
                // bare "-" means treat following args as positionals.
            }
            return Ok(arg_index);
        }
        let on = match arg.chars().next() {
            Some('-') => true,
            Some('+') => false,
            _ => return Ok(arg_index),
        };
        if arg.len() < 2 {
            return Ok(arg_index);
        }
        let mut chars = arg[1..].chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                'c' => {
                    // Bash treats `+c` the same as `-c`: this is an
                    // invocation-only command-string marker, not a toggled
                    // runtime flag.
                    state.want_pending_command = true;
                }
                's' => {
                    state.read_from_stdin = true;
                }
                'i' => {
                    state.forced_interactive = on;
                }
                'l' => {
                    state.make_login_shell = true;
                }
                'r' => {
                    state.restricted = on;
                }
                'D' => {
                    state.dump_translatable_strings = on;
                    state.noexec = on;
                }
                'o' => {
                    if chars.peek().is_some() {
                        return Err(ShellError::with_code(
                            "-o must be followed by an option name",
                            2,
                        ));
                    }
                    arg_index += 1;
                    if let Some(value) = args.get(arg_index) {
                        apply_set_option(value, on, state)?;
                    } else {
                        list_set_options(state);
                    }
                }
                'O' => {
                    if chars.peek().is_some() {
                        return Err(ShellError::with_code(
                            "-O must be followed by a shopt option name",
                            2,
                        ));
                    }
                    arg_index += 1;
                    if let Some(value) = args.get(arg_index) {
                        apply_shopt_option(value, on, state)?;
                    } else {
                        list_shopt_options(state);
                    }
                }
                _ => {
                    if let Some(opt) = set_options::lookup_short(ch) {
                        state.set_option(opt.long, on);
                    } else {
                        return Err(ShellError::with_code(format!("-{ch}: invalid option"), 2));
                    }
                }
            }
        }
        arg_index += 1;
    }
    Ok(arg_index)
}

fn apply_set_option(name: &str, on: bool, state: &mut ShellState) -> Result<(), ShellError> {
    let Some(opt) = set_options::lookup_long(name) else {
        return Err(ShellError::with_code(
            format!("{name}: invalid option name"),
            2,
        ));
    };
    state.set_option(opt.long, on);
    Ok(())
}

fn apply_shopt_option(name: &str, on: bool, state: &mut ShellState) -> Result<(), ShellError> {
    let Some(_) = shopt_table::lookup(name) else {
        return Err(ShellError::with_code(
            format!("{name}: invalid shell option name"),
            2,
        ));
    };
    state.set_option(name, on);
    Ok(())
}

fn list_set_options(state: &ShellState) {
    for opt in set_options::iter_long() {
        println!(
            "{:<16}{}",
            opt.long,
            if state.option(opt.long) { "on" } else { "off" }
        );
    }
}

fn list_shopt_options(state: &ShellState) {
    for opt in shopt_table::SHOPT_OPTIONS {
        let on =
            state.option(opt.name) || (opt.default && !state.shopt_options.contains_key(opt.name));
        println!("{:<32}{}", opt.name, if on { "on" } else { "off" });
    }
}

/// Bind positional parameters: shell.c:1478-1529.
/// `start_index` is the index of $0 source (0 for -c shell, 1 for script).
pub fn bind_args(args: &[String], from: usize, start_index: usize, state: &mut ShellState) {
    let mut positionals = Vec::new();
    if start_index == 0 {
        // For -c: first remaining arg is $0, rest are $1..$n.
        if let Some(zero) = args.get(from) {
            positionals.push(zero.clone());
            state.shell_name = zero.clone();
        } else {
            positionals.push(state.shell_name.clone());
        }
        for value in args.iter().skip(from + 1) {
            positionals.push(value.clone());
        }
    } else {
        // For script invocation: $0 is the script name (already in shell_name),
        // $1..$n come from `from` onward.
        positionals.push(state.shell_name.clone());
        for value in args.iter().skip(from) {
            positionals.push(value.clone());
        }
    }
    state.dollar_vars = positionals;
}

/// Determine base name of argv[0]; strip leading '-' (indicates login shell).
/// Mirrors shell.c:1793-1821.
pub fn set_shell_name(argv0: &str, state: &mut ShellState) {
    let raw = argv0;
    let dash_prefixed = raw.starts_with('-');
    let trimmed = if dash_prefixed { &raw[1..] } else { raw };
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
    if dash_prefixed {
        state.login_shell = 1;
    }
    state.act_like_sh = base == "sh";
    state.su_shell = base == "su";
    state.shell_name = base;
}

pub const SHELL_VERSION: &str = "5.3.15";
pub const MAJOR_VERSION: u32 = 5;
pub const MINOR_VERSION: u32 = 3;
pub const PATCH_LEVEL: u32 = 15;
pub const BUILD_VERSION: u32 = 1;
pub const DIST_VERSION: &str = "5.3";
pub const MACHTYPE: &str = "x86_64-pc-linux-gnu";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl CompatVersion {
    pub fn dist(self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    pub fn release(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

pub fn compat_version() -> CompatVersion {
    let Some(raw) = std::env::var("CHERUBSH_BASH_COMPAT_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return parse_compat_version(SHELL_VERSION).unwrap_or(CompatVersion {
            major: MAJOR_VERSION,
            minor: MINOR_VERSION,
            patch: PATCH_LEVEL,
        });
    };
    parse_compat_version(&raw).unwrap_or(CompatVersion {
        major: MAJOR_VERSION,
        minor: MINOR_VERSION,
        patch: PATCH_LEVEL,
    })
}

pub fn compat_release_version() -> String {
    compat_version().release()
}

pub fn compat_dist_version() -> String {
    if std::env::var("CHERUBSH_BASH_COMPAT_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return DIST_VERSION.to_string();
    }
    compat_version().dist()
}

pub fn compat_shell_version_with_build() -> String {
    format!("{}({BUILD_VERSION})-release", compat_release_version())
}

fn parse_compat_version(raw: &str) -> Option<CompatVersion> {
    let numeric = raw
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|part| !part.is_empty())?;
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some(CompatVersion {
        major,
        minor,
        patch,
    })
}

pub fn show_shell_version() {
    let version = compat_release_version();
    let dist = compat_dist_version();
    println!("cherubsh, version {version}-release (bash-{dist} parity, x86_64-rust-linux-gnu)");
    println!("Copyright (C) 2025 cherubsh contributors");
    println!("License: GPLv3+ (see vendor/bash-5.3.15/COPYING)");
}

pub fn show_shell_usage() {
    print_shell_usage("cherubsh", false);
}

pub fn show_shell_invocation_usage(shell_name: &str) {
    print_shell_usage(shell_name, true);
}

fn print_shell_usage(shell_name: &str, stderr: bool) {
    macro_rules! out {
        ($($arg:tt)*) => {
            if stderr {
                eprintln!($($arg)*);
            } else {
                println!($($arg)*);
            }
        };
    }
    out!("{shell_name} [GNU long option] [option] ...");
    out!("{shell_name} [GNU long option] [option] script-file ...");
    out!("GNU long options:");
    out!("\t--debug");
    out!("\t--debugger");
    out!("\t--dump-po-strings");
    out!("\t--dump-strings");
    out!("\t--help");
    out!("\t--init-file");
    out!("\t--login");
    out!("\t--noediting");
    out!("\t--noprofile");
    out!("\t--norc");
    out!("\t--posix");
    out!("\t--pretty-print");
    out!("\t--rcfile");
    out!("\t--restricted");
    out!("\t--verbose");
    out!("\t--version");
    out!("Shell options:");
    out!("\t-ilrsD or -c command or -O shopt_option\t\t(invocation only)");
    out!("\t-abefhkmnptuvxBCEHPT or -o option");
}
