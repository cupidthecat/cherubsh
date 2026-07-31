use std::ffi::CString;
use std::path::PathBuf;

use cherubsh_common::{AssignError, Environment, Span, TrapAction, TrapKind, VarAttrs, VarKind};
use cherubsh_expander::assignment::ExpandedAssignment;
use cherubsh_expander::quote::shell_string_to_cstring_bytes;
use cherubsh_expander::{
    expand_heredoc_string, expand_word_list_with_proc_subst, ExpandError, ExpandFlags,
};
use cherubsh_parser::WordDesc;

use crate::runner::ExecRunner;
use crate::ExecContext;

pub(crate) fn expand_words(words: &[WordDesc], ctx: &mut ExecContext<'_>) -> Vec<String> {
    match try_expand_words(words, ctx) {
        Ok(words) => words,
        Err(err) => {
            if !err.already_reported() {
                report_expand_error(ctx.env, err, None);
            }
            words.iter().map(|w| w.text.clone()).collect()
        }
    }
}

pub(crate) fn try_expand_words(
    words: &[WordDesc],
    ctx: &mut ExecContext<'_>,
) -> Result<Vec<String>, ExpandError> {
    let mut runner = ExecRunner::with_functions_mut_at_depth(
        &mut ctx.functions,
        &mut ctx.function_sources,
        ctx.function_depth,
        ctx.source_depth,
    );
    let out = expand_word_list_with_proc_subst(
        words,
        ctx.env,
        &mut runner,
        ExpandFlags::SPLIT_FIELDS | ExpandFlags::EXPAND_GLOB | ExpandFlags::QUOTE_REMOVAL,
    )?;
    ctx.register_proc_subst(out.proc_subst);
    Ok(out.words.into_iter().map(|w| w.text).collect())
}

pub(crate) fn report_expand_error(env: &dyn Environment, err: ExpandError, span: Option<Span>) {
    report_expand_error_with_source_line(env, err, span, None);
}

pub(crate) fn report_expand_error_with_source_line(
    env: &dyn Environment,
    err: ExpandError,
    span: Option<Span>,
    source_line: Option<&str>,
) {
    if err.already_reported() {
        return;
    }
    let err = err.into_shell_error(span);
    match (env.diagnostic_source_name(), env.diagnostic_line()) {
        (Some(source), Some(line)) => {
            eprintln!("{source}: line {line}: {}", err.message);
            if let Some(source_line) = source_line.filter(|_| prints_source_line(&err.message)) {
                eprintln!("{source}: line {line}: `{source_line}'");
            }
        }
        _ => err.report(),
    }
}

fn prints_source_line(message: &str) -> bool {
    message.starts_with("syntax error near unexpected token `")
}

pub(crate) fn expand_one(word: &WordDesc, ctx: &mut ExecContext<'_>) -> String {
    match try_expand_one(word, ctx) {
        Ok(value) => value,
        Err(err) => {
            if !err.already_reported() {
                err.into_shell_error(None).report();
            }
            word.text.clone()
        }
    }
}

pub(crate) fn try_expand_one(
    word: &WordDesc,
    ctx: &mut ExecContext<'_>,
) -> Result<String, ExpandError> {
    let mut runner = ExecRunner::with_functions_mut_at_depth(
        &mut ctx.functions,
        &mut ctx.function_sources,
        ctx.function_depth,
        ctx.source_depth,
    );
    let out = expand_word_list_with_proc_subst(
        std::slice::from_ref(word),
        ctx.env,
        &mut runner,
        ExpandFlags::QUOTE_REMOVAL,
    )?;
    ctx.register_proc_subst(out.proc_subst);
    Ok(out
        .words
        .into_iter()
        .next()
        .map(|w| w.text)
        .unwrap_or_default())
}

pub(crate) fn expand_herestring_word(word: &WordDesc, ctx: &mut ExecContext<'_>) -> String {
    let mut runner = ExecRunner::with_functions_mut_at_depth(
        &mut ctx.functions,
        &mut ctx.function_sources,
        ctx.function_depth,
        ctx.source_depth,
    );
    match expand_word_list_with_proc_subst(
        std::slice::from_ref(word),
        ctx.env,
        &mut runner,
        ExpandFlags::QUOTE_REMOVAL,
    ) {
        Ok(out) => {
            ctx.register_proc_subst(out.proc_subst);
            out.words
                .into_iter()
                .map(|word| word.text)
                .collect::<Vec<_>>()
                .join(" ")
        }
        Err(err) => {
            if !err.already_reported() {
                err.into_shell_error(None).report();
            }
            word.text.clone()
        }
    }
}

pub(crate) fn expand_heredoc_body(body: &str, ctx: &mut ExecContext<'_>) -> String {
    let mut runner = ExecRunner::with_functions_mut_at_depth(
        &mut ctx.functions,
        &mut ctx.function_sources,
        ctx.function_depth,
        ctx.source_depth,
    );
    expand_heredoc_string(body, ctx.env, &mut runner).unwrap_or_else(|err| {
        report_expand_error(ctx.env, err, None);
        String::new()
    })
}

pub(crate) fn apply_assignment(env: &mut dyn Environment, assignment: &ExpandedAssignment) -> i32 {
    match assignment {
        ExpandedAssignment::Scalar { name, value } => match env.assign(name, value.clone()) {
            Ok(()) => {
                if env.option("allexport") {
                    env.set_attr(name, VarAttrs::EXPORT, true);
                }
                0
            }
            Err(err) => {
                report_assign_error(env, &err);
                1
            }
        },
        ExpandedAssignment::IndexedElem { name, index, value } => {
            if let Some(target) = nameref_array_member_assignment_error(env, name) {
                report_invalid_identifier_error(env, &target);
                return 1;
            }
            if unbound_nameref_array_assignment(env, name) {
                report_empty_identifier_error(env);
                return 1;
            }
            if let Some(status) = prepare_self_nameref_array_assignment(env, name) {
                if status != 0 {
                    return status;
                }
            }
            let target = assignment_target_name(env, name);
            if env.is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            env.set_array_indexed(&target, *index, value.clone());
            0
        }
        ExpandedAssignment::AssocElem { name, key, value } => {
            if let Some(status) = apply_bash_cmds_assignment(env, name, key, value) {
                return status;
            }
            if let Some(target) = nameref_array_member_assignment_error(env, name) {
                report_invalid_identifier_error(env, &target);
                return 1;
            }
            if unbound_nameref_array_assignment(env, name) {
                report_empty_identifier_error(env);
                return 1;
            }
            if let Some(status) = prepare_self_nameref_array_assignment(env, name) {
                if status != 0 {
                    return status;
                }
            }
            let target = assignment_target_name(env, name);
            if env.is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            env.set_array_assoc(&target, key, value.clone());
            0
        }
        ExpandedAssignment::IndexedArray {
            name,
            entries,
            append,
        } => {
            if let Some(target) = nameref_array_member_assignment_error(env, name) {
                report_invalid_identifier_error(env, &target);
                return 1;
            }
            let target = assignment_target_name(env, name);
            if env.is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            if entries.is_empty() && !*append {
                reset_indexed_array_for_assignment(env, &target);
                return 0;
            }
            if entries.is_empty() && *append {
                ensure_indexed_array_for_append(env, &target);
                return 0;
            }
            let mut next = if *append {
                env.array_keys(&target)
                    .and_then(|keys| keys.into_iter().max().map(|max| max + 1))
                    .unwrap_or_else(|| if env.get(&target).is_some() { 1 } else { 0 })
            } else {
                reset_indexed_array_for_assignment(env, &target);
                0
            };
            for (index, value) in entries {
                match index {
                    Some(index) => {
                        env.set_array_indexed(&target, *index, value.clone());
                        next = *index + 1;
                    }
                    None => {
                        env.set_array_indexed(&target, next, value.clone());
                        next += 1;
                    }
                }
            }
            0
        }
        ExpandedAssignment::AssocArray {
            name,
            values,
            append,
        } => {
            if let Some(target) = nameref_array_member_assignment_error(env, name) {
                report_invalid_identifier_error(env, &target);
                return 1;
            }
            if name == "BASH_CMDS" {
                let mut status = 0;
                for (key, value) in values {
                    status |= apply_bash_cmds_assignment(env, name, key, value).unwrap_or(1);
                }
                return status;
            }
            let target = assignment_target_name(env, name);
            if env.is_readonly(&target) {
                report_readonly_error(env, &target);
                return 1;
            }
            let preserve_order = env.take_preserve_assoc_order_for_next_assignment(&target);
            let order = preserve_order.then(|| {
                let mut keys = if *append {
                    env.assoc_keys(&target).unwrap_or_default()
                } else {
                    Vec::new()
                };
                for (key, _) in values {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
                keys
            });
            if !*append {
                reset_assoc_array_for_assignment(env, &target);
            }
            if let Some(keys) = order {
                env.set_assoc_print_order(&target, Some(keys));
            } else if !*append {
                env.set_assoc_print_order(&target, None);
            }
            if values.is_empty() {
                env.set_array_assoc(&target, "", String::new());
                env.unset_array_elem(&target, "");
            }
            for (key, value) in values {
                env.set_array_assoc(&target, key, value.clone());
            }
            0
        }
    }
}

fn assignment_target_name(env: &dyn Environment, name: &str) -> String {
    if env.attrs(name).contains(VarAttrs::NAMEREF) {
        let target = env
            .resolve_nameref(name)
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| name.to_string());
        if array_reference_base(&target).is_some_and(|base| base == name) {
            name.to_string()
        } else {
            target
        }
    } else {
        name.to_string()
    }
}

fn array_reference_base(value: &str) -> Option<&str> {
    let open = value.find('[')?;
    value.ends_with(']').then_some(&value[..open])
}

fn unbound_nameref_array_assignment(env: &dyn Environment, name: &str) -> bool {
    env.attrs(name).contains(VarAttrs::NAMEREF)
        && env
            .resolve_nameref(name)
            .as_deref()
            .is_some_and(|target| target == name)
}

fn nameref_array_member_assignment_error(env: &dyn Environment, name: &str) -> Option<String> {
    if !env.attrs(name).contains(VarAttrs::NAMEREF) {
        return None;
    }
    let target = env.resolve_nameref(name)?;
    array_reference_base(&target)
        .is_some_and(|base| base != name)
        .then_some(target)
}

fn prepare_self_nameref_array_assignment(env: &mut dyn Environment, name: &str) -> Option<i32> {
    if !env.attrs(name).contains(VarAttrs::NAMEREF) {
        return None;
    }
    let target = env.resolve_nameref(name)?;
    if array_reference_base(&target).is_none_or(|base| base != name) {
        return None;
    }
    if env.get(name).as_deref() == Some(target.as_str()) {
        report_invalid_identifier_error(env, &target);
        return Some(1);
    }
    report_removing_nameref_attribute(env, name);
    env.set_attr(name, VarAttrs::NAMEREF, false);
    env.set_array(name, Vec::new());
    Some(0)
}

fn report_removing_nameref_attribute(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: warning: {name}: removing nameref attribute");
    } else {
        eprintln!("cherubsh: warning: {name}: removing nameref attribute");
    }
}

fn report_empty_identifier_error(env: &dyn Environment) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: `': not a valid identifier");
    } else {
        eprintln!("cherubsh: `': not a valid identifier");
    }
}

fn report_invalid_identifier_error(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: `{name}': not a valid identifier");
    } else {
        eprintln!("cherubsh: `{name}': not a valid identifier");
    }
}

fn reset_indexed_array_for_assignment(env: &mut dyn Environment, name: &str) {
    let attrs = env.attrs(name);
    let exported = env.exported(name);
    env.set_array(name, Vec::new());
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_attr(name, attr, true);
        }
    }
    env.set_attr(name, VarAttrs::ARRAY, true);
    if exported {
        env.export(name);
    }
}

fn ensure_indexed_array_for_append(env: &mut dyn Environment, name: &str) {
    let attrs = env.attrs(name);
    let exported = env.exported(name);
    if matches!(env.kind(name), VarKind::Indexed) && env.array_keys(name).is_some() {
        return;
    }
    let values = env.get(name).map(|value| vec![value]).unwrap_or_default();
    env.set_array(name, values);
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_attr(name, attr, true);
        }
    }
    env.set_attr(name, VarAttrs::ARRAY, true);
    if exported {
        env.export(name);
    }
}

fn reset_assoc_array_for_assignment(env: &mut dyn Environment, name: &str) {
    let attrs = env.attrs(name);
    let exported = env.exported(name);
    env.unset(name);
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) {
            env.set_attr(name, attr, true);
        }
    }
    env.set_attr(name, VarAttrs::ASSOC, true);
    if exported {
        env.export(name);
    }
}

fn apply_bash_cmds_assignment(
    env: &mut dyn Environment,
    name: &str,
    key: &str,
    value: &str,
) -> Option<i32> {
    if name != "BASH_CMDS" {
        return None;
    }
    if env.option("restricted") && value.contains('/') {
        cherubsh_builtins::common::report_diagnostic(env, value, "restricted");
        return Some(1);
    }
    if value.contains('/') {
        env.hash_set(key, PathBuf::from(value));
        return Some(0);
    }
    let Some(path) = search_path(value, env) else {
        cherubsh_builtins::common::report_diagnostic(env, value, "not found");
        return Some(1);
    };
    env.hash_set(key, path);
    Some(0)
}

fn report_assign_error(env: &dyn Environment, err: &AssignError) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        match err {
            AssignError::ReadOnly(name) => {
                eprintln!("{source}: line {line}: {name}: readonly variable");
                return;
            }
            AssignError::BadArraySubscript(name) => {
                eprintln!("{source}: line {line}: {name}: bad array subscript");
                return;
            }
            AssignError::InvalidName(name) => {
                eprintln!("{source}: line {line}: `{name}': not a valid identifier");
                return;
            }
            AssignError::CircularNameReference(_) => return,
            _ => {}
        }
    }
    err.report();
}

fn report_readonly_error(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: {name}: readonly variable");
    } else {
        eprintln!("cherubsh: {name}: readonly variable");
    }
}

pub(crate) fn wait_for_pid(pid: libc::pid_t) -> i32 {
    wait_for_pid_status(pid).0
}

pub(crate) fn wait_for_pid_status(pid: libc::pid_t) -> (i32, Option<i32>) {
    let mut status = 0;
    loop {
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if rc < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                continue;
            }
            return (1, None);
        }
        if libc::WIFSTOPPED(status) {
            // Foreground process stopped (e.g. ^Z): return immediately so
            // the shell regains the terminal. Status follows bash 128+sig.
            return (128 + libc::WSTOPSIG(status), Some(status));
        }
        return (decode_wait_status(status), Some(status));
    }
}

pub(crate) fn wait_for_pid_ignoring_stops(pid: libc::pid_t) -> i32 {
    let mut status = 0;
    loop {
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WCONTINUED) };
        if rc < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                continue;
            }
            return 1;
        }
        if libc::WIFSTOPPED(status) || libc::WIFCONTINUED(status) {
            continue;
        }
        return decode_wait_status(status);
    }
}

pub(crate) fn decode_wait_status(status: i32) -> i32 {
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

/// Reset signal handlers to defaults inside child forks. Mirrors the
/// `default_tty_job_signals` calls bash makes inside `make_child`.
pub(crate) fn reset_child_signal_handlers(env: &dyn Environment) {
    for sig in [
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGQUIT,
        libc::SIGCHLD,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGPIPE,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGWINCH,
        libc::SIGALRM,
    ] {
        unsafe {
            if matches!(
                env.trap_action(TrapKind::Numeric(sig)),
                Some(TrapAction::Ignore)
            ) {
                libc::signal(sig, libc::SIG_IGN);
            } else {
                libc::signal(sig, libc::SIG_DFL);
            }
        }
    }
}

/// `tcsetpgrp` with SIGTTOU/SIGTTIN/SIGTSTP blocked so the shell doesn't
/// self-suspend when handing terminal back to itself.
pub(crate) fn tcsetpgrp_blocked(fd: i32, pgrp: i32) {
    let _guard = cherubsh_common::signals::SignalMaskGuard::block_terminal_handoff();
    unsafe {
        libc::tcsetpgrp(fd, pgrp);
    }
}

pub(crate) fn execv_or_script(
    path: &str,
    argv: &[String],
    fallback_globskipdots_off: bool,
    env: &dyn Environment,
) -> i32 {
    let cstrings = argv
        .iter()
        .map(|arg| {
            CString::new(shell_string_to_cstring_bytes(arg))
                .unwrap_or_else(|_| CString::new("").unwrap())
        })
        .collect::<Vec<_>>();
    let mut raw = cstrings.iter().map(|arg| arg.as_ptr()).collect::<Vec<_>>();
    raw.push(std::ptr::null());
    let cpath = CString::new(shell_string_to_cstring_bytes(path))
        .unwrap_or_else(|_| CString::new("").unwrap());
    unsafe {
        libc::execv(cpath.as_ptr(), raw.as_ptr());
    }

    let errno = std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or_default();
    if errno != libc::ENOEXEC {
        if errno == libc::ENOENT {
            if let Some(interpreter) = shebang_interpreter(path) {
                let source = env
                    .diagnostic_source_name()
                    .unwrap_or_else(|| "cherubsh".to_string());
                eprintln!(
                    "{source}: {path}: {interpreter}: bad interpreter: No such file or directory"
                );
                return 126;
            }
            if let (Some(source), Some(line)) =
                (env.diagnostic_source_name(), env.diagnostic_line())
            {
                eprintln!("{source}: line {line}: {path}: No such file or directory");
            } else {
                eprintln!("{path}: No such file or directory");
            }
        }
        return if errno == libc::ENOENT { 127 } else { 126 };
    }
    if !is_text_script(path) {
        eprintln!("{path}: cannot execute binary file");
        return 126;
    }

    let shell = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/proc/self/exe".to_string());
    let mut shell_argv = Vec::with_capacity(argv.len() + 1);
    shell_argv.push(shell.clone());
    shell_argv.push(path.to_string());
    shell_argv.extend(argv.iter().skip(1).cloned());
    let shell_cstrings = shell_argv
        .iter()
        .map(|arg| {
            CString::new(shell_string_to_cstring_bytes(arg))
                .unwrap_or_else(|_| CString::new("").unwrap())
        })
        .collect::<Vec<_>>();
    let mut shell_raw = shell_cstrings
        .iter()
        .map(|arg| arg.as_ptr())
        .collect::<Vec<_>>();
    shell_raw.push(std::ptr::null());
    let shell_c = CString::new(shell_string_to_cstring_bytes(&shell))
        .unwrap_or_else(|_| CString::new("").unwrap());
    if fallback_globskipdots_off {
        std::env::set_var("__CHERUBSH_ENOEXEC_FALLBACK", "1");
    } else {
        std::env::remove_var("__CHERUBSH_ENOEXEC_FALLBACK");
    }
    unsafe {
        libc::execv(shell_c.as_ptr(), shell_raw.as_ptr());
    }
    126
}

fn is_text_script(path: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    if bytes.is_empty() {
        return true;
    }
    bytes
        .iter()
        .take(80)
        .all(|byte| matches!(*byte, b'\n' | b'\r' | b'\t' | 0x20..=0x7e))
}

fn shebang_interpreter(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let first_line = bytes.split(|byte| *byte == b'\n').next()?;
    let rest = first_line.strip_prefix(b"#!")?;
    let text = String::from_utf8_lossy(rest).trim().to_string();
    text.split_whitespace().next().map(ToOwned::to_owned)
}

pub(crate) fn search_path(name: &str, env: &dyn Environment) -> Option<PathBuf> {
    search_path_with(name, env, None)
}

pub(crate) fn search_path_with(
    name: &str,
    env: &dyn Environment,
    path_override: Option<&str>,
) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        if is_executable(&p) {
            return Some(p);
        }
        return None;
    }
    let path_storage;
    let path = match path_override {
        Some(path) => path,
        None => {
            path_storage = env.get("PATH").unwrap_or_default();
            &path_storage
        }
    };
    for dir in path.split(':') {
        let mut candidate = PathBuf::from(if dir.is_empty() { "." } else { dir });
        candidate.push(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &std::path::Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(cpath) = CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    unsafe { libc::access(cpath.as_ptr(), libc::X_OK) == 0 }
}
