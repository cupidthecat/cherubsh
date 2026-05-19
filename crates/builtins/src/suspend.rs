use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Suspend;
pub static SUSPEND: Suspend = Suspend;
impl Builtin for Suspend {
    fn name(&self) -> &'static str {
        "suspend"
    }
    fn synopsis(&self) -> &'static str {
        "suspend [-f]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut force = false;
        let mut parser = OptParser::new(ctx.args, "f");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'f', .. } => force = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: suspend: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: suspend: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let env = ctx.env_ref();
        if env.shell_pid() == 1 || (!force && env.get("BASH_LOGIN_SHELL").is_some()) {
            eprintln!("cherubsh: suspend: cannot suspend a login shell");
            return 1;
        }
        // SIGTSTP (not SIGSTOP) so the parent can catch it and resume us.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTSTP);
        }
        0
    }
}
