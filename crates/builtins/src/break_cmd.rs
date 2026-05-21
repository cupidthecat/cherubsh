use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Break;
pub static BREAK: Break = Break;

impl Builtin for Break {
    fn name(&self) -> &'static str {
        "break"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "break [n]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.shell.loop_depth() == 0 {
            if ctx.env_ref().option("posix") {
                return 0;
            }
            report_diagnostic(
                ctx.env_ref(),
                "break",
                "only meaningful in a `for', `while', or `until' loop",
            );
            return 0;
        }
        if positional_args_after_separator(ctx.args).len() > 1 {
            report_diagnostic(ctx.env_ref(), "break", "too many arguments");
            return 2;
        }
        let level_arg = match ctx.args.first().map(String::as_str) {
            Some("--") => ctx.args.get(1),
            _ => ctx.args.first(),
        };

        let n: i64 = match level_arg {
            Some(arg) => match arg.parse::<i64>() {
                Ok(v) => v,
                Err(_) => {
                    report_break_numeric_required(ctx, arg);
                    ctx.shell.request_exit(128);
                    return 128;
                }
            },
            None => 1,
        };
        if n == 0 {
            report_diagnostic(
                ctx.env_ref(),
                "break",
                &format!(
                    "{}: loop count out of range",
                    level_arg.map(|s| s.as_str()).unwrap_or("")
                ),
            );
            ctx.shell.request_break(1);
            return 1;
        }
        if n < 0 {
            report_diagnostic(
                ctx.env_ref(),
                "break",
                &format!(
                    "{}: loop count out of range",
                    level_arg.map(|s| s.as_str()).unwrap_or("")
                ),
            );
            ctx.shell.request_break(1);
            return 1;
        }
        let levels = (n as u32).min(ctx.shell.loop_depth());
        ctx.shell.request_break(levels);
        0
    }
}

fn report_break_numeric_required(ctx: &BuiltinCtx<'_>, arg: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: break: {arg}: numeric argument required");
    } else {
        report_diagnostic(
            ctx.env_ref(),
            "break",
            &format!("{arg}: numeric argument required"),
        );
    }
}

fn positional_args_after_separator(args: &[String]) -> &[String] {
    if args.first().map(String::as_str) == Some("--") {
        &args[1..]
    } else {
        args
    }
}
