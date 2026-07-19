mod completion;
mod exit;
mod input;
mod invocation;
mod lifecycle;
mod options;
mod prompt;
mod reader_loop;
mod signals;
mod startup;
mod state;
mod traps;

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use cherubsh_common::{Environment, ShellError};
use cherubsh_exec::ExecState;

use crate::exit::exit_shell;
use crate::input::BashInput;
use crate::invocation::run_source_output_mode;
use crate::lifecycle::{init_interactive, init_noninteractive, load_history, shell_initialize};
use crate::options::{
    bind_args, parse_long_options, parse_shell_options, set_shell_name,
    show_shell_invocation_usage, show_shell_usage, show_shell_version,
};
use crate::reader_loop::reader_loop_with_exec_state;
use crate::signals::{install_default_handlers, install_early_sigint};
use crate::startup::run_startup_files;
use crate::state::{ShellState, StartupMode};

const SHELL_STACK_SIZE: usize = 32 * 1024 * 1024;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn cherub_loadable_abi_link_anchor();
}

fn main() {
    unsafe {
        libc::setlocale(libc::LC_ALL, c"".as_ptr());
    }
    #[cfg(target_os = "linux")]
    unsafe {
        cherub_loadable_abi_link_anchor();
    }
    install_early_sigint();
    let big5_locale = current_locale_is_big5();
    let argv = std::env::args_os()
        .map(|arg| argv_bytes_to_shell_string(arg.as_bytes(), big5_locale))
        .collect::<Vec<_>>();
    let exit_code = std::thread::Builder::new()
        .name("cherubsh-main".to_string())
        .stack_size(SHELL_STACK_SIZE)
        .spawn(move || run_shell(argv))
        .and_then(|handle| {
            handle
                .join()
                .map_err(|_| std::io::Error::other("shell thread panicked"))
        })
        .unwrap_or(2);
    // Cannot call exit_shell here because we don't own state; use _exit directly.
    unsafe {
        libc::fflush(std::ptr::null_mut());
        libc::_exit(exit_code);
    }
}

fn run_shell(argv: Vec<String>) -> i32 {
    match run(argv) {
        Ok(code) => code,
        Err(error) => {
            error.report();
            error.code
        }
    }
}

fn argv_bytes_to_shell_string(bytes: &[u8], big5_locale: bool) -> String {
    if big5_locale {
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0usize;
        while i < bytes.len() {
            if i + 1 < bytes.len() && bytes[i] == 0xa3 && bytes[i + 1] == 0x5c {
                out.extend_from_slice("α".as_bytes());
                i += 2;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        return cherubsh_expander::quote::bytes_to_shell_string(&out);
    }
    cherubsh_expander::quote::bytes_to_shell_string(bytes)
}

fn current_locale_is_big5() -> bool {
    let locale = std::env::var("LC_ALL")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("LC_CTYPE").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LANG").ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let lower = locale.to_ascii_lowercase();
    (lower.starts_with("zh_tw.") && lower.contains("big5"))
        || (lower.starts_with("zh_hk.") && lower.contains("big5hkscs"))
}

fn parse_only(input: &str) -> i32 {
    use cherubsh_lexer::Lexer;
    use cherubsh_parser::Parser;
    let mut lexer = Lexer::new(input);
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        let stop = matches!(token.kind, cherubsh_lexer::TokenKind::End);
        tokens.push(token);
        if stop {
            break;
        }
    }
    let mut parser = Parser::new(tokens, input);
    match parser.parse_input_unit() {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("cherubsh: parse error: {}", error.message);
            2
        }
    }
}

fn script_startup_error(shell_argv0: &str, path: &Path) -> Option<i32> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) => {
            let message = match err.kind() {
                std::io::ErrorKind::NotFound => "No such file or directory".to_string(),
                std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
                _ => err.to_string(),
            };
            eprintln!("{shell_argv0}: {}: {message}", path.display());
            return Some(127);
        }
    };
    if metadata.is_dir() {
        eprintln!("{}: {}: Is a directory", path.display(), path.display());
        return Some(126);
    }
    let bytes = std::fs::read(path).unwrap_or_default();
    if bytes.iter().take(256).any(|byte| *byte == 0) {
        eprintln!(
            "{}: {}: cannot execute binary file",
            path.display(),
            path.display()
        );
        return Some(126);
    }
    None
}

fn run(argv: Vec<String>) -> Result<i32, ShellError> {
    // Handle --parse-only flag specially: lex + parse the next argument, exit
    // with 0 if it parses, non-zero otherwise. Used by parser-parity tests.
    if argv.iter().any(|a| a == "--parse-only") {
        let pos = argv.iter().position(|a| a == "--parse-only").unwrap();
        let input = argv
            .get(pos + 1)
            .cloned()
            .or_else(|| {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).ok().map(|_| buf)
            })
            .unwrap_or_default();
        return Ok(parse_only(&input));
    }

    let mut state = ShellState::default();
    let enoexec_fallback = std::env::var_os("__CHERUBSH_ENOEXEC_FALLBACK").is_some();
    if enoexec_fallback {
        std::env::remove_var("__CHERUBSH_ENOEXEC_FALLBACK");
    }
    if std::env::var_os("POSIXLY_CORRECT").is_some() || std::env::var_os("POSIX_PEDANTIC").is_some()
    {
        state.posixly_correct = true;
    }

    let zero = argv
        .first()
        .cloned()
        .unwrap_or_else(|| String::from("cherubsh"));
    set_shell_name(&zero, &mut state);
    state.shell_invocation_name = state.shell_name.clone();

    let mut arg_index = 1;
    arg_index = match parse_long_options(&argv, arg_index, &mut state) {
        Ok(index) => index,
        Err(error) => {
            report_invocation_error(&state.shell_name, &error.message);
            return Ok(error.code);
        }
    };

    if state.want_initial_help {
        show_shell_usage();
        return Ok(0);
    }
    if state.do_version {
        show_shell_version();
        return Ok(0);
    }

    arg_index = match parse_shell_options(&argv, arg_index, &mut state) {
        Ok(index) => index,
        Err(error) => {
            report_invocation_error(&state.shell_name, &error.message);
            return Ok(error.code);
        }
    };

    if state.make_login_shell {
        // shell.c:497-501: bash inverts login_shell sign to record "from flag".
        state.login_shell = if state.login_shell == 0 {
            -1
        } else {
            -state.login_shell
        };
    }

    if state.want_pending_command {
        let Some(cmd) = argv.get(arg_index).cloned() else {
            eprintln!("{}: -c: option requires an argument", state.shell_name);
            return Ok(2);
        };
        state.command_execution_string = Some(cmd);
        arg_index += 1;
    }

    use std::io::IsTerminal;
    let stdin_is_tty = std::io::stdin().is_terminal();
    let stderr_is_tty = std::io::stderr().is_terminal();
    let no_remaining = arg_index >= argv.len();
    let should_be_interactive = state.forced_interactive
        || (state.command_execution_string.is_none()
            && !state.wordexp_only
            && (no_remaining || state.read_from_stdin)
            && stdin_is_tty
            && stderr_is_tty);
    if should_be_interactive {
        init_interactive(&mut state);
    } else {
        init_noninteractive(&mut state);
    }

    state.running_setuid =
        unsafe { libc::geteuid() != libc::getuid() || libc::getegid() != libc::getgid() };
    if state.running_setuid && !state.privileged_mode {
        unsafe {
            libc::setgid(libc::getgid());
            libc::setuid(libc::getuid());
        }
        state.running_setuid = false;
    }

    install_default_handlers(state.interactive_shell);

    shell_initialize(&mut state);
    let imported_bash_argv0 = state
        .variables
        .get("BASH_ARGV0")
        .and_then(|entry| entry.has_value.then(|| entry.value.clone()))
        .filter(|value| !value.is_empty());
    let _ = enoexec_fallback;
    if !state.interactive_shell {
        state.unset("PS1");
        state.unset("PS2");
        state.interactive = false;
    } else {
        state.set_option("interactive", true);
        state.interactive = true;
    }

    // Bind positional parameters.
    if state.command_execution_string.is_some() {
        // -c command: next arg is $0, then $1..$n.
        if arg_index >= argv.len() {
            let argv0 = imported_bash_argv0.unwrap_or_else(|| zero.clone());
            state.shell_name = argv0.clone();
            state.dollar_vars = vec![argv0];
        } else {
            bind_args(&argv, arg_index, 0, &mut state);
        }
    } else if arg_index < argv.len() && !state.read_from_stdin {
        // Script invocation: argv[arg_index] is the script path = $0.
        let script_path = resolve_script_path(&argv[arg_index])
            .unwrap_or_else(|| PathBuf::from(&argv[arg_index]));
        state.shell_script_filename = Some(script_path.clone());
        state.shell_name = argv[arg_index].clone();
        bind_args(&argv, arg_index + 1, 1, &mut state);
    } else {
        let mut positionals = Vec::with_capacity(argv.len().saturating_sub(arg_index) + 1);
        positionals.push(zero.clone());
        positionals.extend(argv.iter().skip(arg_index).cloned());
        state.dollar_vars = positionals;
    }
    state.set(
        "_",
        state
            .dollar_vars
            .first()
            .cloned()
            .unwrap_or_else(|| zero.clone()),
    );
    state.set_attr("_", cherubsh_common::VarAttrs::EXPORT, false);

    let mut exec_state = ExecState::default();
    exec_state.import_exported_functions(&state);

    if !state.running_setuid {
        let old_errexit = state.errexit;
        state.errexit = false;
        run_startup_files(&mut state, &mut exec_state);
        state.errexit |= old_errexit;
    }

    if state.act_like_sh {
        state.posixly_correct = true;
        state.set("POSIXLY_CORRECT", "y".to_string());
    }

    if state.interactive_shell && state.command_execution_string.is_none() {
        load_history(&mut state);
    }

    if state.pretty_print_mode && state.interactive_shell {
        eprintln!(
            "{}: warning: pretty-printing mode ignored in interactive shells",
            state.shell_invocation_name
        );
        state.pretty_print_mode = false;
    }

    // Choose the appropriate input source. Reset EOF first because
    // startup-file sourcing leaves eof_reached set when its input stack pops.
    state.eof_reached = false;
    state.input_stack.clear();
    state.input_line_count_stack.clear();
    state.current_command_line_count = 0;
    if let Some(command) = state.command_execution_string.clone() {
        state.startup_state = StartupMode::DashC;
        state.pretty_print_mode = false;
        if let Some(status) = run_source_output_mode(&state, "-c", &command) {
            return Ok(status);
        }
        state.input = BashInput::from_string("-c", command);
    } else if let Some(path) = state.shell_script_filename.clone() {
        if let Some(status) = script_startup_error(&zero, &path) {
            return Ok(status);
        }
        if state.pretty_print_mode || state.dump_translatable_strings || state.dump_po_strings {
            let bytes = std::fs::read(&path).map_err(|err| ShellError {
                message: format!("failed to open script {}: {err}", path.display()),
                code: 127,
                span: None,
            })?;
            let source = cherubsh_expander::quote::bytes_to_shell_string(&bytes);
            if let Some(status) = run_source_output_mode(&state, &path.to_string_lossy(), &source) {
                return Ok(status);
            }
        }
        state.input = BashInput::from_file(&path).map_err(|err| ShellError {
            message: format!("failed to open script {}: {err}", path.display()),
            code: 127,
            span: None,
        })?;
    } else {
        state.read_from_stdin = true;
        if state.pretty_print_mode || state.dump_translatable_strings || state.dump_po_strings {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .map_err(|err| ShellError::with_code(err.to_string(), 1))?;
            let source = cherubsh_expander::quote::bytes_to_shell_string(&bytes);
            if let Some(status) =
                run_source_output_mode(&state, &state.shell_invocation_name, &source)
            {
                return Ok(status);
            }
        }
        state.input = BashInput::stdin();
    }

    let status = reader_loop_with_exec_state(&mut state, &mut exec_state);
    exit_shell(&mut state, &mut exec_state, status);
}

fn report_invocation_error(shell_name: &str, message: &str) {
    if message.ends_with("invalid shell option name") {
        eprintln!("{shell_name}: line 0: {message}");
    } else {
        eprintln!("{shell_name}: {message}");
        if message.ends_with("invalid option") {
            show_shell_invocation_usage(shell_name);
        }
    }
}

fn resolve_script_path(script: &str) -> Option<PathBuf> {
    let path = PathBuf::from(script);
    if script.contains('/') || path.exists() {
        return Some(path);
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(script);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_shell_name_marks_login_shell() {
        let mut state = ShellState::default();
        set_shell_name("-bash", &mut state);
        assert_eq!(state.login_shell, 1);
        assert_eq!(state.shell_name, "bash");
    }

    #[test]
    fn set_shell_name_strips_path() {
        let mut state = ShellState::default();
        set_shell_name("/usr/bin/sh", &mut state);
        assert_eq!(state.shell_name, "sh");
        assert!(state.act_like_sh);
    }

    #[test]
    fn parse_long_help_flag() {
        let mut state = ShellState::default();
        let args = vec!["cherubsh".to_string(), "--help".to_string()];
        let new_index = parse_long_options(&args, 1, &mut state).expect("parse");
        assert!(state.want_initial_help);
        assert_eq!(new_index, 2);
    }

    #[test]
    fn parse_short_c_and_s() {
        let mut state = ShellState::default();
        let args = vec![
            "cherubsh".to_string(),
            "-cs".to_string(),
            "echo ok".to_string(),
        ];
        let new_index = parse_shell_options(&args, 1, &mut state).expect("parse");
        assert!(state.want_pending_command);
        assert!(state.read_from_stdin);
        assert_eq!(new_index, 2);
    }

    #[test]
    fn parse_long_stop_at_double_dash() {
        let mut state = ShellState::default();
        let args = vec![
            "cherubsh".into(),
            "--norc".into(),
            "--".into(),
            "script".into(),
        ];
        let new_index = parse_long_options(&args, 1, &mut state).expect("parse");
        assert!(state.no_rc);
        assert_eq!(new_index, 3);
    }

    #[test]
    fn parse_short_plus_clears_flag() {
        let mut state = ShellState::default();
        let args = vec!["cherubsh".into(), "-e".into(), "+e".into()];
        let after_set = parse_shell_options(&args[..2], 1, &mut state).expect("set");
        assert!(state.errexit);
        let _ = parse_shell_options(&args, after_set, &mut state).expect("clear");
        assert!(!state.errexit);
    }

    #[test]
    fn parse_short_n_sets_noexec() {
        let mut state = ShellState::default();
        let args = vec!["cherubsh".into(), "-n".into(), "+n".into()];
        let after_set = parse_shell_options(&args[..2], 1, &mut state).expect("set");
        assert!(state.noexec);
        let _ = parse_shell_options(&args, after_set, &mut state).expect("clear");
        assert!(!state.noexec);
    }

    #[test]
    fn bind_args_for_dash_c() {
        let mut state = ShellState::default();
        let args = vec![
            "cherubsh".into(),
            "-c".into(),
            "echo".into(),
            "name".into(),
            "a".into(),
        ];
        bind_args(&args, 3, 0, &mut state);
        assert_eq!(state.dollar_vars, vec!["name", "a"]);
        assert_eq!(state.shell_name, "name");
    }
}
