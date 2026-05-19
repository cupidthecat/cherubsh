use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Eval;
pub static EVAL: Eval = Eval;

impl Builtin for Eval {
    fn name(&self) -> &'static str {
        "eval"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "eval [arg ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.args.is_empty() {
            return 0;
        }
        let args = if ctx.args.first().map(|arg| arg.as_str()) == Some("--") {
            &ctx.args[1..]
        } else {
            if let Some(arg) = ctx
                .args
                .first()
                .filter(|arg| arg.starts_with('-') && arg.as_str() != "-")
            {
                let opt = arg.chars().nth(1).unwrap_or('-');
                report_diagnostic(ctx.env_ref(), "eval", &format!("-{opt}: invalid option"));
                eprintln!("eval: usage: {}", self.synopsis());
                return 2;
            }
            ctx.args
        };
        if args.is_empty() {
            return 0;
        }
        let joined = args.join(" ");
        ctx.shell.run_eval(&joined)
    }
}
