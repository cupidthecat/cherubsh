use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Unalias;
pub static UNALIAS: Unalias = Unalias;

impl Builtin for Unalias {
    fn name(&self) -> &'static str {
        "unalias"
    }
    fn synopsis(&self) -> &'static str {
        "unalias [-a] name [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut all = false;
        let mut parser = OptParser::new(ctx.args, "a");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'a', .. } => all = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "unalias", &format!("-{ch}: invalid option"));
                    eprintln!("unalias: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "unalias",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("unalias: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        if all {
            for (name, _) in ctx.env_ref().alias_iter() {
                ctx.env().alias_unset(&name);
            }
            return 0;
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            eprintln!("unalias: usage: {}", self.synopsis());
            return 2;
        }
        let mut status = 0;
        for name in rest {
            if ctx.env_ref().alias_get(name).is_none() {
                report_diagnostic(ctx.env_ref(), "unalias", &format!("{name}: not found"));
                status = 1;
                continue;
            }
            ctx.env().alias_unset(name);
        }
        status
    }
}
