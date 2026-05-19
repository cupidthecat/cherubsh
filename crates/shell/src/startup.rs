use std::path::{Path, PathBuf};

use cherubsh_common::Environment;
use cherubsh_expander::{expand_string_to_string, NullRunner};

use crate::input::BashInput;
use crate::reader_loop::run_until_eof;
use crate::state::ShellState;

const SYS_PROFILE: &str = "/etc/profile";

/// Source a file if it exists and is readable. Mirrors maybe_execute_file.
pub fn maybe_source(path: &Path, state: &mut ShellState) -> bool {
    let input = match BashInput::from_file(path) {
        Ok(value) => value,
        Err(_) => return false,
    };
    state.push_input(input);
    run_until_eof(state);
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
    let mut runner = NullRunner::default();
    let expanded = expand_string_to_string(&raw, state, &mut runner).unwrap_or(raw);
    if expanded.is_empty() {
        None
    } else {
        Some(PathBuf::from(expanded))
    }
}

fn source_login_files(state: &mut ShellState) {
    state.no_rc = true;
    if state.no_profile {
        return;
    }
    maybe_source(Path::new(SYS_PROFILE), state);
    if state.act_like_sh {
        if let Some(profile) = home_path(".profile") {
            maybe_source(&profile, state);
        }
    } else {
        for candidate in [".bash_profile", ".bash_login", ".profile"] {
            if let Some(path) = home_path(candidate) {
                if maybe_source(&path, state) {
                    break;
                }
            }
        }
    }
}

/// run_startup_files: full port of shell.c:1124-1260.
pub fn run_startup_files(state: &mut ShellState) {
    let mut sourced_login = false;

    // A non-interactive shell only runs login startup files for --login/-l
    // in this local bash build (NON_INTERACTIVE_LOGIN_SHELLS is disabled).
    if state.login_shell < 0 && !state.posixly_correct {
        source_login_files(state);
        sourced_login = true;
    }

    // Non-interactive: BASH_ENV (and return)
    if !state.interactive_shell {
        if !(state.su_shell && state.login_shell != 0) {
            if !state.posixly_correct
                && !state.act_like_sh
                && !state.privileged_mode
                && state.sourced_env == 0
            {
                state.sourced_env += 1;
                if let Some(path) = expanded_env_path("BASH_ENV", state) {
                    maybe_source(&path, state);
                }
            }
        }
        return;
    }

    // Interactive shell or `-su' shell.
    if !state.posixly_correct {
        if state.login_shell != 0 && !sourced_login {
            source_login_files(state);
        }

        if !state.act_like_sh && !state.no_rc {
            let bashrc = state.bashrc_file.clone();
            maybe_source(&bashrc, state);
        } else if state.act_like_sh && !state.privileged_mode && state.sourced_env == 0 {
            state.sourced_env += 1;
            if let Some(path) = expanded_env_path("ENV", state) {
                maybe_source(&path, state);
            }
        }
    } else {
        if !state.privileged_mode && state.sourced_env == 0 {
            state.sourced_env += 1;
            if let Some(path) = expanded_env_path("ENV", state) {
                maybe_source(&path, state);
            }
        }
    }
}
