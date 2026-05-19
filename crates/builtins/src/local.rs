use crate::common;
use crate::declare;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Local;
pub static LOCAL: Local = Local;

impl Builtin for Local {
    fn name(&self) -> &'static str {
        "local"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::ASSIGNMENT | BuiltinFlags::LOCALVAR
    }
    fn synopsis(&self) -> &'static str {
        "local [option] name[=value] ..."
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.shell.function_depth() == 0 {
            common::report_diagnostic(ctx.env_ref(), "local", "can only be used in a function");
            return 1;
        }
        declare::run_local_form(ctx)
    }
}
