use std::ffi::CStr;

use cherubsh_common::{Environment, VarAttrs};

use crate::options::{compat_shell_version_with_build, compat_version, BUILD_VERSION, MACHTYPE};
use crate::signals::{acquire_terminal, install_job_control_signals};
use crate::state::{ShellState, StartupMode, VariableEntry};

/// bash-5.3 config-top.h: DEFAULT_PATH_VALUE.
const DEFAULT_PATH_VALUE: &str = "/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin:.";
const BASH_LOADABLES_PATH_VALUE: &str =
    "/usr/local/lib/bash:/usr/lib/bash:/opt/local/lib/bash:/usr/pkg/lib/bash:/opt/pkg/lib/bash:.";
const COMP_WORDBREAKS_VALUE: &str = " \t\n\"'@><=;|&(:";

/// init_interactive: shell.c:1845-1857.
pub fn init_interactive(state: &mut ShellState) {
    state.interactive = true;
    state.interactive_shell = true;
    state.startup_state = StartupMode::Interactive;
    state.shopt_options.insert("emacs".to_string(), true);
    state.shopt_options.insert("history".to_string(), true);
    state.shopt_options.insert("histexpand".to_string(), true);

    let mut tty_fd: Option<i32> = None;
    let mut shell_pgrp: i32 = unsafe { libc::getpgrp() };
    let mut original_pgrp: i32 = shell_pgrp;
    if acquire_terminal(&mut tty_fd, &mut shell_pgrp, &mut original_pgrp) {
        state.tty_fd_value = tty_fd;
        state.shell_pgrp_value = shell_pgrp;
        state.original_pgrp_value = original_pgrp;
        state.job_control = true;
        state.set_option("monitor", true);
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
    apply_imported_option_vars(state);

    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    bind(state, "BASH_VERSION", compat_shell_version_with_build());
    bind_bash_versinfo(state);
    bind(state, "BASH", shell_executable_path());
    bind(
        state,
        "SHELL",
        state.get("SHELL").unwrap_or_else(default_login_shell),
    );
    if state.get("BASH_LOADABLES_PATH").is_none() {
        bind(
            state,
            "BASH_LOADABLES_PATH",
            String::from(BASH_LOADABLES_PATH_VALUE),
        );
    }
    bind_readonly(state, "BASHOPTS", bashopts_value(state));
    bind(
        state,
        "COMP_WORDBREAKS",
        String::from(COMP_WORDBREAKS_VALUE),
    );
    bind(state, "HOSTNAME", hostname());
    bind(state, "HOSTTYPE", hosttype());
    bind(state, "MACHTYPE", String::from(MACHTYPE));
    bind(state, "OSTYPE", ostype());
    bind(state, "PWD", cwd.clone());
    state.logical_pwd_value = Some(cwd);
    state.export("PWD");
    bind_readonly_integer(state, "PPID", ppid.to_string());
    bind(state, "BASHPID", pid.to_string());
    state.set_attr("BASHPID", VarAttrs::INTEGER, true);
    bind_readonly_integer(state, "UID", unsafe { libc::getuid() }.to_string());
    bind_readonly_integer(state, "EUID", unsafe { libc::geteuid() }.to_string());

    let shlvl = state
        .get("SHLVL")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
        + 1;
    bind(state, "SHLVL", shlvl.to_string());
    state.export("SHLVL");

    if state.get("IFS").is_none() {
        bind(state, "IFS", String::from(" \t\n"));
    }
    if state.get("PATH").is_none() {
        bind(state, "PATH", String::from(DEFAULT_PATH_VALUE));
    }
    if state.get("OPTIND").is_none() {
        bind(state, "OPTIND", String::from("1"));
    }
    state.set_attr("OPTIND", VarAttrs::INTEGER, true);
    if state.get("OPTERR").is_none() {
        bind(state, "OPTERR", String::from("1"));
    }
    if state.interactive && state.get("MAILCHECK").is_none() {
        bind(state, "MAILCHECK", String::from("60"));
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
    if state.get("TERM").is_none() {
        bind(state, "TERM", String::from("dumb"));
    }
    bind_exported_unset(state, "OLDPWD");
    std::env::remove_var("OLDPWD");
    state.set_array("GROUPS", current_groups());
    state.groups_dynamic = true;

    state.shell_initialized = true;
}

fn apply_imported_option_vars(state: &mut ShellState) {
    if let Some(value) = state
        .variables
        .get("BASHOPTS")
        .and_then(|entry| entry.has_value.then(|| entry.value.clone()))
    {
        for name in value.split(':').filter(|name| !name.is_empty()) {
            if cherubsh_builtins::shopt_table::lookup(name).is_some() {
                state.set_option(name, true);
            }
        }
    }
    if let Some(value) = state
        .variables
        .get("SHELLOPTS")
        .and_then(|entry| entry.has_value.then(|| entry.value.clone()))
    {
        for name in value.split(':').filter(|name| !name.is_empty()) {
            if cherubsh_builtins::options::lookup_long(name).is_some() {
                state.set_option(name, true);
            }
        }
    }
}

fn shell_executable_path() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| String::from("cherubsh"))
}

fn bind(state: &mut ShellState, name: &str, value: String) {
    let exported = state.exported(name);
    state.set(name, value);
    if exported {
        state.export(name);
    }
}

fn bind_readonly(state: &mut ShellState, name: &str, value: String) {
    state.variables.insert(
        name.to_string(),
        VariableEntry {
            value,
            has_value: true,
            exported: false,
            readonly: true,
            attrs: VarAttrs::READONLY,
        },
    );
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

fn bind_exported_unset(state: &mut ShellState, name: &str) {
    state.variables.insert(
        name.to_string(),
        VariableEntry {
            value: String::new(),
            has_value: false,
            exported: true,
            readonly: false,
            attrs: VarAttrs::EXPORT,
        },
    );
}

fn bind_bash_versinfo(state: &mut ShellState) {
    let version = compat_version();
    state.set_array(
        "BASH_VERSINFO",
        vec![
            version.major.to_string(),
            version.minor.to_string(),
            version.patch.to_string(),
            BUILD_VERSION.to_string(),
            String::from("release"),
            String::from(MACHTYPE),
        ],
    );
    state.set_attr("BASH_VERSINFO", VarAttrs::READONLY, true);
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

fn default_login_shell() -> String {
    let uid = unsafe { libc::getuid() };
    let passwd = unsafe { libc::getpwuid(uid) };
    if passwd.is_null() {
        return String::from("/bin/sh");
    }
    let shell = unsafe { (*passwd).pw_shell };
    if shell.is_null() {
        return String::from("/bin/sh");
    }
    unsafe { CStr::from_ptr(shell) }
        .to_string_lossy()
        .into_owned()
}

fn hostname() -> String {
    let mut buf = [0 as libc::c_char; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    buf[buf.len() - 1] = 0;
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn hosttype() -> String {
    match std::env::consts::ARCH {
        "x86_64" => String::from("x86_64"),
        "aarch64" => String::from("aarch64"),
        arch => arch.to_string(),
    }
}

fn ostype() -> String {
    match std::env::consts::OS {
        "linux" => String::from("linux-gnu"),
        "macos" => String::from("darwin"),
        "freebsd" => String::from("freebsd"),
        "netbsd" => String::from("netbsd"),
        "openbsd" => String::from("openbsd"),
        "dragonfly" => String::from("dragonfly"),
        "solaris" => String::from("solaris"),
        "windows" => String::from("msys"),
        os => os.to_string(),
    }
}

fn bashopts_value(state: &ShellState) -> String {
    let mut names = cherubsh_builtins::shopt_table::SHOPT_OPTIONS
        .iter()
        .filter(|opt| state.option(opt.name))
        .map(|opt| opt.name)
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.join(":")
}
