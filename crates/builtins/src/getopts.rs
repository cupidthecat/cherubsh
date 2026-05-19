use crate::common::{is_valid_name, report_builtin_assign_error};
use crate::{Builtin, BuiltinCtx};

const NEXT_CHAR_VAR: &str = "_CHERUBSH_GETOPTS_NEXT";
const LAST_OPTIND_VAR: &str = "_CHERUBSH_GETOPTS_OPTIND";

pub struct Getopts;
pub static GETOPTS: Getopts = Getopts;

impl Builtin for Getopts {
    fn name(&self) -> &'static str {
        "getopts"
    }
    fn synopsis(&self) -> &'static str {
        "getopts optstring name [arg ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.args.len() < 2 {
            eprintln!("getopts: usage: getopts optstring name [arg ...]");
            return 2;
        }
        let optstring = &ctx.args[0];
        let name = &ctx.args[1];
        if let Some(option) = optstring.strip_prefix('-') {
            let ch = option.chars().next().unwrap_or('-');
            report_getopts_error(ctx, &format!("-{ch}: invalid option"));
            eprintln!("getopts: usage: getopts optstring name [arg ...]");
            return 2;
        }
        if !is_valid_name(name) {
            report_getopts_error(ctx, &format!("`{name}': not a valid identifier"));
            let _ = assign(ctx, "OPTIND", "2".to_string());
            return 2;
        }

        let optind: usize = ctx
            .env_ref()
            .get("OPTIND")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);

        let last_optind_var = state_var(ctx, LAST_OPTIND_VAR);
        let next_char_var = state_var(ctx, NEXT_CHAR_VAR);
        let last_optind = ctx
            .env_ref()
            .get(&last_optind_var)
            .and_then(|v| v.parse::<usize>().ok());
        let next_char = if last_optind == Some(optind) {
            ctx.env_ref()
                .get(&next_char_var)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
        } else {
            1
        };

        let owned_pos: Vec<String>;
        let source: &[String] = if ctx.args.len() > 2 {
            &ctx.args[2..]
        } else {
            owned_pos = collect_positionals(ctx);
            return walk(
                ctx,
                optstring,
                name,
                &owned_pos,
                optind,
                next_char,
                &last_optind_var,
                &next_char_var,
            );
        };
        walk(
            ctx,
            optstring,
            name,
            source,
            optind,
            next_char,
            &last_optind_var,
            &next_char_var,
        )
    }
}

fn collect_positionals(ctx: &mut BuiltinCtx<'_>) -> Vec<String> {
    let env = ctx.env_ref();
    let count = env.positional_count();
    (1..=count).filter_map(|i| env.positional(i)).collect()
}

fn walk(
    ctx: &mut BuiltinCtx<'_>,
    optstring: &str,
    name: &str,
    source: &[String],
    mut optind: usize,
    mut next_char: usize,
    last_optind_var: &str,
    next_char_var: &str,
) -> i32 {
    if optind == 0 {
        optind = 1;
    }
    if next_char == 0 {
        next_char = 1;
    }
    if optind > source.len() {
        let _ = assign(ctx, name, "?".to_string());
        ctx.env().unset("OPTARG");
        let _ = assign(ctx, "OPTIND", optind.to_string());
        reset_inner(ctx, optind, last_optind_var, next_char_var);
        return 1;
    }
    let arg = &source[optind - 1];
    if arg == "--" {
        let _ = assign(ctx, name, "?".to_string());
        ctx.env().unset("OPTARG");
        let _ = assign(ctx, "OPTIND", (optind + 1).to_string());
        reset_inner(ctx, optind + 1, last_optind_var, next_char_var);
        return 1;
    }
    if !arg.starts_with('-') || arg.len() < 2 || arg == "-" {
        let _ = assign(ctx, name, "?".to_string());
        ctx.env().unset("OPTARG");
        reset_inner(ctx, optind, last_optind_var, next_char_var);
        return 1;
    }

    let bytes = arg.as_bytes();
    if next_char >= bytes.len() {
        optind += 1;
        next_char = 1;
    }
    if optind > source.len() {
        let _ = assign(ctx, name, "?".to_string());
        ctx.env().unset("OPTARG");
        let _ = assign(ctx, "OPTIND", optind.to_string());
        reset_inner(ctx, optind, last_optind_var, next_char_var);
        return 1;
    }

    let arg = &source[optind - 1];
    let bytes = arg.as_bytes();
    let cur_char = bytes.get(next_char).copied().unwrap_or(b'?') as char;
    let silent = optstring.starts_with(':');
    if option_present(optstring, cur_char) {
        let needs_arg = option_requires_arg(optstring, cur_char);
        if needs_arg {
            let rest = &arg[next_char + 1..];
            if !rest.is_empty() {
                let _ = assign(ctx, "OPTARG", rest.to_string());
                let _ = assign(ctx, name, cur_char.to_string());
                let _ = assign(ctx, "OPTIND", (optind + 1).to_string());
                reset_inner(ctx, optind + 1, last_optind_var, next_char_var);
            } else if optind < source.len() {
                let val = source[optind].clone();
                let _ = assign(ctx, "OPTARG", val);
                let _ = assign(ctx, name, cur_char.to_string());
                let _ = assign(ctx, "OPTIND", (optind + 2).to_string());
                reset_inner(ctx, optind + 2, last_optind_var, next_char_var);
            } else if silent {
                let _ = assign(ctx, "OPTARG", cur_char.to_string());
                let _ = assign(ctx, name, ":".to_string());
                let _ = assign(ctx, "OPTIND", (optind + 1).to_string());
                reset_inner(ctx, optind + 1, last_optind_var, next_char_var);
            } else {
                if ctx.env_ref().get("OPTERR").as_deref() != Some("0") {
                    eprintln!(
                        "{}: option requires an argument -- {cur_char}",
                        shell_name(ctx)
                    );
                }
                let _ = assign(ctx, name, "?".to_string());
                ctx.env().unset("OPTARG");
                let _ = assign(ctx, "OPTIND", (optind + 1).to_string());
                reset_inner(ctx, optind + 1, last_optind_var, next_char_var);
            }
        } else {
            ctx.env().unset("OPTARG");
            let _ = assign(ctx, name, cur_char.to_string());
            if next_char + 1 < bytes.len() {
                let _ = assign(ctx, "OPTIND", optind.to_string());
                set_inner(ctx, optind, next_char + 1, last_optind_var, next_char_var);
            } else {
                let _ = assign(ctx, "OPTIND", (optind + 1).to_string());
                reset_inner(ctx, optind + 1, last_optind_var, next_char_var);
            }
        }
        0
    } else {
        if silent {
            let _ = assign(ctx, "OPTARG", cur_char.to_string());
            let _ = assign(ctx, name, "?".to_string());
        } else {
            if ctx.env_ref().get("OPTERR").as_deref() != Some("0") {
                eprintln!("{}: illegal option -- {cur_char}", shell_name(ctx));
            }
            let _ = assign(ctx, name, "?".to_string());
            ctx.env().unset("OPTARG");
        }
        if next_char + 1 < bytes.len() {
            let _ = assign(ctx, "OPTIND", optind.to_string());
            set_inner(ctx, optind, next_char + 1, last_optind_var, next_char_var);
        } else {
            let _ = assign(ctx, "OPTIND", (optind + 1).to_string());
            reset_inner(ctx, optind + 1, last_optind_var, next_char_var);
        }
        0
    }
}

fn option_present(optstring: &str, option: char) -> bool {
    optstring
        .trim_start_matches(':')
        .chars()
        .enumerate()
        .any(|(i, ch)| ch == option && (i == 0 || optstring.as_bytes()[i] != b':'))
}

fn option_requires_arg(optstring: &str, option: char) -> bool {
    let chars: Vec<char> = optstring.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == option && i + 1 < chars.len() && chars[i + 1] == ':' {
            return true;
        }
    }
    false
}

fn assign(ctx: &mut BuiltinCtx<'_>, name: &str, value: String) -> bool {
    match ctx.env().assign(name, value) {
        Ok(()) => true,
        Err(err) => {
            report_builtin_assign_error(ctx.env_ref(), "getopts", &err);
            false
        }
    }
}

fn report_getopts_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: getopts: {message}");
    } else {
        eprintln!("getopts: {message}");
    }
}

fn set_inner(
    ctx: &mut BuiltinCtx<'_>,
    optind: usize,
    next_char: usize,
    last_optind_var: &str,
    next_char_var: &str,
) {
    ctx.env().set(last_optind_var, optind.to_string());
    ctx.env().set(next_char_var, next_char.to_string());
}

fn reset_inner(
    ctx: &mut BuiltinCtx<'_>,
    optind: usize,
    last_optind_var: &str,
    next_char_var: &str,
) {
    ctx.env().set(last_optind_var, optind.to_string());
    ctx.env().set(next_char_var, "1".to_string());
}

fn shell_name(ctx: &BuiltinCtx<'_>) -> String {
    ctx.env_ref()
        .positional(0)
        .unwrap_or_else(|| "cherubsh".to_string())
}

fn state_var(ctx: &BuiltinCtx<'_>, base: &str) -> String {
    format!("{}_{}", base, ctx.shell.function_depth())
}
