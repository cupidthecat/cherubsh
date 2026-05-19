use crate::common::format_set_var;
use crate::common::report_diagnostic;
use crate::options::{lookup_long, lookup_short, SET_OPTIONS};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Set;
pub static SET: Set = Set;

impl Builtin for Set {
    fn name(&self) -> &'static str {
        "set"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [arg ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.args.is_empty() {
            // List all variables.
            let mut snaps = ctx.env_ref().iter_vars();
            snaps.sort_by(|a, b| a.name.cmp(&b.name));
            let posix =
                ctx.env_ref().option("posix") || ctx.env_ref().get("POSIXLY_CORRECT").is_some();
            for snap in snaps {
                if let Some(line) = format_set_var(&snap, posix) {
                    println!("{line}");
                }
            }
            return 0;
        }

        let mut idx = 0;
        let mut positional_replacement: Option<Vec<String>> = None;
        while idx < ctx.args.len() {
            let arg = &ctx.args[idx];
            if arg == "--" {
                idx += 1;
                let rest = ctx.args[idx..].to_vec();
                positional_replacement = Some(rest);
                break;
            }
            if arg == "-" {
                idx += 1;
                if idx < ctx.args.len() {
                    positional_replacement = Some(ctx.args[idx..].to_vec());
                }
                break;
            }
            if let Some(body) = arg.strip_prefix('-') {
                if body.is_empty() {
                    break;
                }
                if !apply_flags(ctx, body, true, '-') {
                    return 2;
                }
                let next_arg = body.chars().any(|c| c == 'o');
                if next_arg {
                    // -o NAME consumes next arg.
                    idx += 1;
                    if idx < ctx.args.len() {
                        let name = ctx.args[idx].clone();
                        if !apply_long(ctx, &name, true) {
                            return 2;
                        }
                    } else {
                        list_set_options(ctx, false);
                        return 0;
                    }
                }
                idx += 1;
                continue;
            }
            if let Some(body) = arg.strip_prefix('+') {
                if !apply_flags(ctx, body, false, '+') {
                    return 2;
                }
                let next_arg = body.chars().any(|c| c == 'o');
                if next_arg {
                    idx += 1;
                    if idx < ctx.args.len() {
                        let name = ctx.args[idx].clone();
                        if !apply_long(ctx, &name, false) {
                            return 2;
                        }
                    } else {
                        list_set_options(ctx, true);
                        return 0;
                    }
                }
                idx += 1;
                continue;
            }
            // Positional starts.
            let rest = ctx.args[idx..].to_vec();
            positional_replacement = Some(rest);
            break;
        }

        if let Some(rest) = positional_replacement {
            let mut new_positionals = Vec::with_capacity(rest.len() + 1);
            new_positionals.push(
                ctx.env_ref()
                    .positional(0)
                    .unwrap_or_else(|| "cherubsh".to_string()),
            );
            new_positionals.extend(rest);
            ctx.env().set_positionals(new_positionals);
        }
        0
    }
}

fn apply_flags(ctx: &mut BuiltinCtx<'_>, body: &str, on: bool, sign: char) -> bool {
    for ch in body.chars() {
        if ch == 'o' {
            continue; // handled by caller
        }
        if ch == 'r' {
            if on {
                ctx.env().set_option("restricted", true);
                continue;
            }
            if ctx.env_ref().option("restricted") {
                report_diagnostic(ctx.env_ref(), "set", "+r: invalid option");
                eprintln!(
                    "set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]"
                );
                return false;
            }
            continue;
        }
        if let Some(opt) = lookup_short(ch) {
            ctx.env().set_option(opt.long, on);
        } else {
            report_diagnostic(ctx.env_ref(), "set", &format!("{sign}{ch}: invalid option"));
            eprintln!("set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]");
            return false;
        }
    }
    true
}

fn apply_long(ctx: &mut BuiltinCtx<'_>, name: &str, on: bool) -> bool {
    if name == "restricted" {
        report_diagnostic(ctx.env_ref(), "set", "restricted: invalid option name");
        return false;
    }
    if let Some(opt) = lookup_long(name) {
        ctx.env().set_option(opt.long, on);
        true
    } else {
        report_diagnostic(
            ctx.env_ref(),
            "set",
            &format!("{name}: invalid option name"),
        );
        false
    }
}

fn list_set_options(ctx: &mut BuiltinCtx<'_>, print_form: bool) {
    for opt in SET_OPTIONS {
        let on = ctx.env_ref().option(opt.long);
        if print_form {
            println!("set {}o {}", if on { "-" } else { "+" }, opt.long);
        } else {
            println!("{:<15}\t{}", opt.long, if on { "on" } else { "off" });
        }
    }
}
