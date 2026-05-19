use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Continue;
pub static CONTINUE: Continue = Continue;

impl Builtin for Continue {
    fn name(&self) -> &'static str {
        "continue"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "continue [n]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.shell.loop_depth() == 0 {
            report_diagnostic(
                ctx.env_ref(),
                "continue",
                "only meaningful in a `for', `while', or `until' loop",
            );
            return 0;
        }
        let n: i64 = match ctx.args.first() {
            Some(arg) => match arg.parse::<i64>() {
                Ok(v) => v,
                Err(_) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "continue",
                        &format!("{arg}: numeric argument required"),
                    );
                    ctx.shell.request_exit(128);
                    return 128;
                }
            },
            None => 1,
        };
        if n <= 0 {
            report_diagnostic(
                ctx.env_ref(),
                "continue",
                &format!(
                    "{}: loop count out of range",
                    ctx.args.first().map(|s| s.as_str()).unwrap_or("")
                ),
            );
            ctx.shell.request_break(1);
            return 1;
        }
        let levels = (n as u32).min(ctx.shell.loop_depth());
        ctx.shell.request_continue(levels);
        0
    }
}
