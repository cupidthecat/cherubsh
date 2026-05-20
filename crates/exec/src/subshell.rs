use cherubsh_parser::SubshellCommand;

use crate::util::wait_for_pid;
use crate::{ExecContext, ExecMode, Unwind};

pub(crate) fn execute<'a>(ctx: &mut ExecContext<'a>, subshell: &SubshellCommand) -> i32 {
    if ctx.reuse_current_subshell {
        ctx.reuse_current_subshell = false;
        if !ctx.env.option("errtrace") && !ctx.env.option("extdebug") {
            ctx.suppress_err_traps = true;
        }
        if !ctx.env.option("functrace") && !ctx.env.option("extdebug") {
            ctx.suppress_debug_traps = true;
        }
        return ctx.execute_command(&subshell.command, ExecMode::Parent);
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return 1;
    }
    if pid == 0 {
        ctx.env.enter_subshell();
        ctx.env.suppress_inherited_exit_trap();
        if !ctx.env.option("errtrace") && !ctx.env.option("extdebug") {
            ctx.suppress_err_traps = true;
        }
        if !ctx.env.option("functrace") && !ctx.env.option("extdebug") {
            ctx.suppress_debug_traps = true;
        }
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
        }
        let status = ctx.with_abort_line_boundary(|ctx| {
            ctx.execute_command(&subshell.command, ExecMode::Parent)
        });
        let mut final_status = match ctx.pending.take() {
            Some(Unwind::Exit(n)) => n,
            _ => status,
        };
        ctx.env.set_last_status(final_status);
        if let Some(trap_status) = crate::trap::run_exit_trap(ctx) {
            final_status = trap_status;
        }
        unsafe { libc::_exit(final_status) };
    }
    wait_for_pid(pid)
}
