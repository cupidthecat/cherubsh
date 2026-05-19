use std::os::unix::io::RawFd;

use cherubsh_common::jobs::Process;
use cherubsh_common::JobState;
use cherubsh_parser::{
    Command, CommandData, CONN_AMP, CONN_AND_AND, CONN_BAR_AND, CONN_NEWLINE, CONN_OR_OR,
    CONN_PIPE, CONN_SEMI,
};

use crate::util::{
    decode_wait_status, reset_child_signal_handlers, tcsetpgrp_blocked, wait_for_pid,
    wait_for_pid_ignoring_stops,
};
use crate::{ExecContext, ExecMode};

pub(crate) fn execute<'a>(
    ctx: &mut ExecContext<'a>,
    commands: Vec<Command>,
    pipe_stderr: bool,
) -> i32 {
    if commands.is_empty() {
        return 0;
    }
    if commands.len() == 1 {
        return ctx.execute_command(&commands[0], ExecMode::Parent);
    }

    let job_control = ctx.env.job_control_enabled();
    let tty_fd = ctx.env.tty_fd();
    let shell_pgrp = ctx.env.shell_pgrp();
    let lastpipe = ctx.env.option("lastpipe") && !job_control;
    let total = commands.len();
    let stages_to_fork = if lastpipe { total - 1 } else { total };
    let saved_stdin = if lastpipe {
        Some(unsafe { libc::dup(0) })
    } else {
        None
    };

    let mut pids: Vec<libc::pid_t> = Vec::with_capacity(total);
    let mut previous_read: Option<RawFd> = None;
    let mut pgid: Option<libc::pid_t> = None;

    for (index, command) in commands.iter().enumerate().take(stages_to_fork) {
        let is_last_fork = index + 1 == stages_to_fork && !lastpipe;
        let mut pipe_fds = [0i32; 2];
        let needs_pipe = index + 1 < total;
        if needs_pipe {
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } < 0 {
                return 1;
            }
        }
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return 1;
        }
        if pid == 0 {
            // child: set process group then reset signal dispositions.
            unsafe {
                let target_pgid = pgid.unwrap_or(0);
                libc::setpgid(0, target_pgid);
            }
            ctx.env.enter_subshell();
            ctx.env.suppress_inherited_exit_trap();
            reset_child_signal_handlers(ctx.env);
            if let Some(read_fd) = previous_read {
                if read_fd != 0 {
                    unsafe {
                        libc::dup2(read_fd, 0);
                        libc::close(read_fd);
                    }
                }
            }
            if needs_pipe {
                unsafe {
                    libc::close(pipe_fds[0]);
                    libc::dup2(pipe_fds[1], 1);
                    if pipe_stderr {
                        libc::dup2(pipe_fds[1], 2);
                    }
                    libc::close(pipe_fds[1]);
                }
            }
            if matches!(command.data, CommandData::Subshell(_)) {
                ctx.reuse_current_subshell_for_next_dispatch();
            }
            let status = ctx.execute_command(command, ExecMode::Child);
            let mut final_status = match ctx.pending.take() {
                Some(crate::Unwind::Exit(n)) => n,
                _ => status,
            };
            ctx.env.set_last_status(final_status);
            if let Some(trap_status) = crate::trap::run_exit_trap(ctx) {
                final_status = trap_status;
            }
            unsafe { libc::_exit(final_status) };
        }
        if pgid.is_none() {
            pgid = Some(pid);
        }
        unsafe {
            libc::setpgid(pid, pgid.unwrap_or(pid));
        }
        if let Some(read_fd) = previous_read {
            unsafe { libc::close(read_fd) };
        }
        if needs_pipe {
            unsafe { libc::close(pipe_fds[1]) };
            previous_read = Some(pipe_fds[0]);
        } else {
            previous_read = None;
        }
        pids.push(pid);
        let _ = is_last_fork;
    }

    // Foreground pipeline takes the terminal if we have job control.
    if job_control {
        if let (Some(fd), Some(p)) = (tty_fd, pgid) {
            tcsetpgrp_blocked(fd, p);
        }
    }

    // Last stage runs in parent under lastpipe.
    let mut statuses: Vec<i32> = Vec::with_capacity(total);
    let mut parent_status: Option<i32> = None;
    if lastpipe {
        // dup previous_read onto stdin, save & restore
        let saved = saved_stdin.unwrap_or(-1);
        if let Some(read_fd) = previous_read {
            if read_fd != 0 {
                unsafe {
                    libc::dup2(read_fd, 0);
                    libc::close(read_fd);
                }
            }
        }
        let status = ctx.execute_command(&commands[total - 1], ExecMode::Parent);
        if saved >= 0 {
            unsafe {
                libc::dup2(saved, 0);
                libc::close(saved);
            }
        } else {
            unsafe {
                libc::close(0);
            }
        }
        parent_status = Some(status);
    }

    if ctx.env.subshell_level() > 0 {
        for pid in &pids {
            statuses.push(wait_for_pid_ignoring_stops(*pid));
        }
    } else {
        for pid in &pids {
            let mut s = 0;
            loop {
                let rc = unsafe { libc::waitpid(*pid, &mut s, libc::WUNTRACED) };
                if rc < 0 {
                    let errno = unsafe { *libc::__errno_location() };
                    if errno == libc::EINTR {
                        continue;
                    }
                    s = 1;
                    break;
                }
                break;
            }
            statuses.push(decode_wait_status(s));
        }
    }
    if let Some(status) = parent_status {
        statuses.push(status);
    }

    // Restore terminal to the shell after foreground pipeline completes.
    if job_control {
        if let Some(fd) = tty_fd {
            tcsetpgrp_blocked(fd, shell_pgrp);
        }
    }

    // PIPESTATUS array
    let pipestatus: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
    ctx.env.set_array("PIPESTATUS", pipestatus);

    let final_status = if ctx.env.option("pipefail") {
        statuses
            .iter()
            .rev()
            .find(|&&s| s != 0)
            .copied()
            .unwrap_or(0)
    } else {
        *statuses.last().unwrap_or(&0)
    };
    final_status
}

pub(crate) fn spawn_background<'a>(ctx: &mut ExecContext<'a>, command: &Command) -> i32 {
    let job_control = ctx.env.job_control_enabled();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return 1;
    }
    if pid == 0 {
        // Child: own process group, ignore INT/QUIT so a Ctrl-C at the
        // shell doesn't propagate to this async job (bash make_child).
        unsafe {
            libc::setpgid(0, 0);
        }
        reset_child_signal_handlers(ctx.env);
        ctx.env.enter_subshell();
        ctx.env.suppress_inherited_exit_trap();
        if matches!(command.data, CommandData::Subshell(_)) {
            ctx.reuse_current_subshell_for_next_dispatch();
        }
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        }
        let status = ctx.execute_command(command, ExecMode::Child);
        let final_status = match ctx.pending.take() {
            Some(crate::Unwind::Exit(n)) => n,
            _ => status,
        };
        unsafe { libc::_exit(final_status) };
    }
    // Parent: close pgid race, register in the job table.
    unsafe {
        libc::setpgid(pid, pid);
    }
    ctx.env.set_last_async_pid(pid);
    let command_line = job_command_line(command);
    let process = Process {
        pid,
        status_raw: 0,
        state: JobState::Running,
        command: command_line.clone(),
    };
    let job_id_opt = ctx
        .env
        .jobs_table_mut()
        .map(|table| table.add(pid, pid, command_line, true, job_control, vec![process]));
    if job_control && ctx.env.option("interactive") {
        if let Some(jid) = job_id_opt {
            // bash prints `[N] PID` on backgrounding when interactive.
            eprintln!("[{}] {}", jid.raw(), pid);
        }
    }
    0
}

fn job_command_line(command: &Command) -> String {
    match &command.data {
        CommandData::Simple(simple) => simple
            .words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        CommandData::Subshell(subshell) => format!("( {} )", job_command_line(&subshell.command)),
        CommandData::Connection(conn) => {
            let op = match conn.connector {
                CONN_AND_AND => "&&",
                CONN_OR_OR => "||",
                CONN_PIPE => "|",
                CONN_BAR_AND => "|&",
                CONN_SEMI | CONN_NEWLINE => ";",
                CONN_AMP => "&",
                _ => "",
            };
            if op.is_empty() {
                format!("{:?}", command.data)
            } else if conn.connector == CONN_SEMI || conn.connector == CONN_NEWLINE {
                format!(
                    "{}; {}",
                    job_command_line(&conn.first),
                    job_command_line(&conn.second)
                )
            } else {
                format!(
                    "{} {} {}",
                    job_command_line(&conn.first),
                    op,
                    job_command_line(&conn.second)
                )
            }
        }
        CommandData::Group(group) => format!("{{ {} ; }}", job_command_line(&group.command)),
        CommandData::Arith(arith) => format!("(({}))", arith.expression.text),
        CommandData::Cond(_) => "[[ ... ]]".to_string(),
        _ => format!("{:?}", command.data),
    }
}

#[allow(dead_code)]
fn _unused(_pid: libc::pid_t) -> i32 {
    wait_for_pid(_pid)
}
