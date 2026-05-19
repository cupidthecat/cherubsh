use cherubsh_common::{Environment, VarAttrs};

use crate::options::{DIST_VERSION, PATCH_LEVEL, SHELL_VERSION};
use crate::signals::{acquire_terminal, install_default_handlers, install_job_control_signals};
use crate::state::{ShellState, StartupMode, VariableEntry};

/// bash-5.2.21 config-top.h: DEFAULT_PATH_VALUE.
const DEFAULT_PATH_VALUE: &str = "/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin:.";

/// init_interactive: shell.c:1845-1857.
pub fn init_interactive(state: &mut ShellState) {
    state.interactive = true;
    state.interactive_shell = true;
    state.startup_state = StartupMode::Interactive;
    install_default_handlers(true);

    let mut tty_fd: Option<i32> = None;
    let mut shell_pgrp: i32 = unsafe { libc::getpgrp() };
    let mut original_pgrp: i32 = shell_pgrp;
    if acquire_terminal(&mut tty_fd, &mut shell_pgrp, &mut original_pgrp) {
        state.tty_fd_value = tty_fd;
        state.shell_pgrp_value = shell_pgrp;
        state.original_pgrp_value = original_pgrp;
        state.job_control = true;
        install_job_control_signals();
    }
}

pub fn load_history(state: &mut ShellState) {
    state.configure_history_from_vars(true, true);
}

/// init_noninteractive: shell.c:1860-1875.
pub fn init_noninteractive(state: &mut ShellState) {
    state.interactive = false;
    state.interactive_shell = false;
    state.no_line_editing = true;
}

/// shell_initialize: shell.c:1928+. Seed the variable table from the
/// process environment and bind BASH_* introspection variables.
pub fn shell_initialize(state: &mut ShellState) {
    if state.shell_initialized {
        return;
    }
    state.import_process_env();

    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    bind(
        state,
        "BASH_VERSION",
        format!("{SHELL_VERSION}-release(cherubsh)"),
    );
    bind(
        state,
        "BASH_VERSINFO",
        format!("({} {} {} 1 release)", DIST_VERSION, PATCH_LEVEL, 0),
    );
    bind(
        state,
        "BASH",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| String::from("cherubsh")),
    );
    bind(
        state,
        "SHELL",
        state
            .get("SHELL")
            .unwrap_or_else(|| String::from("/bin/cherubsh")),
    );
    bind(state, "PWD", cwd);
    bind(state, "PPID", ppid.to_string());
    bind(state, "BASHPID", pid.to_string());
    bind_readonly_integer(state, "UID", unsafe { libc::getuid() }.to_string());
    bind_readonly_integer(state, "EUID", unsafe { libc::geteuid() }.to_string());

    let shlvl = state
        .get("SHLVL")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
        + 1;
    bind(state, "SHLVL", shlvl.to_string());

    if state.get("IFS").is_none() {
        bind(state, "IFS", String::from(" \t\n"));
    }
    if state.get("PATH").is_none() {
        bind(state, "PATH", String::from(DEFAULT_PATH_VALUE));
    }
    if state.get("OPTIND").is_none() {
        bind(state, "OPTIND", String::from("1"));
    }
    if state.get("OPTERR").is_none() {
        bind(state, "OPTERR", String::from("1"));
    }
    if state.get("PS1").is_none() {
        bind(state, "PS1", String::from("\\s-\\v\\$ "));
    }
    if state.get("PS2").is_none() {
        bind(state, "PS2", String::from("> "));
    }
    if state.get("PS4").is_none() {
        bind(state, "PS4", String::from("+ "));
    }
    state.set_array("GROUPS", current_groups());

    state.shell_initialized = true;
}

fn bind(state: &mut ShellState, name: &str, value: String) {
    let exported = state.exported(name);
    state.set(name, value);
    if exported {
        state.export(name);
    }
}

fn bind_readonly_integer(state: &mut ShellState, name: &str, value: String) {
    state.variables.insert(
        name.to_string(),
        VariableEntry {
            value,
            has_value: true,
            exported: false,
            readonly: true,
            attrs: VarAttrs::READONLY | VarAttrs::INTEGER,
        },
    );
}

fn current_groups() -> Vec<String> {
    let egid = unsafe { libc::getegid() }.to_string();
    let mut groups = vec![egid.clone()];
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if count > 0 {
        let mut raw = vec![0 as libc::gid_t; count as usize];
        let got = unsafe { libc::getgroups(raw.len() as i32, raw.as_mut_ptr()) };
        if got > 0 {
            raw.truncate(got as usize);
            for gid in raw.into_iter().map(|gid| gid.to_string()) {
                if !groups.iter().any(|existing| existing == &gid) {
                    groups.push(gid);
                }
            }
        }
    }
    groups
}
