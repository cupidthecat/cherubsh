use crate::common::flush_stdout;
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Pwd;
pub static PWD: Pwd = Pwd;

impl Builtin for Pwd {
    fn name(&self) -> &'static str {
        "pwd"
    }
    fn synopsis(&self) -> &'static str {
        "pwd [-LP]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut physical = false;
        let mut parser = OptParser::new(ctx.args, "LP");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'L', .. } => physical = false,
                GetOpt::Opt { ch: 'P', .. } => physical = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: pwd: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: pwd: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let value = if physical {
            match std::env::current_dir() {
                Ok(p) => match std::fs::canonicalize(&p) {
                    Ok(real) => real.display().to_string(),
                    Err(_) => p.display().to_string(),
                },
                Err(err) => {
                    eprintln!("cherubsh: pwd: {err}");
                    return 1;
                }
            }
        } else {
            ctx.env_ref().get("PWD").unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            })
        };
        println!("{value}");
        let _ = flush_stdout();
        0
    }
}
