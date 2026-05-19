//! `bg` builtin.

use cherubsh_common::jobs::{JobSpec, JobState};
use cherubsh_common::signals::SignalMaskGuard;

use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Bg;
pub static BG: Bg = Bg;

impl Builtin for Bg {
    fn name(&self) -> &'static str {
        "bg"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "bg [job_spec ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if let Some(arg) = ctx.args.iter().find(|arg| arg.starts_with('-')) {
            report_bg_error(ctx, &format!("{arg}: invalid option"));
            eprintln!("bg: usage: bg [job_spec ...]");
            return 2;
        }
        if !ctx.env_ref().job_control_enabled() {
            report_bg_error(ctx, "no job control");
            return 1;
        }
        if ctx.env_ref().subshell_level() > 0 {
            report_bg_error(ctx, "no current jobs");
            return 1;
        }
        reap_jobs(ctx);
        let specs: Vec<JobSpec> = if ctx.args.is_empty() {
            vec![JobSpec::Current]
        } else {
            ctx.args.iter().filter_map(|s| JobSpec::parse(s)).collect()
        };
        let mut last_status = 0;
        for spec in specs {
            let (pgid, command, id, marker) = {
                let Some(table) = ctx.env_ref().jobs_table() else {
                    last_status = 1;
                    continue;
                };
                let jid = match table.lookup(&spec) {
                    Ok(i) => i,
                    Err(_) => {
                        report_bg_error(ctx, "no such job");
                        last_status = 1;
                        continue;
                    }
                };
                let job = table.get(jid).unwrap();
                if job.state == JobState::Running {
                    report_bg_error(ctx, &format!("job {} already in background", jid.raw()));
                    last_status = 1;
                    continue;
                }
                let marker = if table.current() == Some(jid) {
                    Some('+')
                } else if table.previous() == Some(jid) {
                    Some('-')
                } else {
                    None
                };
                (job.pgid, job.command_line.clone(), jid.raw(), marker)
            };
            unsafe {
                libc::killpg(pgid, libc::SIGCONT);
            }
            if let Some(table) = ctx.env().jobs_table_mut() {
                if let Some(job) = table.list_mut().iter_mut().find(|j| j.pgid == pgid) {
                    job.state = JobState::Running;
                    for p in job.processes.iter_mut() {
                        if p.state == JobState::Stopped {
                            p.state = JobState::Running;
                        }
                    }
                }
            }
            if let Some(marker) = marker {
                println!("[{}]{} {} &", id, marker, command);
            } else {
                println!("[{}] {} &", id, command);
            }
        }
        last_status
    }
}

fn reap_jobs(ctx: &mut BuiltinCtx<'_>) {
    let _guard = SignalMaskGuard::block_sigchld();
    if let Some(table) = ctx.env().jobs_table_mut() {
        let _ = table.reap_all();
    }
}

fn report_bg_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: bg: {message}");
    } else {
        eprintln!("cherubsh: bg: {message}");
    }
}
