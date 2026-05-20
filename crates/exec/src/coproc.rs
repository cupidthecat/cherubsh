use cherubsh_builtins::common::is_valid_name;
use cherubsh_common::jobs::Process;
use cherubsh_common::{AssignError, Environment, JobState, VarAttrs};
use cherubsh_parser::CoprocCommand;

use crate::util::reset_child_signal_handlers;
use crate::ExecContext;

const COPROC_READ_FD: i32 = 63;
const COPROC_WRITE_FD: i32 = 60;

pub(crate) fn execute<'a>(ctx: &mut ExecContext<'a>, coproc: &CoprocCommand) -> i32 {
    let name = coproc
        .name
        .as_ref()
        .map(|word| word.text.as_str())
        .unwrap_or("COPROC");
    let Some(storage_name) = coproc_storage_name(ctx.env, name) else {
        return 1;
    };

    let mut to_child = [0i32; 2];
    let mut from_child = [0i32; 2];
    if unsafe { libc::pipe(to_child.as_mut_ptr()) } < 0 {
        eprintln!("cherubsh: coproc: pipe failed");
        return 1;
    }
    if unsafe { libc::pipe(from_child.as_mut_ptr()) } < 0 {
        unsafe {
            libc::close(to_child[0]);
            libc::close(to_child[1]);
        }
        eprintln!("cherubsh: coproc: pipe failed");
        return 1;
    }

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        unsafe {
            libc::close(to_child[0]);
            libc::close(to_child[1]);
            libc::close(from_child[0]);
            libc::close(from_child[1]);
        }
        eprintln!("cherubsh: coproc: fork failed");
        return 1;
    }

    if pid == 0 {
        unsafe {
            libc::setpgid(0, 0);
            libc::close(to_child[1]);
            libc::close(from_child[0]);
            libc::dup2(to_child[0], 0);
            libc::dup2(from_child[1], 1);
            libc::close(to_child[0]);
            libc::close(from_child[1]);
        }
        reset_child_signal_handlers(ctx.env);
        ctx.env.enter_subshell();
        let status = ctx.execute_child_command(&coproc.command);
        let final_status = match ctx.pending.take() {
            Some(crate::Unwind::Exit(n)) => n,
            _ => status,
        };
        unsafe { libc::_exit(final_status) };
    }

    unsafe {
        libc::setpgid(pid, pid);
        libc::close(to_child[0]);
        libc::close(from_child[1]);
    }
    let read_fd = move_fd_to(from_child[0], COPROC_READ_FD);
    let write_fd = move_fd_to(to_child[1], COPROC_WRITE_FD);

    if ctx.env.is_readonly(&storage_name) {
        report_readonly_error(ctx.env, &storage_name);
        ctx.env
            .queue_coproc_cleanup(storage_name.clone(), Some(format!("{storage_name}_PID")));
    } else {
        if storage_name == name && ctx.env.attrs(name).contains(VarAttrs::NAMEREF) {
            report_removing_nameref_attribute(ctx.env, name);
        }
        ctx.env.set_array(
            &storage_name,
            vec![read_fd.to_string(), write_fd.to_string()],
        );
        let pid_name = format!("{storage_name}_PID");
        if let Err(err) = ctx.env.assign(&pid_name, pid.to_string()) {
            report_assign_error(ctx.env, &err);
        }
        if name == "COPROC" {
            if let Err(err) = ctx.env.assign("COPROC_PID", pid.to_string()) {
                report_assign_error(ctx.env, &err);
            }
        }
    }
    ctx.env.set_last_async_pid(pid);

    let command_line = format!("{:?}", coproc.command.data);
    let process = Process {
        pid,
        status_raw: 0,
        state: JobState::Running,
        command: command_line.clone(),
    };
    let job_control = ctx.env.job_control_enabled();
    if let Some(table) = ctx.env.jobs_table_mut() {
        table.add(pid, pid, command_line, true, job_control, vec![process]);
    }
    0
}

fn coproc_storage_name(env: &dyn Environment, name: &str) -> Option<String> {
    if !env.attrs(name).contains(VarAttrs::NAMEREF) {
        if !is_valid_name(name) {
            report_invalid_identifier_error(env, name);
            return None;
        }
        return Some(name.to_string());
    }
    let target = env
        .resolve_nameref(name)
        .unwrap_or_else(|| name.to_string());
    if is_array_reference(&target) {
        report_invalid_identifier_error(env, &target);
        return None;
    }
    if !is_valid_name(&target) {
        report_invalid_identifier_error(env, &target);
        return None;
    }
    Some(target)
}

fn is_array_reference(value: &str) -> bool {
    let Some(open) = value.find('[') else {
        return false;
    };
    open > 0 && value.ends_with(']')
}

fn report_invalid_identifier_error(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: `{name}': not a valid identifier");
    } else {
        eprintln!("cherubsh: `{name}': not a valid identifier");
    }
}

fn report_assign_error(env: &dyn Environment, err: &AssignError) {
    match err {
        AssignError::ReadOnly(name) => report_readonly_error(env, name),
        AssignError::InvalidName(name) => report_invalid_identifier_error(env, name),
        AssignError::BadArraySubscript(name) => {
            if let (Some(source), Some(line)) =
                (env.diagnostic_source_name(), env.diagnostic_line())
            {
                eprintln!("{source}: line {line}: {name}: bad array subscript");
            } else {
                eprintln!("cherubsh: {name}: bad array subscript");
            }
        }
        AssignError::InvalidInteger(value) => {
            if let (Some(source), Some(line)) =
                (env.diagnostic_source_name(), env.diagnostic_line())
            {
                eprintln!("{source}: line {line}: {value}: invalid integer");
            } else {
                eprintln!("cherubsh: {value}: invalid integer");
            }
        }
        AssignError::CircularNameReference(_) => {}
    }
}

fn report_readonly_error(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {name}: readonly variable");
    } else {
        eprintln!("cherubsh: {name}: readonly variable");
    }
}

fn report_removing_nameref_attribute(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: warning: {name}: removing nameref attribute");
    } else {
        eprintln!("cherubsh: warning: {name}: removing nameref attribute");
    }
}

fn move_fd_to(source: i32, target: i32) -> i32 {
    if source == target {
        return target;
    }
    if unsafe { libc::dup2(source, target) } >= 0 {
        unsafe {
            libc::close(source);
        }
        target
    } else {
        source
    }
}
