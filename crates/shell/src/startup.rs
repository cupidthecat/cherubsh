use std::path::{Path, PathBuf};

use cherubsh_common::Environment;
use cherubsh_exec::ExecState;
use cherubsh_expander::{expand_string_to_string, NullRunner};

use crate::input::BashInput;
use crate::reader_loop::run_until_eof_with_exec_state;
use crate::state::ShellState;

const SYS_PROFILE: &str = "/etc/profile";

/// Sources a file when it exists and can be read.
pub fn maybe_source(path: &Path, state: &mut ShellState) -> bool {
    let mut exec_state = ExecState::default();
    exec_state.import_exported_functions(state);
    maybe_source_with_exec_state(path, state, &mut exec_state)
}

pub fn maybe_source_with_exec_state(
    path: &Path,
    state: &mut ShellState,
    exec_state: &mut ExecState,
) -> bool {
    let input = match BashInput::from_file(path) {
        Ok(value) => value,
        Err(_) => return false,
    };
    state.push_input(input);
    run_until_eof_with_exec_state(state, exec_state);
    state.pop_input();
    true
}

fn home_path(component: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(component);
    Some(path)
}

fn expanded_env_path(name: &str, state: &mut ShellState) -> Option<PathBuf> {
    let raw = state.get(name)?;
    if raw.is_empty() {
        return None;
    }
    let mut runner = NullRunner;
    let expanded = expand_string_to_string(&raw, state, &mut runner).unwrap_or(raw);
    if expanded.is_empty() {
        None
    } else {
        Some(PathBuf::from(expanded))
    }
}

fn source_login_files(state: &mut ShellState, exec_state: &mut ExecState) {
    state.no_rc = true;
    if state.no_profile {
        return;
    }
    maybe_source_with_exec_state(Path::new(SYS_PROFILE), state, exec_state);
    if state.act_like_sh {
        if let Some(profile) = home_path(".profile") {
            maybe_source_with_exec_state(&profile, state, exec_state);
        }
    } else {
        for candidate in [".bash_profile", ".bash_login", ".profile"] {
            if let Some(path) = home_path(candidate) {
                if maybe_source_with_exec_state(&path, state, exec_state) {
                    break;
                }
            }
        }
    }
}

/// Loads the startup files selected for the current shell mode.
pub fn run_startup_files(state: &mut ShellState, exec_state: &mut ExecState) {
    let mut sourced_login = false;

    // A non-interactive shell only runs login startup files for --login/-l
    // in this local bash build (NON_INTERACTIVE_LOGIN_SHELLS is disabled).
    if state.login_shell < 0 && !state.posixly_correct {
        source_login_files(state, exec_state);
        sourced_login = true;
    }

    // Non-interactive: BASH_ENV (and return)
    if !state.interactive_shell {
        let skip_bash_env = state.su_shell && state.login_shell != 0
            || state.posixly_correct
            || state.act_like_sh
            || state.privileged_mode
            || state.sourced_env != 0;
        if !skip_bash_env {
            state.sourced_env += 1;
            if let Some(path) = expanded_env_path("BASH_ENV", state) {
                maybe_source_with_exec_state(&path, state, exec_state);
            }
        }
        return;
    }

    // Interactive shell or `-su' shell.
    if !state.posixly_correct {
        if state.login_shell != 0 && !sourced_login {
            source_login_files(state, exec_state);
        }

        if !state.act_like_sh && !state.no_rc {
            let rc_file = state.rc_file.clone();
            maybe_source_with_exec_state(&rc_file, state, exec_state);
        } else if state.act_like_sh && !state.privileged_mode && state.sourced_env == 0 {
            state.sourced_env += 1;
            if let Some(path) = expanded_env_path("ENV", state) {
                maybe_source_with_exec_state(&path, state, exec_state);
            }
        }
    } else if !state.privileged_mode && state.sourced_env == 0 {
        state.sourced_env += 1;
        if let Some(path) = expanded_env_path("ENV", state) {
            maybe_source_with_exec_state(&path, state, exec_state);
        }
    }
}
