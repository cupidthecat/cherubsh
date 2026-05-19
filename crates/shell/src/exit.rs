use std::io::Write;
use std::path::PathBuf;

use cherubsh_common::Environment;

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
pub fn exit_shell(state: &mut ShellState, status: i32) -> ! {
    // Fire EXIT trap exactly once before any logout / history save.
    let status = crate::traps::run_exit_trap(state).unwrap_or(status);

    // Persist command history if configured.
    if state.interactive_shell {
        let histfile_disabled = state
            .history_table
            .iter()
            .any(|entry| entry.line.trim_end_matches('\n') == "HISTFILE=");
        if !histfile_disabled && !matches!(state.get("HISTFILE"), Some(value) if value.is_empty()) {
            if let Some(path) = state.histfile() {
                let with_ts = std::env::var("HISTTIMEFORMAT").is_ok();
                let _ = state
                    .history_table
                    .write_to(&path, state.histfilesize, with_ts);
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
