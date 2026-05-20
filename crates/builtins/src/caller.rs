use crate::{Builtin, BuiltinCtx};

pub struct Caller;
pub static CALLER: Caller = Caller;

impl Builtin for Caller {
    fn name(&self) -> &'static str {
        "caller"
    }
    fn synopsis(&self) -> &'static str {
        "caller [expr]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let Some(level) = parse_level(ctx) else {
            return 2;
        };
        let env = ctx.env_ref();
        let Some(line) = env.get_array_indexed("BASH_LINENO", level as i64) else {
            return 1;
        };
        if ctx.args.is_empty() {
            let source = env
                .get_array_indexed("BASH_SOURCE", level as i64 + 1)
                .unwrap_or_else(|| "NULL".to_string());
            println!("{line} {source}");
            return 0;
        }
        let Some(source) = env.get_array_indexed("BASH_SOURCE", level as i64 + 1) else {
            return 1;
        };
        let Some(function) = env.get_array_indexed("FUNCNAME", level as i64 + 1) else {
            return 1;
        };
        println!("{line} {function} {source}");
        0
    }
}

fn parse_level(ctx: &BuiltinCtx<'_>) -> Option<usize> {
    if ctx.args.len() > 1 {
        print_usage();
        return None;
    }
    let Some(arg) = ctx.args.first() else {
        return Some(0);
    };
    match arg.parse::<usize>() {
        Ok(level) => Some(level),
        Err(_) => {
            crate::common::report_diagnostic(
                ctx.env_ref(),
                "caller",
                &format!("{arg}: invalid number"),
            );
            print_usage();
            None
        }
    }
}

fn print_usage() {
    eprintln!("caller: usage: caller [expr]");
}
