use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Shift;
pub static SHIFT: Shift = Shift;

impl Builtin for Shift {
    fn name(&self) -> &'static str {
        "shift"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "shift [n]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.args.first().map(String::as_str) == Some("--help") {
            print_shift_help();
            return 0;
        }
        let args = if ctx.args.first().map(String::as_str) == Some("--") {
            &ctx.args[1..]
        } else {
            ctx.args
        };
        if args.len() > 1 {
            report_diagnostic(ctx.env_ref(), "shift", "too many arguments");
            return 2;
        }
        let n: usize = match args.first() {
            Some(arg) => match arg.parse::<i64>() {
                Ok(v) if v >= 0 => v as usize,
                Ok(_) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "shift",
                        &format!("{arg}: shift count out of range"),
                    );
                    return 1;
                }
                _ => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "shift",
                        &format!("{arg}: numeric argument required"),
                    );
                    if ctx.env_ref().option("posix")
                        && !ctx.env_ref().option("interactive")
                        && !ctx.invoked_via_command
                    {
                        ctx.shell.request_exit(2);
                    }
                    return 2;
                }
            },
            None => 1,
        };
        let mut positionals = ctx.env().positionals_clone();
        let available = positionals.len().saturating_sub(1);
        if n > available {
            if ctx.env_ref().option("shift_verbose") || ctx.env_ref().option("posix") {
                report_diagnostic(
                    ctx.env_ref(),
                    "shift",
                    &format!("{n}: shift count out of range"),
                );
            }
            return 1;
        }
        for _ in 0..n {
            if positionals.len() > 1 {
                positionals.remove(1);
            }
        }
        ctx.env().set_positionals(positionals);
        0
    }
}

fn print_shift_help() {
    print!(
        "shift: shift [n]
    Shift positional parameters.
    
    Rename the positional parameters $N+1,$N+2 ... to $1,$2 ...  If N is
    not given, it is assumed to be 1.
    
    Exit Status:
    Returns success unless N is negative or greater than $#.
"
    );
}
