use crate::common::report_diagnostic;
use crate::{lookup_raw, Builtin, BuiltinCtx, BuiltinFlags};

pub struct BuiltinDispatcher;
pub static BUILTIN: BuiltinDispatcher = BuiltinDispatcher;

impl Builtin for BuiltinDispatcher {
    fn name(&self) -> &'static str {
        "builtin"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "builtin [shell-builtin [arg ...]]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.args.is_empty() {
            return 0;
        }
        let target_name = ctx.args[0].clone();
        let rest = ctx.args[1..].to_vec();
        let rest_flags = if ctx.arg_flags.len() >= ctx.args.len() {
            ctx.arg_flags[1..].to_vec()
        } else {
            vec![0; rest.len()]
        };
        let Some(b) = lookup_raw(&target_name) else {
            report_diagnostic(
                ctx.env_ref(),
                "builtin",
                &format!("{target_name}: not a shell builtin"),
            );
            return 1;
        };
        if !ctx.env_ref().builtin_enabled(b.name()) {
            report_diagnostic(
                ctx.env_ref(),
                "builtin",
                &format!("{target_name}: not a shell builtin"),
            );
            return 1;
        }
        let mut inner = BuiltinCtx {
            args: &rest,
            arg_flags: &rest_flags,
            assignments: ctx.assignments,
            redirects: ctx.redirects,
            invoked_via_command: false,
            shell: ctx.shell,
        };
        b.run(&mut inner)
    }
}
