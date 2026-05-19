use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Exit;
pub static EXIT: Exit = Exit;
impl Builtin for Exit {
    fn name(&self) -> &'static str {
        "exit"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "exit [n]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let status = parse_status(ctx);
        ctx.shell.request_exit(status);
        status
    }
}

pub struct Logout;
pub static LOGOUT: Logout = Logout;
impl Builtin for Logout {
    fn name(&self) -> &'static str {
        "logout"
    }
    fn synopsis(&self) -> &'static str {
        "logout [n]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if !ctx.env_ref().is_login_shell() {
            report_diagnostic(ctx.env_ref(), "logout", "not login shell: use `exit'");
            return 1;
        }
        let status = parse_status(ctx);
        ctx.shell.request_exit(status);
        status
    }
}

fn parse_status(ctx: &mut BuiltinCtx<'_>) -> i32 {
    match ctx.args.first() {
        Some(arg) => match arg.parse::<i64>() {
            Ok(n) => ((n % 256 + 256) % 256) as i32,
            Err(_) => {
                report_diagnostic(
                    ctx.env_ref(),
                    "exit",
                    &format!("{arg}: numeric argument required"),
                );
                2
            }
        },
        None => ctx.env_ref().last_status(),
    }
}
