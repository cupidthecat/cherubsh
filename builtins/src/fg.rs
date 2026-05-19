//! `fg` builtin.

use cherubsh_common::jobs::{JobFlags, JobId, JobSpec, JobState};
use cherubsh_common::signals::SignalMaskGuard;

use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Fg;
pub static FG: Fg = Fg;

impl Builtin for Fg {
    fn name(&self) -> &'static str {
        "fg"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "fg [job_spec]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if let Some(arg) = ctx.args.first().filter(|arg| arg.starts_with('-')) {
            report_fg_error(ctx, &format!("{arg}: invalid option"));
            eprintln!("fg: usage: fg [job_spec]");
            return 2;
        }
        if !ctx.env_ref().job_control_enabled() {
            report_fg_error(ctx, "no job control");
            return 1;
        }
        if ctx.env_ref().subshell_level() > 0 {
            report_fg_error(ctx, "no current jobs");
            return 1;
        }
        reap_jobs(ctx);
        let spec = ctx
            .args
            .first()
            .and_then(|s| JobSpec::parse(s))
            .unwrap_or(JobSpec::Current);
        let (pgid, command, id, job_control_job) = {
            let Some(table) = ctx.env_ref().jobs_table() else {
                report_fg_error(ctx, "no current jobs");
                return 1;
            };
            let id = match table.lookup(&spec) {
                Ok(i) => i,
                Err(_) => {
                    let message = match ctx.args.first() {
                        Some(arg) => format!("{arg}: no such job"),
                        None => "no current jobs".to_string(),
                    };
                    report_fg_error(ctx, &message);
                    return 1;
                }
            };
            let job = table.get(id).unwrap();
            (
                job.pgid,
                job.command_line.clone(),
                id,
                job.flags.contains(JobFlags::JOBCONTROL),
            )
        };
        if !job_control_job {
            report_fg_error(
                ctx,
                &format!("job {} started without job control", id.raw()),
            );
            return 1;
        }
        println!("{}", command);
        let tty = ctx.env_ref().tty_fd();
        let shell_pgrp = ctx.env_ref().shell_pgrp();
        if let Some(fd) = tty {
            cherubsh_common::signals::SignalMaskGuard::block_terminal_handoff();
            unsafe {
                libc::tcsetpgrp(fd, pgid);
            }
        }
        unsafe {
            libc::killpg(pgid, libc::SIGCONT);
        }
        if let Some(table) = ctx.env().jobs_table_mut() {
            for j in table.list_mut() {
                if j.pgid == pgid {
                    j.state = JobState::Running;
                    for p in j.processes.iter_mut() {
                        if p.state == JobState::Stopped {
                            p.state = JobState::Running;
                        }
                    }
                }
            }
        }
        let mut status: libc::c_int = 0;
        let mut last_status = 0;
        loop {
            let rc = unsafe { libc::waitpid(-pgid, &mut status, libc::WUNTRACED) };
            if rc < 0 {
                let errno = unsafe { *libc::__errno_location() };
                if errno == libc::EINTR {
                    continue;
                }
                break;
            }
            if libc::WIFSTOPPED(status) {
                let stop_sig = libc::WSTOPSIG(status);
                last_status = 128 + stop_sig;
                if let Some(table) = ctx.env().jobs_table_mut() {
                    table.mark_stopped(rc, stop_sig);
                }
                break;
            }
            if let Some(table) = ctx.env().jobs_table_mut() {
                table.mark_dead(rc, status);
            }
            if libc::WIFEXITED(status) {
                last_status = libc::WEXITSTATUS(status);
            } else if libc::WIFSIGNALED(status) {
                last_status = 128 + libc::WTERMSIG(status);
            }
            // Check if job done
            let done = ctx
                .env_ref()
                .jobs_table()
                .and_then(|t| {
                    t.list()
                        .iter()
                        .find(|j| j.pgid == pgid)
                        .map(|j| j.state == JobState::Done)
                })
                .unwrap_or(true);
            if done {
                break;
            }
        }
        purge_done_job(ctx, id);
        if let Some(fd) = tty {
            cherubsh_common::signals::SignalMaskGuard::block_terminal_handoff();
            unsafe {
                libc::tcsetpgrp(fd, shell_pgrp);
            }
        }
        last_status
    }
}

fn purge_done_job(ctx: &mut BuiltinCtx<'_>, id: JobId) {
    let done = ctx
        .env_ref()
        .jobs_table()
        .and_then(|table| table.get(id))
        .is_some_and(|job| job.state == JobState::Done);
    if done {
        if let Some(table) = ctx.env().jobs_table_mut() {
            table.remove(id);
        }
    }
}

fn reap_jobs(ctx: &mut BuiltinCtx<'_>) {
    let _guard = SignalMaskGuard::block_sigchld();
    if let Some(table) = ctx.env().jobs_table_mut() {
        let _ = table.reap_all();
    }
}

fn report_fg_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: fg: {message}");
    } else {
        eprintln!("cherubsh: fg: {message}");
    }
}
