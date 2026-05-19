use cherubsh_parser::{Command, FunctionDef, Redirect};
use std::sync::Arc;

use crate::redirect::{self};
use crate::{ExecContext, ExecMode, Unwind};

struct PrefixAssignmentSnapshot {
    key: String,
    value: Option<String>,
    exported: bool,
}

pub(crate) fn define<'a>(ctx: &mut ExecContext<'a>, def: &FunctionDef) -> i32 {
    if ctx.env.option("posix") && !cherubsh_builtins::common::is_valid_name(&def.name.text) {
        cherubsh_builtins::common::report_diagnostic(
            ctx.env,
            &format!("`{}'", def.name.text),
            "not a valid identifier",
        );
        ctx.pending = Some(Unwind::Exit(2));
        return 2;
    }

    if ctx.env.function_is_readonly(&def.name.text) {
        if let Some(current_line) = ctx.env.diagnostic_line() {
            ctx.env
                .push_diagnostic_line(current_line.saturating_add(def.line));
            cherubsh_builtins::common::report_diagnostic(
                ctx.env,
                &def.name.text,
                "readonly function",
            );
            ctx.env.pop_diagnostic_line();
        } else {
            cherubsh_builtins::common::report_diagnostic(
                ctx.env,
                &def.name.text,
                "readonly function",
            );
        }
        return 1;
    }
    let body = if def.command.line == 0 {
        let mut body = def.command.as_ref().clone();
        body.line = match (def.line, ctx.env.diagnostic_line()) {
            (offset_from_end, Some(current_line)) if offset_from_end > 0 => current_line
                .saturating_sub(offset_from_end.saturating_sub(2))
                .max(1),
            _ => def.line.max(ctx.env.diagnostic_line().unwrap_or(0)),
        };
        Arc::new(body)
    } else {
        Arc::clone(&def.command)
    };
    ctx.functions.insert(def.name.text.clone(), body);
    0
}

pub(crate) fn call<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    body: &Command,
    args: Vec<String>,
    redirects: &[Redirect],
    assignments: &[(String, String)],
    mode: ExecMode,
) -> i32 {
    if ctx.funcnest_max > 0 && ctx.function_depth >= ctx.funcnest_max {
        cherubsh_builtins::common::report_diagnostic(
            ctx.env,
            name,
            &format!(
                "maximum function nesting level exceeded ({})",
                ctx.funcnest_max
            ),
        );
        return 1;
    }

    let function_call_id = ctx.next_function_call_id;
    ctx.next_function_call_id += 1;
    ctx.function_call_stack.push(function_call_id);

    let saved_positionals = ctx.env.push_function_positionals(&args);

    let mut assignment_snapshot = Vec::new();
    let mut prefix_names = Vec::new();
    if !assignments.is_empty() {
        assignment_snapshot.reserve(assignments.len());
        prefix_names.reserve(assignments.len());
        for (k, _) in assignments {
            let key = assignment_storage_name(ctx, k);
            prefix_names.push(key.clone());
            assignment_snapshot.push(PrefixAssignmentSnapshot {
                value: ctx.env.get(&key),
                exported: ctx.env.exported(&key),
                key,
            });
        }
    }

    ctx.env.funcname_push(name, &args);
    ctx.function_depth += 1;
    ctx.env.push_local_scope();
    ctx.function_prefix_assignment_stack.push(prefix_names);
    if body.line > 0 {
        ctx.env.push_diagnostic_line(body.line);
    }
    let debug_scope = ctx.env.option("functrace") || ctx.function_traced.contains(name);
    ctx.debug_trap_scopes.push(debug_scope);
    if mode == ExecMode::Parent && debug_scope {
        crate::trap::run_debug_trap(ctx);
    }

    let status = ctx.with_abort_line_boundary(|ctx| match mode {
        ExecMode::Parent => {
            if redirects.is_empty() {
                apply_assignments(ctx, assignments);
                ctx.execute_command(&body, ExecMode::Parent)
            } else {
                match redirect::apply_redirects_to_parent(ctx, redirects) {
                    Ok(_guard) => {
                        apply_assignments(ctx, assignments);
                        ctx.execute_command(&body, ExecMode::Parent)
                    }
                    Err(err) => {
                        err.report_with_env(ctx.env);
                        1
                    }
                }
            }
        }
        ExecMode::Child => {
            if let Err(err) = redirect::apply_redirects_to_child(ctx, redirects) {
                err.report_with_env(ctx.env);
                1
            } else {
                apply_assignments(ctx, assignments);
                ctx.execute_command(&body, ExecMode::Child)
            }
        }
    });
    let status = match ctx.pending.take() {
        Some(Unwind::Return(n)) => n,
        other => {
            ctx.pending = other;
            status
        }
    };

    if mode == ExecMode::Parent && debug_scope {
        crate::trap::run_return_trap(ctx);
    }
    if mode == ExecMode::Parent && matches!(ctx.pending, Some(Unwind::Exit(_))) {
        if let Some(status) = crate::trap::run_exit_trap(ctx) {
            ctx.pending = Some(Unwind::Exit(status));
        }
    }
    ctx.debug_trap_scopes.pop();
    if body.line > 0 {
        ctx.env.pop_diagnostic_line();
    }

    ctx.env.pop_local_scope();
    ctx.function_prefix_assignment_stack.pop();

    ctx.function_depth -= 1;
    ctx.env.funcname_pop();

    // restore command-prefix assignments (functions are not special builtins)
    for snapshot in assignment_snapshot {
        if ctx.posix_special_assignment_affects_call(&snapshot.key, function_call_id) {
            continue;
        }
        if ctx
            .explicit_function_exports
            .iter()
            .any(|(name, call_id)| name == &snapshot.key && *call_id == function_call_id)
        {
            continue;
        }
        match snapshot.value {
            Some(v) => {
                ctx.env.set(&snapshot.key, v);
                if snapshot.exported {
                    ctx.env.export(&snapshot.key);
                } else {
                    ctx.env
                        .set_attr(&snapshot.key, cherubsh_common::VarAttrs::EXPORT, false);
                    std::env::remove_var(&snapshot.key);
                }
            }
            None => ctx.env.unset(&snapshot.key),
        }
    }
    if !ctx.posix_special_assignment_persisted.is_empty() {
        ctx.clear_posix_special_assignments_for_call(function_call_id);
    }
    if !ctx.explicit_function_exports.is_empty() {
        ctx.explicit_function_exports
            .retain(|(_, call_id)| *call_id != function_call_id);
    }
    let popped_call_id = ctx.function_call_stack.pop();
    debug_assert_eq!(popped_call_id, Some(function_call_id));

    ctx.env.pop_function_positionals(saved_positionals);
    status
}

fn apply_assignments(ctx: &mut ExecContext<'_>, assignments: &[(String, String)]) {
    for (k, v) in assignments {
        ctx.env.set(k, v.clone());
        ctx.env.export(k);
    }
}

fn assignment_storage_name(ctx: &ExecContext<'_>, name: &str) -> String {
    ctx.env
        .resolve_nameref(name)
        .filter(|target| !target.is_empty())
        .unwrap_or_else(|| name.to_string())
}
