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
                    report_suspend_error(ctx, &format!("-{ch}: invalid option"));
                    eprintln!("suspend: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_suspend_error(ctx, &format!("-{ch}: option requires an argument"));
                    eprintln!("suspend: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let env = ctx.env_ref();
        if !env.job_control_enabled() {
            report_suspend_error(ctx, "cannot suspend: no job control");
            return 1;
        }
        if env.shell_pid() == 1 || (!force && env.get("BASH_LOGIN_SHELL").is_some()) {
            report_suspend_error(ctx, "cannot suspend a login shell");
            return 1;
        }
        // SIGTSTP (not SIGSTOP) so the parent can catch it and resume us.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTSTP);
        }
        0
    }
}

fn report_suspend_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: suspend: {message}");
    } else {
        eprintln!("cherubsh: suspend: {message}");
    }
}
