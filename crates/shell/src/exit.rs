use std::io::Write;
use std::path::PathBuf;

use cherubsh_common::Environment;
use cherubsh_exec::ExecState;

use crate::state::ShellState;

/// Source `~/.bash_logout` for login shells. Mirrors shell.c:bash_logout.
pub fn bash_logout(state: &mut ShellState) {
    if state.login_shell == 0 {
        return;
    }
    let home = match std::env::var_os("HOME") {
        Some(value) => PathBuf::from(value),
        None => return,
    };
    let path = home.join(".bash_logout");
    if !path.exists() {
        return;
    }
    let _ = crate::startup::maybe_source(&path, state);
}

/// Flush stdio, run logout files, then `_exit(status)`. Mirrors exit_shell()
/// in bash. Marked never-returns.
pub fn exit_shell(state: &mut ShellState, exec_state: &mut ExecState, status: i32) -> ! {
    // Fire EXIT trap exactly once before any logout / history save.
    let status = crate::traps::run_exit_trap_with_exec_state(state, exec_state).unwrap_or(status);

    // Persist command history if configured.
    if state.interactive_shell {
        let histfile_disabled = state
            .history_table
            .iter()
            .any(|entry| entry.line.trim_end_matches('\n') == "HISTFILE=");
        if !histfile_disabled && !matches!(state.get("HISTFILE"), Some(value) if value.is_empty()) {
            if let Some(path) = state.histfile() {
                let with_ts = state.get("HISTTIMEFORMAT").is_some();
                let _ = state
                    .history_table
                    .write_to(&path, state.histfilesize, with_ts);
            }
        }
    } else if state.option("history") {
        let histfile_disabled = state
            .history_table
            .iter()
            .any(|entry| entry.line.trim_end_matches('\n') == "HISTFILE=");
        if !histfile_disabled && !matches!(state.get("HISTFILE"), Some(value) if value.is_empty()) {
            if let Some(path) = state.histfile() {
                let _ = state.history_table.append_to(&path, false);
            }
        }
    }

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    if state.interactive_shell {
        bash_logout(state);
    }
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    unsafe {
        libc::_exit(status);
    }
}
