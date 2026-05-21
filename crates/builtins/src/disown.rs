use cherubsh_common::jobs::{JobFlags, JobSpec, JobState};

use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Disown;
pub static DISOWN: Disown = Disown;

impl Builtin for Disown {
    fn name(&self) -> &'static str {
        "disown"
    }
    fn synopsis(&self) -> &'static str {
        "disown [-h] [-ar] [jobspec ... | pid ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut all = false;
        let mut running_only = false;
        let mut nohup_only = false;
        let mut parser = OptParser::new(ctx.args, "ahr");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'a', .. } => all = true,
                GetOpt::Opt { ch: 'r', .. } => running_only = true,
                GetOpt::Opt { ch: 'h', .. } => nohup_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_disown_error(ctx, &format!("-{ch}: invalid option"));
                    eprintln!("disown: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_disown_error(ctx, &format!("-{ch}: option requires an argument"));
                    eprintln!("disown: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args).to_vec();

        // Collect target job ids first to avoid borrow issues.
        let ids: Vec<cherubsh_common::JobId> = {
            let Some(table) = ctx.env_ref().jobs_table() else {
                return 0;
            };
            if all {
                table
                    .list()
                    .iter()
                    .filter(|j| !running_only || j.state == JobState::Running)
                    .map(|j| j.id)
                    .collect()
            } else if rest.is_empty() {
                table.current().into_iter().collect()
            } else {
                let mut acc = Vec::new();
                for s in &rest {
                    if let Some(spec) = JobSpec::parse(s) {
                        if let Ok(id) = table.lookup(&spec) {
                            acc.push(id);
                        } else {
                            report_disown_error(ctx, &format!("{s}: no such job"));
                        }
                    } else {
                        if s.starts_with('@') {
                            report_disown_error(
                                ctx,
                                &format!("warning: {s}: job specification requires leading `%'",),
                            );
                        }
                        report_disown_error(ctx, &format!("{s}: no such job"));
                    }
                }
                acc
            }
        };

        if nohup_only {
            if let Some(table) = ctx.env().jobs_table_mut() {
                for id in ids {
                    if let Some(job) = table.get_mut(id) {
                        job.flags |= JobFlags::NOHUP;
                    }
                }
            }
        } else if let Some(table) = ctx.env().jobs_table_mut() {
            for id in ids {
                table.remove(id);
            }
        }
        0
    }
}

fn report_disown_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: disown: {message}");
    } else {
        eprintln!("cherubsh: disown: {message}");
    }
}
