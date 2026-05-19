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
        if ctx.args.len() > 1 {
            report_diagnostic(ctx.env_ref(), "shift", "too many arguments");
            return 1;
        }
        let n: usize = match ctx.args.first() {
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
                    return 1;
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
