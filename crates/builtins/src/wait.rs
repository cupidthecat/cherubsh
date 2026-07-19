//! `wait` builtin.

use std::thread;
use std::time::Duration;

use cherubsh_common::jobs::{JobFlags, JobSpec, JobState};
use cherubsh_common::signals::{TrapAction, TrapKind, NSIG};
use cherubsh_common::AssignError;

use crate::common::{
    assign_direct_target, indexed_target_array_expand_once_message, is_valid_name,
    report_bare_diagnostic, report_builtin_assign_error, unset_array_reference,
};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Wait;
pub static WAIT: Wait = Wait;

impl Builtin for Wait {
    fn name(&self) -> &'static str {
        "wait"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "wait [-fn] [-p var] [id ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut p_var: Option<String> = None;
        let mut wait_any = false;
        let mut force = false;
        let mut parser = OptParser::new(ctx.args, "fnp:");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'f', .. } => force = true,
                GetOpt::Opt { ch: 'n', .. } => wait_any = true,
                GetOpt::Opt { ch: 'p', arg, .. } => p_var = arg.clone(),
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: wait: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: wait: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args).to_vec();

        if let Some(var) = p_var.as_deref() {
            if !prepare_wait_p_var(ctx, var) {
                return 1;
            }
        }

        if rest.is_empty() {
            let (last_status, winner) = wait_for_children(ctx, wait_any, force);
            if let (Some(var), Some(pid)) = (p_var, winner) {
                if let Err(err) = assign_direct_target(ctx.env(), &var, pid.to_string()) {
                    report_builtin_assign_error(ctx.env_ref(), "wait", &err);
                }
            }
            purge_waited_done(ctx);
            return last_status;
        }

        let mut last_status = 0;
        let mut targets = Vec::new();
        for arg in rest {
            if !arg.starts_with('%') {
                match arg.parse::<i32>() {
                    Ok(pid) if pid > 0 => {}
                    _ => {
                        report_wait_error(ctx, &format!("`{arg}': not a pid or valid job spec"));
                        last_status = 127;
                        continue;
                    }
                }
            }
            let Some(spec) = JobSpec::parse(&arg) else {
                report_wait_error(ctx, &format!("`{arg}': not a pid or valid job spec"));
                last_status = 127;
                continue;
            };
            let pids = match resolve_target_pids(ctx, &spec, wait_any) {
                Some(pids) if !pids.is_empty() => pids,
                None => {
                    if arg.starts_with('%') {
                        report_wait_error(ctx, &format!("{arg}: no such job"));
                    } else {
                        report_wait_error(ctx, &format!("pid {arg} is not a child of this shell"));
                    }
                    last_status = 127;
                    continue;
                }
                Some(_) => continue,
            };
            targets.push(WaitTarget { pids });
        }
        if wait_any {
            let (status, winner) = wait_for_any_target(ctx, targets, force);
            if let (Some(var), Some(pid)) = (p_var, winner) {
                if let Err(err) = assign_direct_target(ctx.env(), &var, pid.to_string()) {
                    report_builtin_assign_error(ctx.env_ref(), "wait", &err);
                }
            }
            purge_waited_done(ctx);
            return status.unwrap_or(127);
        }
        for target in targets {
            match wait_for_pids(ctx, &target.pids, force) {
                Some(status) => last_status = status,
                None => {
                    if target.pids.len() == 1 {
                        report_wait_error(
                            ctx,
                            &format!("pid {} is not a child of this shell", target.pids[0]),
                        );
                    }
                    last_status = 127;
                }
            }
        }
        purge_waited_done(ctx);
        last_status
    }
}

struct WaitTarget {
    pids: Vec<i32>,
}

fn resolve_target_pids(
    ctx: &mut BuiltinCtx<'_>,
    spec: &JobSpec,
    whole_job_for_pid: bool,
) -> Option<Vec<i32>> {
    match spec {
        JobSpec::Pid(pid) if whole_job_for_pid => {
            let Some(table) = ctx.env_ref().jobs_table() else {
                return Some(vec![*pid]);
            };
            let Some(job_id) = table.job_of_pid(*pid) else {
                return Some(vec![*pid]);
            };
            let job = table.get(job_id)?;
            Some(job.processes.iter().map(|p| p.pid).collect())
        }
        JobSpec::Pid(pid) => Some(vec![*pid]),
        _ => {
            let table = ctx.env_ref().jobs_table()?;
            let jid = table.lookup(spec).ok()?;
            let job = table.get(jid)?;
            Some(job.processes.iter().map(|p| p.pid).collect())
        }
    }
}

fn report_wait_error(ctx: &BuiltinCtx<'_>, message: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: wait: {message}");
    } else {
        eprintln!("cherubsh: wait: {message}");
    }
}

fn prepare_wait_p_var(ctx: &mut BuiltinCtx<'_>, var: &str) -> bool {
    if let Some(message) = indexed_target_array_expand_once_message(ctx.env_ref(), var) {
        report_bare_diagnostic(ctx.env_ref(), &message);
        return false;
    }
    match unset_array_reference(ctx.env(), var) {
        Ok(true) => return true,
        Err(message) => {
            report_wait_error(ctx, &message);
            return false;
        }
        Ok(false) => {}
    }
    if !is_valid_name(var) {
        report_builtin_assign_error(
            ctx.env_ref(),
            "wait",
            &AssignError::InvalidName(var.to_string()),
        );
        return false;
    }
    if ctx.env_ref().is_readonly(var) {
        report_wait_error(ctx, &format!("{var}: cannot unset: readonly variable"));
        return false;
    }
    ctx.env().unset(var);
    true
}

fn wait_for_children(ctx: &mut BuiltinCtx<'_>, wait_any: bool, force: bool) -> (i32, Option<i32>) {
    if wait_any {
        if let Some(completed) = take_completed_job(ctx, None) {
            return completed;
        }
    }
    if let Some(pids) = all_waitable_job_pids(ctx) {
        if pids.is_empty() {
            if !wait_any {
                mark_all_completed_jobs_waited(ctx);
            }
            return (if wait_any { 127 } else { 0 }, None);
        }
        if wait_any {
            let (status, winner) = wait_for_any_target(ctx, vec![WaitTarget { pids }], force);
            return (status.unwrap_or(127), winner);
        } else {
            if let Some(status) = wait_for_pids(ctx, &pids, force) {
                if status > 128 {
                    return (status, None);
                }
            }
            mark_all_completed_jobs_waited(ctx);
            return (0, None);
        }
    }
    let mut last_status = 0;
    let mut saw_child = false;
    loop {
        let mut status: libc::c_int = 0;
        let flags = if force { 0 } else { libc::WUNTRACED };
        let pid = unsafe { libc::waitpid(-1, &mut status, flags) };
        if pid < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                if let Some(status) = run_pending_wait_trap(ctx) {
                    return (status, None);
                }
                continue;
            }
            break;
        }
        saw_child = true;
        update_job_table(ctx, pid, status);
        mark_process_waited_by_pid(ctx, pid);
        run_sigchld_trap(ctx);
        last_status = decode(status);
        if wait_any {
            return (last_status, Some(pid));
        }
    }
    if wait_any && !saw_child {
        (127, None)
    } else {
        (last_status, None)
    }
}

fn all_waitable_job_pids(ctx: &BuiltinCtx<'_>) -> Option<Vec<i32>> {
    let table = ctx.env_ref().jobs_table()?;
    let pids = table
        .list()
        .iter()
        .filter(|job| job.state == JobState::Running || job.state == JobState::Stopped)
        .flat_map(|job| {
            job.processes
                .iter()
                .filter(|process| {
                    process.state == JobState::Running || process.state == JobState::Stopped
                })
                .map(|process| process.pid)
        })
        .collect::<Vec<_>>();
    Some(pids)
}

fn wait_for_pids(ctx: &mut BuiltinCtx<'_>, pids: &[i32], force: bool) -> Option<i32> {
    let mut last_status = None;
    let mut trapped_children = 0usize;
    let mut waited_pids = Vec::new();
    for pid in pids {
        if let Some(status) = completed_pid_status(ctx, *pid) {
            last_status = Some(status);
            waited_pids.push(*pid);
            continue;
        }
        let mut status: libc::c_int = 0;
        loop {
            let flags = libc::WNOHANG | if force { 0 } else { libc::WUNTRACED };
            let rc = unsafe { libc::waitpid(*pid, &mut status, flags) };
            if rc == 0 {
                if let Some(status) = run_pending_wait_trap(ctx) {
                    return Some(status);
                }
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            if rc < 0 {
                let errno = unsafe { *libc::__errno_location() };
                if errno == libc::EINTR {
                    if let Some(status) = run_pending_wait_trap(ctx) {
                        return Some(status);
                    }
                    continue;
                }
                break;
            }
            update_job_table(ctx, rc, status);
            trapped_children += 1;
            last_status = Some(decode(status));
            waited_pids.push(rc);
            break;
        }
    }
    for pid in waited_pids {
        mark_process_waited_by_pid(ctx, pid);
    }
    for _ in 0..trapped_children {
        run_sigchld_trap(ctx);
    }
    last_status
}

fn wait_for_any_target(
    ctx: &mut BuiltinCtx<'_>,
    targets: Vec<WaitTarget>,
    force: bool,
) -> (Option<i32>, Option<i32>) {
    let mut pids: Vec<i32> = targets.into_iter().flat_map(|t| t.pids).collect();
    if let Some((status, pid)) = take_completed_job(ctx, Some(&pids)) {
        return (Some(status), pid);
    }
    while !pids.is_empty() {
        let mut status: libc::c_int = 0;
        let flags = libc::WNOHANG | if force { 0 } else { libc::WUNTRACED };
        let rc = unsafe { libc::waitpid(-1, &mut status, flags) };
        if rc > 0 {
            update_job_table(ctx, rc, status);
            run_sigchld_trap(ctx);
            if let Some((status, winner)) = take_completed_job(ctx, Some(&pids)) {
                return (Some(status), winner);
            }
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                pids.retain(|pid| *pid != rc);
            }
            continue;
        }
        if rc < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                if let Some(status) = run_pending_wait_trap(ctx) {
                    return (Some(status), None);
                }
                continue;
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    (None, None)
}

fn run_pending_wait_trap(ctx: &mut BuiltinCtx<'_>) -> Option<i32> {
    for sig in 1..NSIG {
        let sig = sig as i32;
        if sig == libc::SIGCHLD {
            continue;
        }
        let count = ctx.env().pending_signal_take(sig);
        if count == 0 {
            continue;
        }
        if let Some(TrapAction::Command(body)) = ctx.env_ref().trap_action(TrapKind::Numeric(sig)) {
            for _ in 0..count {
                let saved = ctx.env_ref().running_trap();
                ctx.env().set_running_trap(Some(sig));
                let _ = ctx.shell.run_source(&body);
                ctx.env().set_running_trap(saved);
            }
            return Some(128 + sig);
        }
    }
    None
}

fn update_job_table(ctx: &mut BuiltinCtx<'_>, pid: i32, status: libc::c_int) {
    if let Some(table) = ctx.env().jobs_table_mut() {
        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            table.mark_dead(pid, status);
        } else if libc::WIFSTOPPED(status) {
            table.mark_stopped(pid, libc::WSTOPSIG(status));
        }
    }
}

fn run_sigchld_trap(ctx: &mut BuiltinCtx<'_>) {
    let body = match ctx.env_ref().trap_action(TrapKind::Numeric(libc::SIGCHLD)) {
        Some(TrapAction::Command(body)) => body,
        _ => return,
    };
    let _ = ctx.shell.run_source(&body);
}

fn purge_waited_done(ctx: &mut BuiltinCtx<'_>) {
    let ids: Vec<_> = {
        ctx.env_ref()
            .jobs_table()
            .map(|table| {
                table
                    .list()
                    .iter()
                    .filter(|job| {
                        job.state == JobState::Done && job.flags.contains(JobFlags::WAITED_FOR)
                    })
                    .map(|job| job.id)
                    .collect()
            })
            .unwrap_or_default()
    };
    if let Some(table) = ctx.env().jobs_table_mut() {
        for id in ids {
            table.remove(id);
        }
    }
    run_coproc_cleanups(ctx);
}

fn take_completed_job(
    ctx: &mut BuiltinCtx<'_>,
    allowed_pids: Option<&[i32]>,
) -> Option<(i32, Option<i32>)> {
    let (status, winner, pids) = {
        let table = ctx.env_ref().jobs_table()?;
        let job = table
            .list()
            .iter()
            .filter(|job| {
                job.state == JobState::Done
                    && !job.flags.contains(JobFlags::WAITED_FOR)
                    && allowed_pids.is_none_or(|allowed| {
                        job.processes
                            .iter()
                            .any(|process| allowed.contains(&process.pid))
                    })
            })
            .min_by_key(|job| table.completion_order(job.id))?;
        let status = job.exit_status.unwrap_or_else(|| {
            job.processes
                .last()
                .map(|process| decode(process.status_raw))
                .unwrap_or(0)
        });
        let winner = job
            .processes
            .last()
            .map(|process| process.pid)
            .unwrap_or(job.leader_pid);
        let pids = job
            .processes
            .iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>();
        (status, winner, pids)
    };
    if let Some(table) = ctx.env().jobs_table_mut() {
        for pid in pids {
            table.mark_pid_waited(pid);
        }
    }
    Some((status, Some(winner)))
}

fn completed_pid_status(ctx: &BuiltinCtx<'_>, pid: i32) -> Option<i32> {
    let table = ctx.env_ref().jobs_table()?;
    if table.pid_was_waited(pid) {
        return None;
    }
    let job = table.get(table.job_of_pid(pid)?)?;
    let process = job.processes.iter().find(|process| process.pid == pid)?;
    (process.state == JobState::Done).then(|| decode(process.status_raw))
}

fn mark_process_waited_by_pid(ctx: &mut BuiltinCtx<'_>, pid: i32) {
    let Some(table) = ctx.env().jobs_table_mut() else {
        return;
    };
    table.mark_pid_waited(pid);
}

fn mark_all_completed_jobs_waited(ctx: &mut BuiltinCtx<'_>) {
    let pids = ctx
        .env_ref()
        .jobs_table()
        .map(|table| {
            table
                .list()
                .iter()
                .filter(|job| job.state == JobState::Done)
                .flat_map(|job| job.processes.iter().map(|process| process.pid))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(table) = ctx.env().jobs_table_mut() {
        for pid in pids {
            table.mark_pid_waited(pid);
        }
    }
}

fn run_coproc_cleanups(ctx: &mut BuiltinCtx<'_>) {
    let cleanups = ctx.env().take_coproc_cleanups();
    for (name, pid_name) in cleanups {
        report_cannot_unset_readonly_error(ctx, &name);
        if let Some(pid_name) = pid_name {
            ctx.env().unset(&pid_name);
        }
    }
}

fn report_cannot_unset_readonly_error(ctx: &BuiltinCtx<'_>, name: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: {name}: cannot unset: readonly variable");
    } else {
        eprintln!("cherubsh: {name}: cannot unset: readonly variable");
    }
}

fn decode(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else if libc::WIFSTOPPED(status) {
        128 + libc::WSTOPSIG(status)
    } else {
        1
    }
}
