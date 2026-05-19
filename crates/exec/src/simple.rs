use cherubsh_builtins::common::{array_reference, is_valid_name, split_assignment_op};
use cherubsh_builtins::{is_assignment_builtin, is_special, lookup, BuiltinCtx};
use cherubsh_common::{
    Environment, VarSnapshot, CMD_IGNORE_RETURN, CMD_INVERT_RETURN, W_ASSIGNMENT, W_COMPASSIGN,
};
use cherubsh_expander::assignment::ExpandedAssignment;
use cherubsh_expander::assignment::{expand_assignment_word, looks_like_assignment};
use cherubsh_expander::ExpandError;
use cherubsh_parser::{SimpleCommand, WordDesc};

use crate::function;
use crate::redirect::{self, ExecError};
use crate::runner::ExecRunner;
use crate::shell_ops::ExecAdapter;
use crate::util::{
    apply_assignment, execv_or_script, expand_one, expand_words, report_expand_error,
    report_expand_error_with_source_line, search_path, search_path_with, try_expand_words,
    wait_for_pid, wait_for_pid_ignoring_stops,
};
use crate::{ExecContext, ExecMode, Unwind};

pub(crate) fn execute<'a>(
    ctx: &mut ExecContext<'a>,
    simple: &SimpleCommand,
    flags: u32,
    mode: ExecMode,
) -> i32 {
    let debug_trap_in_scope = mode == ExecMode::Parent
        && !ctx.suppress_debug_traps
        && ((ctx.function_depth == 0 && ctx.source_depth == 0)
            || ctx.debug_trap_scopes.last().copied().unwrap_or(false));
    let err_trap_in_scope = mode == ExecMode::Parent
        && !ctx.suppress_err_traps
        && flags & CMD_IGNORE_RETURN == 0
        && flags & CMD_INVERT_RETURN == 0
        && ctx.errexit_suppressed == 0
        && (ctx.function_depth == 0 || ctx.env.option("errtrace") || ctx.env.option("extdebug"));

    if debug_trap_in_scope {
        if let Some(status) = crate::trap::run_debug_trap(ctx) {
            if status == 2 && ctx.env.option("extdebug") {
                return ctx.last_status;
            }
        }
    }
    let (raw_assignments, remaining) = split_assignments(&simple.words, ctx.env.option("keyword"));
    let trace_enabled = ctx.env.option("xtrace");
    let trace_assignments = if trace_enabled {
        raw_assignments
            .iter()
            .map(|word| word.text.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut assignment_expansion_status = 0;
    let mut scalar_readonly_assignment_error = false;
    let mut scalar_invalid_assignment_error = false;
    let mut assignments = Vec::new();
    let command_prefix_assignments = !remaining.is_empty();
    let mut assignment_expansion_snapshot = Vec::new();
    for word in raw_assignments {
        if command_prefix_assignments {
            if let Some((lhs, _, _)) = split_assignment_op(&word.text) {
                if array_reference(lhs).is_some() {
                    report_invalid_assignment_identifier(ctx.env, lhs);
                    assignment_expansion_status = 1;
                    continue;
                }
            }
        }
        let has_command_subst = word.text.contains("$(") || word.text.contains('`');
        let mut runner = ExecRunner::with_functions(&ctx.functions);
        match expand_assignment_word(&word, ctx.env, &mut runner) {
            Ok(Some(a)) => {
                if has_command_subst {
                    assignment_expansion_status = ctx.env.last_status();
                }
                if command_prefix_assignments {
                    remember_assignment_snapshot(ctx, &mut assignment_expansion_snapshot, &a);
                }
                let scalar_readonly = scalar_assignment_targets_readonly(ctx.env, &a, &word.text);
                let assignment_status = if command_prefix_assignments
                    && unresolved_nameref_prefix_assignment(ctx.env, &a)
                {
                    0
                } else {
                    apply_assignment(ctx.env, &a)
                };
                assignment_expansion_status |= assignment_status;
                if scalar_readonly && assignment_status != 0 {
                    scalar_readonly_assignment_error = true;
                }
                if scalar_assignment_invalid_nameref_target(ctx.env, &a) && assignment_status != 0 {
                    scalar_invalid_assignment_error = true;
                }
                assignments.push(a);
            }
            Ok(None) => {}
            Err(err) => {
                let status = expansion_error_status(&err);
                let unwind_status = expansion_error_unwind_status(&err, status, mode);
                let exits_shell = expansion_error_exits_shell(&err, ctx.env);
                if !err.already_reported() {
                    report_expand_error_with_source_line(
                        ctx.env,
                        err,
                        Some(word.span),
                        Some(&word.text),
                    );
                }
                assignment_expansion_status = status;
                if exits_shell {
                    ctx.pending = Some(Unwind::Exit(unwind_status));
                    return status;
                }
            }
        }
    }
    if command_prefix_assignments {
        restore_assignment_values(ctx, assignment_expansion_snapshot);
    }
    let scalar_assignments: Vec<(String, String)> = assignments
        .iter()
        .filter_map(ExpandedAssignment::scalar_pair)
        .collect();

    if remaining.is_empty() {
        if trace_enabled {
            trace_simple(ctx, &trace_assignments, &[]);
        }
        let _guard = if simple.redirects.is_empty() {
            None
        } else {
            match mode {
                ExecMode::Parent => {
                    match redirect::apply_redirects_to_parent(ctx, &simple.redirects) {
                        Ok(guard) => Some(guard),
                        Err(err) => {
                            err.report_with_env(ctx.env);
                            if err_trap_in_scope {
                                crate::trap::run_err_trap(ctx);
                            }
                            return 1;
                        }
                    }
                }
                ExecMode::Child => {
                    if let Err(err) = redirect::apply_redirects_to_child(ctx, &simple.redirects) {
                        err.report_with_env(ctx.env);
                        if err_trap_in_scope {
                            crate::trap::run_err_trap(ctx);
                        }
                        return 1;
                    }
                    None
                }
            }
        };
        let status = assignment_expansion_status;
        if status != 0 && (scalar_readonly_assignment_error || scalar_invalid_assignment_error) {
            handle_scalar_assignment_error(ctx, scalar_readonly_assignment_error);
        }
        if status != 0 && err_trap_in_scope {
            crate::trap::run_err_trap(ctx);
        }
        return status;
    }

    let command_expanded = match try_expand_words(&remaining[..1], ctx) {
        Ok(words) => words,
        Err(err) => {
            let status = expansion_error_status(&err);
            let unwind_status = expansion_error_unwind_status(&err, status, mode);
            let exits_shell = expansion_error_exits_shell(&err, ctx.env);
            report_expand_error(ctx.env, err, Some(remaining[0].span));
            if err_trap_in_scope {
                crate::trap::run_err_trap(ctx);
            }
            if exits_shell {
                ctx.pending = Some(Unwind::Exit(unwind_status));
            }
            return status;
        }
    };
    if command_expanded.is_empty() {
        if trace_enabled {
            trace_simple(ctx, &trace_assignments, &[]);
        }
        let _guard = if simple.redirects.is_empty() {
            None
        } else {
            match mode {
                ExecMode::Parent => {
                    match redirect::apply_redirects_to_parent(ctx, &simple.redirects) {
                        Ok(guard) => Some(guard),
                        Err(err) => {
                            err.report_with_env(ctx.env);
                            if err_trap_in_scope {
                                crate::trap::run_err_trap(ctx);
                            }
                            return 1;
                        }
                    }
                }
                ExecMode::Child => {
                    if let Err(err) = redirect::apply_redirects_to_child(ctx, &simple.redirects) {
                        err.report_with_env(ctx.env);
                        if err_trap_in_scope {
                            crate::trap::run_err_trap(ctx);
                        }
                        return 1;
                    }
                    None
                }
            }
        };
        let mut status = assignment_expansion_status;
        if status == 0 {
            for assignment in &assignments {
                status |= apply_assignment(ctx.env, assignment);
            }
        }
        if status != 0 && (scalar_readonly_assignment_error || scalar_invalid_assignment_error) {
            handle_scalar_assignment_error(ctx, scalar_readonly_assignment_error);
        }
        if status != 0 && err_trap_in_scope {
            crate::trap::run_err_trap(ctx);
        }
        return status;
    }
    let mut command_name = command_expanded[0].clone();
    let command_word_extra_args = command_expanded[1..].to_vec();
    let command_is_function = ctx.functions.contains_key(&command_name);
    let assignment_builtin = !command_is_function
        && lookup(&command_name)
            .map(|b| ctx.env.builtin_enabled(b.name()) && is_assignment_builtin(&command_name))
            .unwrap_or(false);
    let (mut args, mut arg_flags) = if assignment_builtin {
        let (mut args, mut arg_flags) = expand_assignment_builtin_args(&remaining[1..], ctx);
        if !command_word_extra_args.is_empty() {
            let mut combined = command_word_extra_args.clone();
            combined.extend(args);
            args = combined;
            let mut combined_flags = vec![0; command_word_extra_args.len()];
            combined_flags.extend(arg_flags);
            arg_flags = combined_flags;
        }
        (args, arg_flags)
    } else {
        let (mut args, mut arg_flags) = match expand_regular_args_with_flags(&remaining[1..], ctx) {
            Ok(words) => words,
            Err(err) => {
                let status = expansion_error_status(&err);
                let unwind_status = expansion_error_unwind_status(&err, status, mode);
                let exits_shell = expansion_error_exits_shell(&err, ctx.env);
                report_expand_error(ctx.env, err, None);
                if err_trap_in_scope {
                    crate::trap::run_err_trap(ctx);
                }
                if exits_shell {
                    ctx.pending = Some(Unwind::Exit(unwind_status));
                }
                return status;
            }
        };
        if !command_word_extra_args.is_empty() {
            let mut combined = command_word_extra_args.clone();
            combined.extend(args);
            args = combined;
            let mut combined_flags = vec![0; command_word_extra_args.len()];
            combined_flags.extend(arg_flags);
            arg_flags = combined_flags;
        }
        (args, arg_flags)
    };
    if trace_enabled {
        trace_simple(
            ctx,
            &trace_assignments,
            &command_words(&command_name, &args),
        );
    }

    if ctx.env.option("restricted") && command_name.contains('/') {
        cherubsh_builtins::common::report_diagnostic(
            ctx.env,
            &command_name,
            "restricted: cannot specify `/' in command names",
        );
        return 126;
    }

    let mut suppress_function_lookup = false;
    let mut path_override = None;
    while command_name == "command" {
        if let Some(parsed) = peel_command_execution(&args, &arg_flags) {
            if !(parsed.use_default_path && ctx.env.option("restricted")) {
                command_name = parsed.name;
                args = parsed.args;
                arg_flags = parsed.arg_flags;
                suppress_function_lookup = true;
                if parsed.use_default_path {
                    path_override = Some(cherubsh_builtins::command_cmd::DEFAULT_PATH);
                }
                continue;
            }
        }
        break;
    }
    if ctx.env.option("restricted") && command_name.contains('/') {
        cherubsh_builtins::common::report_diagnostic(
            ctx.env,
            &command_name,
            "restricted: cannot specify `/' in command names",
        );
        return 126;
    }

    if command_prefix_assignments
        && assignment_expansion_status != 0
        && ctx.env.option("posix")
        && (scalar_readonly_assignment_error || scalar_invalid_assignment_error)
    {
        let special = lookup(&command_name)
            .map(|b| ctx.env.builtin_enabled(b.name()) && is_special(&command_name))
            .unwrap_or(false);
        if special && !suppress_function_lookup {
            ctx.pending = Some(Unwind::Exit(1));
        }
        return 1;
    }

    if !suppress_function_lookup {
        if command_name.starts_with('%') && ctx.env.job_control_enabled() {
            let fg_args = std::iter::once(command_name.clone())
                .chain(args.clone())
                .collect::<Vec<_>>();
            let status = run_builtin(
                ctx,
                "fg",
                &fg_args,
                &vec![0; fg_args.len()],
                &scalar_assignments,
                simple,
                mode,
                suppress_function_lookup,
            );
            if status != 0 && err_trap_in_scope {
                crate::trap::run_err_trap(ctx);
            }
            return status;
        }
        if let Some(function_body) = ctx.functions.get(&command_name).cloned() {
            let status = function::call(
                ctx,
                &command_name,
                &function_body,
                args,
                &simple.redirects,
                &scalar_assignments,
                ExecMode::Parent,
            );
            if status != 0 && err_trap_in_scope {
                crate::trap::run_err_trap(ctx);
            }
            return status;
        }
    }

    if command_name == "exec" && args.is_empty() {
        // Persistent redirect application - exec without a command leaves
        // its redirects in place for the rest of the shell.
        if let Err(err) = crate::redirect::apply_redirects_to_child(ctx, &simple.redirects) {
            err.report_with_env(ctx.env);
            return 1;
        }
        return 0;
    }

    if let Some(b) = lookup(&command_name) {
        if ctx.env.builtin_enabled(b.name()) {
            let status = run_builtin(
                ctx,
                &command_name,
                &args,
                &arg_flags,
                &scalar_assignments,
                simple,
                mode,
                suppress_function_lookup,
            );
            if status != 0 && err_trap_in_scope {
                crate::trap::run_err_trap(ctx);
            }
            return status;
        }
    }

    let status = match mode {
        ExecMode::Parent => {
            match execute_external_parent(
                ctx,
                &command_name,
                &args,
                &scalar_assignments,
                simple,
                path_override,
            ) {
                Ok(status) => status,
                Err(err) => {
                    err.report_with_env(ctx.env);
                    127
                }
            }
        }
        ExecMode::Child => {
            if !ctx.allow_child_external_exec {
                execute_external_child_forked(
                    ctx,
                    &command_name,
                    &args,
                    &scalar_assignments,
                    simple,
                    path_override,
                )
            } else if simple.redirects.is_empty() {
                crate::util::reset_child_signal_handlers(ctx.env);
                execute_external_child(
                    ctx,
                    &command_name,
                    &args,
                    &scalar_assignments,
                    simple,
                    path_override,
                )
            } else {
                match redirect::apply_redirects_to_child(ctx, &simple.redirects) {
                    Ok(()) => {
                        crate::util::reset_child_signal_handlers(ctx.env);
                        execute_external_child(
                            ctx,
                            &command_name,
                            &args,
                            &scalar_assignments,
                            simple,
                            path_override,
                        )
                    }
                    Err(err) => {
                        err.report_with_env(ctx.env);
                        1
                    }
                }
            }
        }
    };
    if status != 0 && err_trap_in_scope {
        crate::trap::run_err_trap(ctx);
    }
    status
}

fn execute_external_child_forked<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    args: &[String],
    assignments: &[(String, String)],
    simple: &SimpleCommand,
    path_override: Option<&'static str>,
) -> i32 {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return 1;
    }
    if pid == 0 {
        crate::util::reset_child_signal_handlers(ctx.env);
        if let Err(err) = redirect::apply_redirects_to_child(ctx, &simple.redirects) {
            err.report_with_env(ctx.env);
            unsafe { libc::_exit(1) };
        }
        let code = execute_external_child(ctx, name, args, assignments, simple, path_override);
        unsafe { libc::_exit(code) };
    }
    wait_for_pid_ignoring_stops(pid)
}

fn handle_scalar_assignment_error(ctx: &mut ExecContext<'_>, readonly: bool) {
    ctx.pending = Some(if readonly && ctx.env.option("posix") {
        Unwind::Exit(1)
    } else {
        Unwind::AbortLine(1)
    });
}

struct CommandExecution {
    name: String,
    args: Vec<String>,
    arg_flags: Vec<u32>,
    use_default_path: bool,
}

fn peel_command_execution(args: &[String], arg_flags: &[u32]) -> Option<CommandExecution> {
    let mut use_default_path = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        for ch in arg[1..].chars() {
            match ch {
                'p' => use_default_path = true,
                'v' | 'V' => return None,
                _ => return None,
            }
        }
        i += 1;
    }
    if i >= args.len() {
        return None;
    }
    Some(CommandExecution {
        name: args[i].clone(),
        args: args[i + 1..].to_vec(),
        arg_flags: arg_flags
            .get(i + 1..)
            .map_or_else(Vec::new, ToOwned::to_owned),
        use_default_path,
    })
}

fn command_words(name: &str, args: &[String]) -> Vec<String> {
    std::iter::once(name.to_string())
        .chain(args.iter().cloned())
        .collect()
}

fn trace_simple(ctx: &mut ExecContext<'_>, assignments: &[String], words: &[String]) {
    let line = assignments
        .iter()
        .chain(words.iter())
        .map(|word| cherubsh_builtins::common::diagnostic_subject(word))
        .collect::<Vec<_>>()
        .join(" ");
    if !line.is_empty() {
        crate::xtrace::trace(ctx, &line);
    }
}

fn remember_assignment_snapshot(
    ctx: &ExecContext<'_>,
    snapshot: &mut Vec<(String, Option<String>)>,
    assignment: &ExpandedAssignment,
) {
    let name = assignment_storage_name(ctx.env, assignment_name(assignment));
    if snapshot.iter().any(|(key, _)| key == &name) {
        return;
    }
    snapshot.push((name.clone(), ctx.env.get(&name)));
}

fn assignment_name(assignment: &ExpandedAssignment) -> &str {
    match assignment {
        ExpandedAssignment::Scalar { name, .. }
        | ExpandedAssignment::IndexedElem { name, .. }
        | ExpandedAssignment::AssocElem { name, .. }
        | ExpandedAssignment::IndexedArray { name, .. }
        | ExpandedAssignment::AssocArray { name, .. } => name,
    }
}

fn assignment_storage_name(env: &dyn cherubsh_common::Environment, name: &str) -> String {
    env.resolve_nameref(name)
        .filter(|target| !target.is_empty())
        .unwrap_or_else(|| name.to_string())
}

fn scalar_assignment_targets_readonly(
    env: &dyn cherubsh_common::Environment,
    assignment: &ExpandedAssignment,
    raw: &str,
) -> bool {
    !env.option("restricted")
        && !raw.contains("+=")
        && matches!(assignment, ExpandedAssignment::Scalar { name, .. } if env.is_readonly(name))
}

fn scalar_assignment_invalid_nameref_target(
    env: &dyn cherubsh_common::Environment,
    assignment: &ExpandedAssignment,
) -> bool {
    let ExpandedAssignment::Scalar { name, value } = assignment else {
        return false;
    };
    env.attrs(name).contains(cherubsh_common::VarAttrs::NAMEREF)
        && env
            .nameref_target(name)
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        && !is_valid_name(value)
        && array_reference(value).is_none()
}

fn unresolved_nameref_prefix_assignment(
    env: &dyn cherubsh_common::Environment,
    assignment: &ExpandedAssignment,
) -> bool {
    let ExpandedAssignment::Scalar { name, value } = assignment else {
        return false;
    };
    env.attrs(name).contains(cherubsh_common::VarAttrs::NAMEREF)
        && env
            .nameref_target(name)
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        && !is_valid_name(value)
        && array_reference(value).is_none()
}

fn expand_assignment_builtin_args(
    words: &[WordDesc],
    ctx: &mut ExecContext<'_>,
) -> (Vec<String>, Vec<u32>) {
    let mut out = Vec::new();
    let mut flags = Vec::new();
    for word in words {
        if looks_like_assignment(word) {
            let mut arg_flags = word.flags;
            if word.text.contains("=(") || word.text.contains("+=(") {
                arg_flags |= W_ASSIGNMENT | W_COMPASSIGN;
            }
            if arg_flags & W_COMPASSIGN != 0 {
                out.push(word.text.clone());
            } else {
                out.push(expand_one(word, ctx));
            }
            flags.push(arg_flags);
        } else {
            let expanded = expand_words(std::slice::from_ref(word), ctx);
            flags.extend(std::iter::repeat(0).take(expanded.len()));
            out.extend(expanded);
        }
    }
    (out, flags)
}

fn expand_regular_args_with_flags(
    words: &[WordDesc],
    ctx: &mut ExecContext<'_>,
) -> Result<(Vec<String>, Vec<u32>), ExpandError> {
    let mut out = Vec::new();
    let mut flags = Vec::new();
    for word in words {
        let expanded = try_expand_words(std::slice::from_ref(word), ctx)?;
        flags.extend(std::iter::repeat(word.flags).take(expanded.len()));
        out.extend(expanded);
    }
    Ok((out, flags))
}

fn run_builtin<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    args: &[String],
    arg_flags: &[u32],
    assignments: &[(String, String)],
    simple: &SimpleCommand,
    mode: ExecMode,
    invoked_via_command: bool,
) -> i32 {
    // POSIX requires assignments preceding special builtins to persist.
    // Non-POSIX bash makes them temporary just like for regular builtins.
    let posix = ctx.env.option("posix") || ctx.env.get("POSIXLY_CORRECT").is_some();
    let special = is_special(name);
    let posix_special_persist = special && posix && !invoked_via_command;
    let persist = posix_special_persist || matches!(name, "export" | "readonly");
    let localvar_builtin = matches!(name, "declare" | "typeset" | "local");
    let preserve_declared_attrs =
        matches!(name, "declare" | "typeset") && declare_sets_persistent_attrs(args);
    let saved = if !persist {
        snapshot_assignments(ctx, assignments)
    } else {
        Vec::new()
    };

    let status = match mode {
        ExecMode::Parent => {
            if simple.redirects.is_empty() {
                apply_builtin_assignments(ctx, name, assignments, posix_special_persist);
                dispatch(
                    ctx,
                    name,
                    args,
                    arg_flags,
                    assignments,
                    &simple.redirects,
                    invoked_via_command,
                )
            } else {
                match redirect::apply_redirects_to_parent(ctx, &simple.redirects) {
                    Ok(_guard) => {
                        apply_builtin_assignments(ctx, name, assignments, posix_special_persist);
                        dispatch(
                            ctx,
                            name,
                            args,
                            arg_flags,
                            assignments,
                            &simple.redirects,
                            invoked_via_command,
                        )
                    }
                    Err(err) => {
                        err.report_with_env(ctx.env);
                        if posix_special_persist {
                            ctx.pending = Some(Unwind::Exit(1));
                        }
                        1
                    }
                }
            }
        }
        ExecMode::Child => {
            if simple.redirects.is_empty() {
                apply_builtin_assignments(ctx, name, assignments, posix_special_persist);
                dispatch(
                    ctx,
                    name,
                    args,
                    arg_flags,
                    assignments,
                    &simple.redirects,
                    invoked_via_command,
                )
            } else {
                match redirect::apply_redirects_to_parent(ctx, &simple.redirects) {
                    Ok(_guard) => {
                        apply_builtin_assignments(ctx, name, assignments, posix_special_persist);
                        dispatch(
                            ctx,
                            name,
                            args,
                            arg_flags,
                            assignments,
                            &simple.redirects,
                            invoked_via_command,
                        )
                    }
                    Err(err) => {
                        err.report_with_env(ctx.env);
                        1
                    }
                }
            }
        }
    };

    if !persist && (preserve_declared_attrs || localvar_builtin) {
        restore_assignments_except_declared_attrs_or_locals(ctx, saved, preserve_declared_attrs);
    } else if !persist {
        restore_assignments(ctx, saved);
    }
    if posix_special_persist && special_assignment_bypasses_function_restore(name) {
        for (key, _) in assignments {
            ctx.mark_posix_special_assignment_persisted(key);
        }
    }
    status
}

fn special_assignment_bypasses_function_restore(name: &str) -> bool {
    !matches!(name, "." | "eval" | "source" | "unset")
}

fn apply_builtin_assignments(
    ctx: &mut ExecContext<'_>,
    name: &str,
    assignments: &[(String, String)],
    posix_special_persist: bool,
) {
    let assignment_builtin = is_assignment_builtin(name);
    for (k, v) in assignments {
        ctx.env.set(k, v.clone());
        if assignment_builtin || posix_special_persist {
            let target = assignment_storage_name(ctx.env, k);
            ctx.env
                .set_attr(&target, cherubsh_common::VarAttrs::EXPORT, true);
        }
    }
}

fn declare_sets_persistent_attrs(args: &[String]) -> bool {
    for arg in args {
        if arg == "--" {
            break;
        }
        let Some(body) = arg.strip_prefix('-') else {
            break;
        };
        if body.is_empty() {
            break;
        }
        if !body.chars().all(|c| {
            matches!(
                c,
                'a' | 'A'
                    | 'f'
                    | 'F'
                    | 'g'
                    | 'i'
                    | 'I'
                    | 'l'
                    | 'n'
                    | 'p'
                    | 'r'
                    | 't'
                    | 'u'
                    | 'x'
                    | 'c'
            )
        }) {
            break;
        }
        if body.contains('x') || body.contains('r') {
            return true;
        }
    }
    false
}

fn dispatch<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    args: &[String],
    arg_flags: &[u32],
    assignments: &[(String, String)],
    redirects: &[cherubsh_parser::Redirect],
    invoked_via_command: bool,
) -> i32 {
    let Some(b) = lookup(name) else {
        return 1;
    };
    let mut adapter = ExecAdapter { ctx };
    let mut bctx = BuiltinCtx {
        args,
        arg_flags,
        assignments,
        redirects,
        invoked_via_command,
        shell: &mut adapter,
    };
    b.run(&mut bctx)
}

fn snapshot_assignments(
    ctx: &ExecContext,
    assignments: &[(String, String)],
) -> Vec<AssignmentSnapshot> {
    if assignments.is_empty() {
        return Vec::new();
    }
    assignments
        .iter()
        .map(|(k, _)| {
            let key = assignment_storage_name(ctx.env, k);
            let value = ctx.env.get(&key);
            let snapshot = ctx.env.var_snapshot(&key);
            AssignmentSnapshot {
                key,
                value,
                snapshot,
            }
        })
        .collect()
}

#[derive(Clone)]
struct AssignmentSnapshot {
    key: String,
    value: Option<String>,
    snapshot: Option<VarSnapshot>,
}

fn restore_assignments(ctx: &mut ExecContext, snapshot: Vec<AssignmentSnapshot>) {
    for saved in snapshot {
        match saved.value {
            Some(v) => ctx.env.set(&saved.key, v),
            None => ctx.env.unset(&saved.key),
        }
    }
}

fn restore_assignment_values(ctx: &mut ExecContext, snapshot: Vec<(String, Option<String>)>) {
    for (key, prior) in snapshot {
        match prior {
            Some(v) => ctx.env.set(&key, v),
            None => ctx.env.unset(&key),
        }
    }
}

fn restore_assignments_except_declared_attrs_or_locals(
    ctx: &mut ExecContext,
    snapshot: Vec<AssignmentSnapshot>,
    preserve_declared_attrs: bool,
) {
    if snapshot.is_empty() {
        return;
    }
    let local_names = ctx
        .env
        .iter_local_vars()
        .into_iter()
        .map(|snap| snap.name)
        .collect::<std::collections::HashSet<_>>();
    for saved in snapshot {
        let attrs = ctx.env.attrs(&saved.key);
        if local_names.contains(&saved.key) {
            ctx.env
                .set_local_restore_snapshot(&saved.key, saved.snapshot);
            continue;
        }
        if preserve_declared_attrs
            && attrs
                .intersects(cherubsh_common::VarAttrs::EXPORT | cherubsh_common::VarAttrs::READONLY)
        {
            continue;
        }
        match saved.value {
            Some(v) => ctx.env.set(&saved.key, v),
            None => ctx.env.unset(&saved.key),
        }
    }
}

fn report_invalid_assignment_identifier(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: `{name}': not a valid identifier");
    } else {
        eprintln!("cherubsh: `{name}': not a valid identifier");
    }
}

fn execute_external_parent<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    args: &[String],
    assignments: &[(String, String)],
    simple: &SimpleCommand,
    path_override: Option<&'static str>,
) -> Result<i32, ExecError> {
    let job_control = ctx.env.job_control_enabled();
    let tty_fd = ctx.env.tty_fd();
    let shell_pgrp = ctx.env.shell_pgrp();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(ExecError::new("fork() failed"));
    }
    if pid == 0 {
        // Child: install fresh process group leadership, reset signals.
        unsafe {
            libc::setpgid(0, 0);
        }
        crate::util::reset_child_signal_handlers(ctx.env);
        if let Err(err) = redirect::apply_redirects_to_child(ctx, &simple.redirects) {
            err.report_with_env(ctx.env);
            unsafe { libc::_exit(1) };
        }
        let code = execute_external_child(ctx, name, args, assignments, simple, path_override);
        unsafe { libc::_exit(code) };
    }
    // Parent: close pgid race + transfer terminal to child group.
    unsafe {
        libc::setpgid(pid, pid);
    }
    if job_control {
        if let Some(fd) = tty_fd {
            crate::util::tcsetpgrp_blocked(fd, pid);
        }
    }
    let status = wait_for_pid(pid);
    if job_control {
        if let Some(fd) = tty_fd {
            crate::util::tcsetpgrp_blocked(fd, shell_pgrp);
        }
    }
    Ok(status)
}

fn execute_external_child<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    args: &[String],
    assignments: &[(String, String)],
    _simple: &SimpleCommand,
    path_override: Option<&'static str>,
) -> i32 {
    for (key, value) in assignments {
        let target = assignment_storage_name(ctx.env, key);
        std::env::set_var(target, value);
    }
    let assignment_path = assignments.iter().rev().find_map(|(key, value)| {
        (assignment_storage_name(ctx.env, key) == "PATH").then_some(value.as_str())
    });
    let effective_path = path_override.or(assignment_path);
    let empty_effective_path = matches!(effective_path, Some(""));
    let bypass_hash = effective_path.is_some();
    let egrep_compat = name == "egrep";
    let lookup_name = if egrep_compat { "grep" } else { name };
    let argv0_name = if egrep_compat { "grep" } else { name };

    if name.contains('/') && std::path::Path::new(name).is_dir() {
        cherubsh_builtins::common::report_diagnostic(ctx.env, name, "Is a directory");
        return 126;
    }

    // Resolve via cache or PATH search.
    let resolved = if bypass_hash {
        search_path_with(lookup_name, ctx.env, effective_path)
    } else {
        match ctx.env.hash_get_with_hit(lookup_name) {
            Some(path) => Some(path),
            None => match search_path(lookup_name, ctx.env) {
                Some(path) => {
                    ctx.env.hash_set(lookup_name, path.clone());
                    Some(path)
                }
                None => None,
            },
        }
    };
    let argv0 = match resolved.as_ref() {
        Some(_) => argv0_name.to_string(),
        None => {
            let empty_path = empty_effective_path
                || (effective_path.is_none() && ctx.env.get("PATH").as_deref() == Some(""));
            let message = if empty_path {
                "No such file or directory"
            } else {
                "command not found"
            };
            cherubsh_builtins::common::report_diagnostic(ctx.env, name, message);
            return 127;
        }
    };
    let mut argv = Vec::with_capacity(args.len() + if egrep_compat { 2 } else { 1 });
    argv.push(argv0);
    if egrep_compat {
        argv.push("-E".to_string());
    }
    argv.extend_from_slice(args);
    let exec_path = resolved
        .as_ref()
        .map(|path| path.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let status = execv_or_script(&exec_path, &argv, ctx.env.option("globskipdots"));
    eprintln!("cherubsh: {name}: exec failed");
    status
}

fn expansion_error_exits_shell(err: &ExpandError, env: &dyn Environment) -> bool {
    matches!(err, ExpandError::AssignToReadonly(_)) && env.arithmetic_expansion_errors_exit_shell()
        || (!matches!(err, ExpandError::AssignToReadonly(_)) && err.is_fatal())
        || matches!(err, ExpandError::UnboundVariable(_))
        || (env.option("posix")
            && matches!(err, ExpandError::BadSubstitution(s) if !s.starts_with("${-")))
        || (env.arithmetic_expansion_errors_exit_shell()
            && matches!(
                err,
                ExpandError::DivisionByZero
                    | ExpandError::ArithSyntax(_)
                    | ExpandError::ArithOverflow
                    | ExpandError::ArithRecursion
            ))
}

fn expansion_error_status(err: &ExpandError) -> i32 {
    match err {
        ExpandError::UnboundVariable(_) | ExpandError::UnboundColonError(_, _) => 127,
        ExpandError::AlreadyReported(status) | ExpandError::CommandSubstFailed(status) => *status,
        _ => 1,
    }
}

fn expansion_error_unwind_status(err: &ExpandError, status: i32, mode: ExecMode) -> i32 {
    if mode == ExecMode::Child
        && matches!(
            err,
            ExpandError::UnboundVariable(_) | ExpandError::UnboundColonError(_, _)
        )
    {
        1
    } else {
        status
    }
}

pub(crate) fn split_assignments(
    words: &[WordDesc],
    keyword: bool,
) -> (Vec<WordDesc>, Vec<WordDesc>) {
    let mut assignments = Vec::new();
    if keyword {
        let mut remaining = Vec::new();
        for word in words {
            if looks_like_assignment(word) {
                assignments.push(word.clone());
            } else {
                remaining.push(word.clone());
            }
        }
        return (assignments, remaining);
    }

    let mut index = 0;
    for word in words {
        if looks_like_assignment(word) {
            assignments.push(word.clone());
            index += 1;
        } else {
            break;
        }
    }
    (assignments, words[index..].to_vec())
}
