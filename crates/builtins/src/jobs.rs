//! `jobs` builtin.

use cherubsh_common::jobs::{JobSpec, JobState};
use cherubsh_common::signals::SignalMaskGuard;

use crate::common::ansi_c_quote;
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Jobs;
pub static JOBS: Jobs = Jobs;

#[derive(Default)]
struct Flags {
    long: bool,
    pgids_only: bool,
    new_only: bool,
    running_only: bool,
    stopped_only: bool,
    exec_mode: Option<String>,
}

impl Builtin for Jobs {
    fn name(&self) -> &'static str {
        "jobs"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "jobs [-lnprs] [jobspec ...] or jobs -x command [args]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut flags = Flags::default();
        let mut parser = OptParser::new(ctx.args, "lnprsx");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'l', .. } => flags.long = true,
                GetOpt::Opt { ch: 'n', .. } => flags.new_only = true,
                GetOpt::Opt { ch: 'p', .. } => flags.pgids_only = true,
                GetOpt::Opt { ch: 'r', .. } => flags.running_only = true,
                GetOpt::Opt { ch: 's', .. } => flags.stopped_only = true,
                GetOpt::Opt { ch: 'x', .. } => {
                    flags.exec_mode = Some(String::new());
                    break;
                }
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: jobs: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: jobs: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if flags.exec_mode.is_some() {
            return run_jobs_x(ctx, rest);
        }
        reap_jobs(ctx);
        let table = match ctx.env_ref().jobs_table() {
            Some(t) => t,
            None => return 0,
        };
        let mut ids: Vec<u32> = if rest.is_empty() {
            table.list().iter().map(|j| j.id.raw()).collect()
        } else {
            let mut out = Vec::new();
            for arg in rest {
                let Some(spec) = JobSpec::parse(arg) else {
                    report_jobs_error(ctx, &format!("{arg}: no such job"));
                    return 1;
                };
                match table.lookup(&spec) {
                    Ok(id) => out.push(id.raw()),
                    Err(_) => {
                        report_jobs_error(ctx, &format!("{arg}: no such job"));
                        return 1;
                    }
                }
            }
            out
        };
        ids.sort();
        let current = table.current().map(|i| i.raw());
        let previous = table.previous().map(|i| i.raw());
        for id in ids {
            let Some(job) = table.list().iter().find(|j| j.id.raw() == id) else {
                continue;
            };
            if flags.running_only && job.state != JobState::Running {
                continue;
            }
            if flags.stopped_only && job.state != JobState::Stopped {
                continue;
            }
            if flags.new_only && job.notified {
                continue;
            }
            if flags.pgids_only {
                println!("{}", job.pgid);
                continue;
            }
            let marker = if Some(id) == current {
                '+'
            } else if Some(id) == previous {
                '-'
            } else {
                ' '
            };
            let label = match job.state {
                JobState::Running => "Running",
                JobState::Stopped => "Stopped",
                JobState::Done => "Done",
            };
            let command = if job.state == JobState::Running {
                format!("{} &", job.command_line)
            } else {
                job.command_line.clone()
            };
            if flags.long {
                println!(
                    "[{}]{}  {} {:<23} {}",
                    job.id.raw(),
                    marker,
                    job.leader_pid,
                    label,
                    command
                );
            } else {
                println!("[{}]{}  {:<23} {}", job.id.raw(), marker, label, command);
            }
        }
        let _ = ansi_c_quote;
        0
    }
}

fn report_jobs_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: jobs: {message}");
    } else {
        eprintln!("cherubsh: jobs: {message}");
    }
}

fn reap_jobs(ctx: &mut BuiltinCtx<'_>) {
    let _guard = SignalMaskGuard::block_sigchld();
    if let Some(table) = ctx.env().jobs_table_mut() {
        let _ = table.reap_all();
    }
}

fn run_jobs_x(ctx: &mut BuiltinCtx<'_>, argv: &[String]) -> i32 {
    if argv.is_empty() {
        eprintln!("cherubsh: jobs: -x: option requires an argument");
        return 2;
    }
    let mut words = Vec::with_capacity(argv.len());
    for arg in argv {
        if arg.starts_with('%') {
            let replacement = {
                let Some(table) = ctx.env_ref().jobs_table() else {
                    eprintln!("cherubsh: jobs: {arg}: no such job");
                    return 1;
                };
                let Some(spec) = JobSpec::parse(arg) else {
                    eprintln!("cherubsh: jobs: {arg}: no such job");
                    return 1;
                };
                let id = match table.lookup(&spec) {
                    Ok(id) => id,
                    Err(_) => {
                        eprintln!("cherubsh: jobs: {arg}: no such job");
                        return 1;
                    }
                };
                table.get(id).map(|job| job.pgid.to_string())
            };
            let Some(pgid) = replacement else {
                eprintln!("cherubsh: jobs: {arg}: no such job");
                return 1;
            };
            words.push(shell_quote(&pgid));
        } else {
            words.push(shell_quote(arg));
        }
    }
    ctx.shell.run_source(&words.join(" "))
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}
