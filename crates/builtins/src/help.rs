use crate::getopt::{GetOpt, OptParser};
use crate::{iter_builtins, lookup_raw, Builtin, BuiltinCtx};

pub struct Help;
pub static HELP: Help = Help;

impl Builtin for Help {
    fn name(&self) -> &'static str {
        "help"
    }
    fn synopsis(&self) -> &'static str {
        "help [-dms] [pattern ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut short_desc = false;
        let mut man_format = false;
        let mut synopsis_only = false;
        let mut parser = OptParser::new(ctx.args, "dms");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'd', .. } => short_desc = true,
                GetOpt::Opt { ch: 'm', .. } => man_format = true,
                GetOpt::Opt { ch: 's', .. } => synopsis_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: help: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: help: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            for b in iter_builtins() {
                if synopsis_only || short_desc {
                    println!("{}", b.synopsis());
                } else {
                    println!("{:<20} {}", b.name(), b.synopsis());
                }
            }
            return 0;
        }
        let mut status = 0;
        for pattern in rest {
            match lookup_raw(pattern) {
                Some(b) => {
                    if man_format {
                        println!("NAME\n    {}\nSYNOPSIS\n    {}", b.name(), b.synopsis());
                    } else if synopsis_only {
                        println!("{}", b.synopsis());
                    } else if short_desc {
                        println!("{} - {}", b.name(), b.synopsis());
                    } else {
                        println!("{}: {}", b.name(), b.synopsis());
                    }
                }
                None => {
                    eprintln!("cherubsh: help: no help topics match `{pattern}'");
                    status = 1;
                }
            }
        }
        status
    }
}
