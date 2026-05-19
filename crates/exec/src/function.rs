use std::collections::HashMap;

use cherubsh_parser::{Command, FunctionDef, Redirect};

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
    let mut body = (*def.command).clone();
    if body.line == 0 {
        body.line = match (def.line, ctx.env.diagnostic_line()) {
            (offset_from_end, Some(current_line)) if offset_from_end > 0 => current_line
                .saturating_sub(offset_from_end.saturating_sub(2))
                .max(1),
            _ => def.line.max(ctx.env.diagnostic_line().unwrap_or(0)),
        };
    }
    ctx.functions.insert(def.name.text.clone(), body);
    0
}

pub(crate) fn call<'a>(
    ctx: &mut ExecContext<'a>,
    name: &str,
    body: Command,
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

    let saved_positionals = ctx.env.positionals_clone();
    let mut new_positionals = Vec::with_capacity(args.len() + 1);
    new_positionals.push(
        saved_positionals
            .first()
            .cloned()
            .unwrap_or_else(|| "cherubsh".to_string()),
    );
    new_positionals.extend(args.iter().cloned());
    ctx.env.set_positionals(new_positionals);

    let assignment_snapshot: Vec<PrefixAssignmentSnapshot> = assignments
        .iter()
        .map(|(k, _)| {
            let key = assignment_storage_name(ctx, k);
            PrefixAssignmentSnapshot {
                value: ctx.env.get(&key),
                exported: ctx.env.exported(&key),
                key,
            }
        })
        .collect();
    let prefix_names = assignment_snapshot
        .iter()
        .map(|snapshot| snapshot.key.clone())
        .collect::<std::collections::HashSet<_>>();

    ctx.env.funcname_push(name, &args);
    ctx.function_depth += 1;
    ctx.env.push_local_scope();
    ctx.local_stack.push(HashMap::new());
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

    // pop local frame
    if let Some(frame) = ctx.local_stack.pop() {
        for (name, prior) in frame {
            match prior {
                Some(v) => ctx.env.set(&name, v),
                None => ctx.env.unset(&name),
            }
        }
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
            .contains(&(snapshot.key.clone(), function_call_id))
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
    ctx.clear_posix_special_assignments_for_call(function_call_id);
    ctx.explicit_function_exports
        .retain(|(_, call_id)| *call_id != function_call_id);
    let popped_call_id = ctx.function_call_stack.pop();
    debug_assert_eq!(popped_call_id, Some(function_call_id));

    let mut restored_positionals = saved_positionals;
    if let Some(current_zero) = ctx.env.positional(0) {
        if restored_positionals.is_empty() {
            restored_positionals.push(current_zero);
        } else {
            restored_positionals[0] = current_zero;
        }
    }
    ctx.env.set_positionals(restored_positionals);
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
