use std::io::Write;

use crate::common::{diagnostic_label, report_diagnostic};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Return;
pub static RETURN: Return = Return;

impl Builtin for Return {
    fn name(&self) -> &'static str {
        "return"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "return [n]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.shell.function_depth() == 0 && ctx.shell.source_depth() == 0 {
            let _ = writeln!(
                std::io::stderr(),
                "{}: can only `return' from a function or sourced script",
                diagnostic_label(ctx.env_ref(), "return")
            );
            if ctx.env_ref().option("posix")
                && !ctx.env_ref().option("interactive")
                && !ctx.invoked_via_command
            {
                ctx.shell.request_exit(2);
            }
            return 2;
        }
        let status = match ctx.args.first() {
            Some(arg) => match arg.parse::<i64>() {
                Ok(n) => ((n % 256 + 256) % 256) as i32,
                Err(_) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "return",
                        &format!("{arg}: numeric argument required"),
                    );
                    return 2;
                }
            },
            None => ctx.env_ref().last_status(),
        };
        ctx.shell.request_return(status);
        status
    }
}
