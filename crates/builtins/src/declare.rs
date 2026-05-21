use crate::common::{
    apply_assignment_arg_global, array_reference, assign_direct_target_op,
    assign_direct_target_op_global, assign_value, assign_value_global, assignment_base_name,
    format_declare_listing_var, format_var, is_valid_name, report_assign_error,
    report_builtin_assign_error, report_builtin_readonly_error, report_diagnostic,
    split_assignment_op,
};
use crate::type_cmd::{function_export_value, print_function_definition};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};
use cherubsh_common::{AssignError, Environment, VarAttrs, VarKind, VarSnapshot, W_COMPASSIGN};

pub struct Declare;
pub static DECLARE: Declare = Declare;

pub struct Typeset;
pub static TYPESET: Typeset = Typeset;

impl Builtin for Declare {
    fn name(&self) -> &'static str {
        "declare"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::ASSIGNMENT | BuiltinFlags::LOCALVAR
    }
    fn synopsis(&self) -> &'static str {
        "declare [-aAfFgiIlnrtux] [-p] [name[=value] ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_declare(ctx, true, "declare")
    }
}

impl Builtin for Typeset {
    fn name(&self) -> &'static str {
        "typeset"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::ASSIGNMENT | BuiltinFlags::LOCALVAR
    }
    fn synopsis(&self) -> &'static str {
        "typeset [-aAfFgiIlnrtux] [-p] [name[=value] ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_declare(ctx, true, "typeset")
    }
}

#[derive(Default, Clone, Copy)]
struct Flags {
    set: VarAttrs,
    clear: VarAttrs,
    print: bool,
    function: bool,
    function_names_only: bool,
    global: bool,
    inherit: bool,
}

fn run_declare(ctx: &mut BuiltinCtx<'_>, force_local: bool, diagnostic_name: &str) -> i32 {
    let mut f = Flags::default();
    let mut idx = 0;
    let mut invalid_function_option = None;
    while idx < ctx.args.len() {
        let arg = &ctx.args[idx];
        if arg.is_empty() || arg == "--" {
            if arg == "--" {
                idx += 1;
            }
            break;
        }
        let (plus, body) = if let Some(b) = arg.strip_prefix('-') {
            (false, b)
        } else if let Some(b) = arg.strip_prefix('+') {
            (true, b)
        } else {
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
            let bad = body
                .chars()
                .find(|c| {
                    !matches!(
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
                })
                .unwrap_or('?');
            report_diagnostic(
                ctx.env_ref(),
                diagnostic_name,
                &format!("-{bad}: invalid option"),
            );
            eprintln!(
                "{diagnostic_name}: usage: declare [-aAfFgiIlnrtux] [name[=value] ...] or declare -p [-aAfFilnrtux] [name ...]"
            );
            return 2;
        }
        for ch in body.chars() {
            if f.function && matches!(ch, 'a' | 'A' | 'i' | 'l' | 'n' | 'u' | 'c') {
                invalid_function_option.get_or_insert(ch);
            }
            let attr = match ch {
                'a' => Some(VarAttrs::ARRAY),
                'A' => Some(VarAttrs::ASSOC),
                'i' => Some(VarAttrs::INTEGER),
                'l' => Some(VarAttrs::LOWERCASE),
                'u' => Some(VarAttrs::UPPERCASE),
                'c' => Some(VarAttrs::CAPCASE),
                'r' => Some(VarAttrs::READONLY),
                't' => Some(VarAttrs::TRACE),
                'x' => Some(VarAttrs::EXPORT),
                'n' => Some(VarAttrs::NAMEREF),
                'f' => {
                    f.function = true;
                    None
                }
                'F' => {
                    f.function = true;
                    f.function_names_only = true;
                    None
                }
                'g' => {
                    f.global = true;
                    None
                }
                'I' => {
                    f.inherit = true;
                    None
                }
                'p' => {
                    f.print = true;
                    None
                }
                _ => None,
            };
            if let Some(a) = attr {
                if plus {
                    f.clear.insert(a);
                } else {
                    f.set.insert(a);
                }
            }
        }
        idx += 1;
    }

    if let Some(ch) = invalid_function_option {
        report_diagnostic(
            ctx.env_ref(),
            diagnostic_name,
            &format!("-{ch}: invalid option"),
        );
        return 1;
    }

    let rest = &ctx.args[idx..];

    if f.function && rest.is_empty() {
        return print_function_listing(ctx, &f);
    }
    if f.print && !f.function && !rest.is_empty() {
        let mut status = 0;
        for name in rest {
            if let Some(snap) = print_lookup_var(ctx, diagnostic_name, name)
                .filter(|snap| declare_filter_matches(snap, &f))
            {
                println!("{}", format_var(&snap, Some("declare")));
            } else {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{name}: not found"),
                );
                status = 1;
            }
        }
        return status;
    }
    if rest.is_empty() && (f.print || !f.set.is_empty() || !f.clear.is_empty()) {
        if diagnostic_name == "local" && f.print && ctx.env_ref().local_options_active() {
            println!("local -");
        }
        for snap in listing_vars(ctx, diagnostic_name) {
            if !declare_filter_matches(&snap, &f) {
                continue;
            }
            println!("{}", format_var(&snap, Some("declare")));
        }
        return 0;
    }
    if rest.is_empty() && f.set.is_empty() && f.clear.is_empty() {
        let posix = ctx.env_ref().option("posix");
        for snap in listing_vars(ctx, diagnostic_name) {
            if diagnostic_name == "local" {
                println!("{}", format_var(&snap, Some("declare")));
            } else if let Some(line) = format_declare_listing_var(&snap, posix) {
                println!("{line}");
            }
        }
        return 0;
    }

    let mut status = 0;
    if diagnostic_name == "local" && rest.len() == 1 && rest[0] == "-" {
        ctx.env().make_options_local();
        return 0;
    }
    for (offset, arg) in rest.iter().enumerate() {
        let mut f = f;
        if f.function {
            status |= handle_function_arg(ctx, &f, arg);
            continue;
        }

        let assignment = split_assignment_op(arg)
            .map(|(lhs, rhs, append)| (lhs.to_string(), rhs.to_string(), append));
        let malformed_assoc_assignment = if assignment.is_none() {
            assoc_expand_once_malformed_assignment(ctx.env_ref(), arg)
        } else {
            None
        };
        let (name, value, array_ref_without_value): (String, bool, bool) = match assignment.as_ref()
        {
            Some((lhs, _rhs, _append)) => {
                let Some(base) = assignment_base_name(lhs) else {
                    report_diagnostic(
                        ctx.env_ref(),
                        diagnostic_name,
                        &format!("`{arg}': not a valid identifier"),
                    );
                    status = 1;
                    continue;
                };
                (base.to_string(), true, false)
            }
            None if let Some((base, _key, _rhs, _append)) = malformed_assoc_assignment.as_ref() => {
                (base.clone(), true, false)
            }
            None => {
                let base = if let Some((base, _)) = array_reference(arg) {
                    (base, true)
                } else if is_valid_name(arg) {
                    (arg.as_str(), false)
                } else {
                    report_diagnostic(
                        ctx.env_ref(),
                        diagnostic_name,
                        &format!("`{arg}': not a valid identifier"),
                    );
                    status = 1;
                    continue;
                };
                (base.0.to_string(), false, base.1)
            }
        };
        if f.set.contains(VarAttrs::NAMEREF)
            && f.set.contains(VarAttrs::INTEGER)
            && value
            && !f.set.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
        {
            status = 1;
            continue;
        }
        if f.set.contains(VarAttrs::NAMEREF) && f.set.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
        {
            f.set.remove(VarAttrs::NAMEREF);
        }

        if f.print && array_ref_without_value && !value {
            report_diagnostic(ctx.env_ref(), diagnostic_name, &format!("{arg}: not found"));
            status = 1;
            continue;
        }

        let should_localize = force_local
            && !f.global
            && ctx.shell.function_depth() > 0
            && (!f.print || value || !f.set.is_empty() || !f.clear.is_empty());
        let visible_kind_before_localize = if f.global {
            ctx.env_ref().global_kind(&name)
        } else {
            ctx.env_ref().kind(&name)
        };
        if should_localize {
            let compound_arg = value && ctx.arg_flag(idx + offset) & W_COMPASSIGN != 0;
            let temporary_assignment = !value
                && (ctx.assignments.iter().any(|(key, _)| key == &name)
                    || ctx.shell.current_function_prefix_assignment(&name));
            let inherit_local = f.inherit
                || compound_arg
                || temporary_assignment
                || (ctx.env_ref().option("localvar_inherit") && !f.set.contains(VarAttrs::NAMEREF));
            let local_result = if inherit_local {
                ctx.env().make_local_inherit(&name)
            } else {
                ctx.env().make_local(&name)
            };
            if let Err(err) = local_result {
                match &err {
                    AssignError::ReadOnly(readonly_name) => {
                        if compound_arg {
                            report_diagnostic(ctx.env_ref(), readonly_name, "readonly variable");
                        }
                        report_builtin_readonly_error(
                            ctx.env_ref(),
                            diagnostic_name,
                            readonly_name,
                        );
                    }
                    _ => report_assign_error(ctx.env_ref(), &err),
                }
                status = 1;
                continue;
            }
            if compound_arg
                && !f.inherit
                && !ctx.env_ref().option("localvar_inherit")
                && !f.set.contains(VarAttrs::READONLY)
            {
                ctx.env().set_attr(&name, VarAttrs::READONLY, false);
            }
        }

        if f.function {
            if let Some(function) = ctx.shell.function_get(&name) {
                if f.set.contains(VarAttrs::TRACE) {
                    ctx.shell.function_set_trace(&name, true);
                }
                if f.clear.contains(VarAttrs::TRACE) {
                    ctx.shell.function_set_trace(&name, false);
                }
                if f.function_names_only {
                    println!("declare -f {name}");
                } else if f.print || f.set.is_empty() && f.clear.is_empty() {
                    print_function_definition(&name, &function);
                }
            } else if f.print {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{name}: not found"),
                );
                status = 1;
            }
            continue;
        }

        if f.print && !value {
            if let Some(snap) = listing_vars(ctx, diagnostic_name)
                .into_iter()
                .find(|s| s.name == name)
                .filter(|snap| declare_filter_matches(snap, &f))
            {
                println!("{}", format_var(&snap, Some("declare")));
            } else {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{name}: not found"),
                );
                status = 1;
            }
            continue;
        }

        let declared_kind = if f.global {
            ctx.env_ref().global_kind(&name)
        } else {
            ctx.env_ref().kind(&name)
        };
        let declared_attrs = if f.global {
            ctx.env_ref().global_attrs(&name)
        } else {
            ctx.env_ref().attrs(&name)
        };

        if (f.clear.contains(VarAttrs::ARRAY) || f.clear.contains(VarAttrs::ASSOC))
            && declared_attrs.contains(VarAttrs::READONLY)
        {
            report_builtin_readonly_error(ctx.env_ref(), diagnostic_name, &name);
            status = 1;
            continue;
        }
        if f.clear.contains(VarAttrs::ARRAY)
            && (declared_attrs.contains(VarAttrs::ARRAY)
                || matches!(declared_kind, VarKind::Indexed))
        {
            report_diagnostic(
                ctx.env_ref(),
                diagnostic_name,
                &format!("{name}: cannot destroy array variables in this way"),
            );
            status = 1;
            continue;
        }
        if f.clear.contains(VarAttrs::ASSOC)
            && (declared_attrs.contains(VarAttrs::ASSOC) || matches!(declared_kind, VarKind::Assoc))
        {
            report_diagnostic(
                ctx.env_ref(),
                diagnostic_name,
                &format!("{name}: cannot destroy array variables in this way"),
            );
            status = 1;
            continue;
        }
        if f.set.contains(VarAttrs::ASSOC) && matches!(declared_kind, VarKind::Indexed) {
            if value {
                if let Some(function_name) = current_function_name(ctx) {
                    report_diagnostic(
                        ctx.env_ref(),
                        &function_name,
                        &format!("{name}: cannot convert indexed to associative array"),
                    );
                } else {
                    report_diagnostic(
                        ctx.env_ref(),
                        &name,
                        "cannot convert indexed to associative array",
                    );
                    status = 1;
                    continue;
                }
            }
            report_diagnostic(
                ctx.env_ref(),
                diagnostic_name,
                &format!("{name}: cannot convert indexed to associative array"),
            );
            status = 1;
            continue;
        }
        if f.set.contains(VarAttrs::ARRAY) && matches!(declared_kind, VarKind::Assoc) {
            if value {
                if let Some(function_name) = current_function_name(ctx) {
                    report_diagnostic(
                        ctx.env_ref(),
                        &function_name,
                        &format!("{name}: cannot convert associative to indexed array"),
                    );
                } else {
                    report_diagnostic(
                        ctx.env_ref(),
                        &name,
                        "cannot convert associative to indexed array",
                    );
                    status = 1;
                    continue;
                }
            }
            report_diagnostic(
                ctx.env_ref(),
                diagnostic_name,
                &format!("{name}: cannot convert associative to indexed array"),
            );
            status = 1;
            continue;
        }
        if f.set.contains(VarAttrs::NAMEREF) {
            if array_ref_without_value {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{arg}: reference variable cannot be an array"),
                );
                status = 1;
                continue;
            }
            if value && ctx.arg_flag(idx + offset) & W_COMPASSIGN != 0 {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{name}: reference variable cannot be an array"),
                );
                if f.global {
                    apply_assignment_arg_global(ctx.env(), arg, true);
                } else {
                    ctx.shell.apply_assignment_arg(arg, true);
                }
                status = 1;
                continue;
            }
            if let Some((lhs, rhs, _append)) = assignment.as_ref() {
                if array_reference(lhs).is_some() {
                    report_diagnostic(
                        ctx.env_ref(),
                        diagnostic_name,
                        &format!("{lhs}: reference variable cannot be an array"),
                    );
                    status = 1;
                    continue;
                }
                let target_value = nameref_assignment_target_value(ctx, &name, rhs, *_append);
                if target_value.is_empty() {
                    report_diagnostic(ctx.env_ref(), diagnostic_name, "`': not a valid identifier");
                    status = 1;
                    continue;
                }
                if !nameref_target_is_valid(&target_value) {
                    report_diagnostic(
                        ctx.env_ref(),
                        diagnostic_name,
                        &format!("`{target_value}': invalid variable name for name reference"),
                    );
                    status = 1;
                    continue;
                }
                if f.set.contains(VarAttrs::INTEGER)
                    && array_reference(&target_value).is_some()
                    && ctx.env_ref().kind(&name) == VarKind::Unset
                {
                    continue;
                }
                if nameref_target_base(&target_value).is_some_and(|base| base == name) {
                    if should_localize && !f.global {
                        report_diagnostic(
                            ctx.env_ref(),
                            diagnostic_name,
                            &format!("warning: {name}: circular name reference"),
                        );
                        report_circular_name_reference(ctx.env_ref(), &name);
                    } else {
                        if *_append {
                            report_diagnostic(
                                ctx.env_ref(),
                                &name,
                                "nameref variable self references not allowed",
                            );
                        } else {
                            report_diagnostic(
                                ctx.env_ref(),
                                diagnostic_name,
                                &format!("{name}: nameref variable self references not allowed"),
                            );
                        }
                        status = 1;
                        continue;
                    }
                }
            } else if let Some(current) = ctx.env_ref().get(&name) {
                if !nameref_target_is_valid(&current) {
                    report_diagnostic(
                        ctx.env_ref(),
                        diagnostic_name,
                        &format!("`{current}': invalid variable name for name reference"),
                    );
                    status = 1;
                    continue;
                }
            }
            if !value && ctx.env_ref().is_readonly(&name) {
                report_builtin_readonly_error(ctx.env_ref(), diagnostic_name, &name);
                status = 1;
                continue;
            }
            if f.set.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
                || matches!(declared_kind, VarKind::Indexed | VarKind::Assoc)
            {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{name}: reference variable cannot be an array"),
                );
                status = 1;
                continue;
            }
        }

        if array_ref_without_value && !f.set.contains(VarAttrs::ASSOC) {
            ctx.env().set_attr(&name, VarAttrs::ARRAY, true);
        }

        let readonly_attr_name = readonly_attr_target_name(ctx, &name, &f);
        let readonly_is_readonly = if f.global {
            ctx.env_ref().global_is_readonly(&readonly_attr_name)
        } else {
            ctx.env_ref().is_readonly(&readonly_attr_name)
        };
        let nameref_target = ctx.env_ref().nameref_target(&name);
        let unresolved_nameref = ctx.env_ref().attrs(&name).contains(VarAttrs::NAMEREF)
            && nameref_target.as_deref().unwrap_or_default().is_empty();
        if f.clear.contains(VarAttrs::READONLY) && readonly_is_readonly {
            if unresolved_nameref {
                continue;
            }
            report_builtin_readonly_error(ctx.env_ref(), diagnostic_name, &readonly_attr_name);
            status = 1;
            continue;
        }
        let has_nameref_target = nameref_target
            .as_deref()
            .is_some_and(|target| !target.is_empty());
        if f.clear.contains(VarAttrs::NAMEREF)
            && ctx.env_ref().is_readonly(&name)
            && has_nameref_target
        {
            report_builtin_readonly_error(ctx.env_ref(), diagnostic_name, &name);
            status = 1;
            continue;
        }

        let assigns_array_member = assignment
            .as_ref()
            .is_some_and(|(lhs, _, _)| array_reference(lhs).is_some());
        if f.set.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
            && !f.set.contains(VarAttrs::NAMEREF)
            && ctx.env_ref().attrs(&name).contains(VarAttrs::NAMEREF)
            && (assigns_array_member
                || ctx
                    .env_ref()
                    .resolve_nameref(&name)
                    .as_deref()
                    .is_some_and(|target| target == name))
        {
            if assigns_array_member {
                report_removing_nameref_attribute(ctx.env_ref(), &name);
            }
            if f.global {
                ctx.env().set_global_attr(&name, VarAttrs::NAMEREF, false);
                if assigns_array_member {
                    ctx.env().set_global_array(&name, Vec::new());
                }
            } else {
                ctx.env().set_attr(&name, VarAttrs::NAMEREF, false);
                if assigns_array_member {
                    ctx.env().set_array(&name, Vec::new());
                }
            }
        }

        let attr_target_name = attr_target_name(ctx, &name, &f);
        if !f.global
            && attr_target_name != name
            && ctx.env_ref().attrs(&name).contains(VarAttrs::NAMEREF)
        {
            ctx.env()
                .prepare_nameref_target_assignment(&name, &attr_target_name);
        }

        // Apply attributes first so set/assign respects them.
        for attr in [
            VarAttrs::ARRAY,
            VarAttrs::ASSOC,
            VarAttrs::INTEGER,
            VarAttrs::LOWERCASE,
            VarAttrs::UPPERCASE,
            VarAttrs::CAPCASE,
            VarAttrs::TRACE,
            VarAttrs::EXPORT,
            VarAttrs::NAMEREF,
        ] {
            let attr_name = if attr.contains(VarAttrs::NAMEREF) {
                &name
            } else {
                &attr_target_name
            };
            if f.set.contains(attr) {
                if f.global {
                    ctx.env().set_global_attr(attr_name, attr, true);
                } else {
                    ctx.env().set_attr(attr_name, attr, true);
                }
                if attr.contains(VarAttrs::EXPORT) {
                    if f.global {
                        ctx.env().export_global(attr_name);
                    } else {
                        ctx.env().export(attr_name);
                    }
                }
            }
            if f.clear.contains(attr) {
                if attr.contains(VarAttrs::NAMEREF) && value {
                    continue;
                }
                if f.global {
                    ctx.env().set_global_attr(attr_name, attr, false);
                } else {
                    ctx.env().set_attr(attr_name, attr, false);
                }
            }
        }
        if !value && !f.print {
            if f.global {
                ctx.env().set_global_attr(&name, VarAttrs::empty(), true);
            } else {
                ctx.env().set_attr(&name, VarAttrs::empty(), true);
            }
        }
        if value
            && ctx.env_ref().option("allexport")
            && !f.clear.contains(VarAttrs::EXPORT)
            && !f.set.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
        {
            if f.global {
                ctx.env()
                    .set_global_attr(&attr_target_name, VarAttrs::EXPORT, true);
            } else {
                ctx.env()
                    .set_attr(&attr_target_name, VarAttrs::EXPORT, true);
            }
        }

        if value {
            if let Some((base, key, rhs, append)) = malformed_assoc_assignment.as_ref() {
                let readonly = if f.global {
                    ctx.env_ref().global_is_readonly(base)
                } else {
                    ctx.env_ref().is_readonly(base)
                };
                if readonly {
                    report_diagnostic(ctx.env_ref(), base, "readonly variable");
                    status = 1;
                    continue;
                }
                let value = if *append {
                    let old = ctx.env_ref().get_array_assoc(base, key).unwrap_or_default();
                    if old.is_empty() {
                        rhs.clone()
                    } else {
                        format!("{old}{rhs}")
                    }
                } else {
                    rhs.clone()
                };
                if f.global {
                    ctx.env().set_global_array_assoc(base, key, value);
                } else {
                    ctx.env().set_array_assoc(base, key, value);
                }
                continue;
            }
            let (lhs, rhs, append) = assignment.as_ref().expect("value implies assignment");
            let compound = ctx.arg_flag(idx + offset) & W_COMPASSIGN != 0;
            let mut reset_on_compound_failure = false;
            let assign_status = if compound {
                if array_reference(lhs).is_some() {
                    report_diagnostic(ctx.env_ref(), lhs, "cannot assign list to array member");
                    1
                } else {
                    if f.set.contains(VarAttrs::ASSOC)
                        && matches!(visible_kind_before_localize, VarKind::Scalar)
                    {
                        ctx.env().preserve_assoc_order_for_next_assignment(&name);
                    }
                    reset_on_compound_failure = !append;
                    if f.global {
                        let visible_target = ctx
                            .env_ref()
                            .resolve_nameref(&name)
                            .filter(|target| !target.is_empty())
                            .unwrap_or_else(|| name.clone());
                        let global_target = global_assignment_target_name(ctx, &name);
                        let visible_target_existed = ctx.env_ref().kind(&visible_target)
                            != VarKind::Unset
                            || ctx.env_ref().get(&visible_target).is_some();
                        let status = apply_assignment_arg_global(ctx.env(), arg, true);
                        if visible_target != global_target && !visible_target_existed {
                            ctx.env().unset(&visible_target);
                        }
                        status
                    } else {
                        ctx.shell.apply_assignment_arg(arg, true)
                    }
                }
            } else if is_valid_name(lhs) {
                if should_treat_as_compound(ctx.env_ref(), &name, &f, rhs) {
                    let op = if *append { "+=" } else { "=" };
                    let base_assignment = format!("{lhs}{op}{rhs}");
                    if f.set.contains(VarAttrs::ASSOC)
                        && matches!(visible_kind_before_localize, VarKind::Scalar)
                    {
                        ctx.env().preserve_assoc_order_for_next_assignment(&name);
                    }
                    reset_on_compound_failure = !append;
                    if f.global {
                        apply_assignment_arg_global(ctx.env(), &base_assignment, true)
                    } else {
                        ctx.shell.apply_assignment_arg(&base_assignment, true)
                    }
                } else if f.set.contains(VarAttrs::NAMEREF) {
                    if ctx.env_ref().is_readonly(&name) {
                        report_builtin_readonly_error(ctx.env_ref(), diagnostic_name, &name);
                        1
                    } else {
                        let value = if *append {
                            let current = ctx
                                .env_ref()
                                .var_snapshot(&name)
                                .and_then(|snap| snap.nameref_target.or(snap.scalar))
                                .unwrap_or_default();
                            format!("{current}{rhs}")
                        } else {
                            rhs.clone()
                        };
                        ctx.env().set_attr(&name, VarAttrs::NAMEREF, false);
                        ctx.env().set_attr(&name, VarAttrs::INTEGER, false);
                        ctx.env().set_attr(&name, VarAttrs::LOWERCASE, false);
                        ctx.env().set_attr(&name, VarAttrs::UPPERCASE, false);
                        ctx.env().set_attr(&name, VarAttrs::CAPCASE, false);
                        match assign_value(ctx.env(), &name, value, false) {
                            Ok(()) => {
                                ctx.env().set_attr(&name, VarAttrs::NAMEREF, true);
                                0
                            }
                            Err(err) => {
                                if matches!(err, AssignError::ReadOnly(_)) {
                                    report_builtin_readonly_error(
                                        ctx.env_ref(),
                                        diagnostic_name,
                                        &name,
                                    );
                                } else {
                                    report_builtin_assign_error(
                                        ctx.env_ref(),
                                        diagnostic_name,
                                        &err,
                                    );
                                }
                                1
                            }
                        }
                    }
                } else {
                    let assign_result = if f.global {
                        assign_value_global(ctx.env(), &name, rhs.clone(), *append)
                    } else {
                        assign_value(ctx.env(), &name, rhs.clone(), *append)
                    };
                    match assign_result {
                        Ok(()) => 0,
                        Err(err) => {
                            let invalid_unresolved_nameref =
                                matches!(err, AssignError::InvalidName(_))
                                    && ctx.env_ref().attrs(&name).contains(VarAttrs::NAMEREF)
                                    && ctx
                                        .env_ref()
                                        .nameref_target(&name)
                                        .as_deref()
                                        .unwrap_or_default()
                                        .is_empty();
                            if matches!(err, AssignError::ReadOnly(_)) {
                                report_builtin_readonly_error(
                                    ctx.env_ref(),
                                    diagnostic_name,
                                    &name,
                                );
                            } else if invalid_unresolved_nameref && should_localize {
                                if let AssignError::InvalidName(target) = &err {
                                    report_diagnostic(
                                        ctx.env_ref(),
                                        diagnostic_name,
                                        &format!(
                                            "`{target}': invalid variable name for name reference"
                                        ),
                                    );
                                }
                            } else {
                                report_builtin_assign_error(ctx.env_ref(), diagnostic_name, &err);
                            }
                            if invalid_unresolved_nameref && !should_localize {
                                if f.global {
                                    ctx.env().unset_global(&name);
                                } else {
                                    ctx.env().unset(&name);
                                }
                            }
                            1
                        }
                    }
                }
            } else if let Some((base, _)) = array_reference(lhs) {
                if flags_request_array(&f)
                    && rhs.trim_start().starts_with('(')
                    && rhs.trim_end().ends_with(')')
                {
                    let op = if *append { "+=" } else { "=" };
                    let base_assignment = format!("{base}{op}{rhs}");
                    if f.global {
                        apply_assignment_arg_global(ctx.env(), &base_assignment, true)
                    } else {
                        ctx.shell.apply_assignment_arg(&base_assignment, true)
                    }
                } else {
                    if warns_quoted_compound_array_element(ctx.env_ref(), base, rhs) {
                        report_quoted_compound_array_element_warning(ctx.env_ref(), lhs, rhs);
                    }
                    let assign_result = if f.global {
                        assign_direct_target_op_global(ctx.env(), lhs, rhs.clone(), *append)
                    } else {
                        assign_direct_target_op(ctx.env(), lhs, rhs.clone(), *append)
                    };
                    match assign_result {
                        Ok(()) => 0,
                        Err(err) => {
                            report_assign_error(ctx.env_ref(), &err);
                            1
                        }
                    }
                }
            } else {
                1
            };
            if assign_status != 0 {
                if reset_on_compound_failure {
                    prepare_empty_compound_target(ctx, &name, &f);
                }
                status = 1;
                continue;
            }
        }

        if f.set.contains(VarAttrs::READONLY) {
            if f.global {
                ctx.env()
                    .set_global_attr(&readonly_attr_name, VarAttrs::READONLY, true);
            } else {
                ctx.env()
                    .set_attr(&readonly_attr_name, VarAttrs::READONLY, true);
            }
        }
        if f.clear.contains(VarAttrs::READONLY) {
            if f.global {
                ctx.env()
                    .set_global_attr(&readonly_attr_name, VarAttrs::READONLY, false);
            } else {
                ctx.env()
                    .set_attr(&readonly_attr_name, VarAttrs::READONLY, false);
            }
        }
        if f.clear.contains(VarAttrs::NAMEREF) && value {
            ctx.env().set_attr(&name, VarAttrs::NAMEREF, false);
        }

        if f.print {
            if let Some(snap) = ctx.env_ref().var_snapshot(&name) {
                println!("{}", format_var(&snap, Some("declare")));
            } else {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{name}: not found"),
                );
                status = 1;
            }
        }
    }
    status
}

fn current_function_name(ctx: &BuiltinCtx<'_>) -> Option<String> {
    (ctx.shell.function_depth() > 0)
        .then(|| ctx.env_ref().get_array_indexed("FUNCNAME", 0))
        .flatten()
}

fn print_function_listing(ctx: &mut BuiltinCtx<'_>, f: &Flags) -> i32 {
    let mut names = ctx.shell.function_names();
    names.sort();
    for name in names {
        let attrs = function_attrs(ctx, &name);
        if !function_attrs_match(attrs, f) {
            continue;
        }
        if f.function_names_only {
            print_function_name(ctx, &name, attrs, true, false);
        } else if let Some(function) = ctx.shell.function_get(&name) {
            print_function_definition(&name, &function);
        }
    }
    0
}

fn listing_vars(ctx: &BuiltinCtx<'_>, diagnostic_name: &str) -> Vec<VarSnapshot> {
    if diagnostic_name == "local" {
        ctx.env_ref().iter_local_vars()
    } else {
        ctx.env_ref().iter_vars()
    }
}

fn print_lookup_var(
    ctx: &BuiltinCtx<'_>,
    diagnostic_name: &str,
    name: &str,
) -> Option<VarSnapshot> {
    if diagnostic_name == "local" {
        return ctx
            .env_ref()
            .iter_local_vars()
            .into_iter()
            .find(|snap| snap.name == name);
    }
    ctx.env_ref().var_snapshot(name)
}

fn handle_function_arg(ctx: &mut BuiltinCtx<'_>, f: &Flags, name: &str) -> i32 {
    if name.contains('=') {
        report_diagnostic(
            ctx.env_ref(),
            "declare",
            "cannot use `-f' to make functions",
        );
        return 1;
    }

    let Some(function) = ctx.shell.function_get(name) else {
        if f.print {
            report_diagnostic(ctx.env_ref(), "declare", &format!("{name}: not found"));
        }
        return 1;
    };

    if f.clear.contains(VarAttrs::READONLY) && ctx.env_ref().function_is_readonly(name) {
        report_diagnostic(
            ctx.env_ref(),
            "declare",
            &format!("{name}: readonly function"),
        );
        return 1;
    }
    if f.set.contains(VarAttrs::READONLY) {
        ctx.env().function_set_readonly(name);
    }
    if f.set.contains(VarAttrs::TRACE) {
        ctx.shell.function_set_trace(name, true);
    }
    if f.clear.contains(VarAttrs::TRACE) {
        ctx.shell.function_set_trace(name, false);
    }
    if f.set.contains(VarAttrs::EXPORT) {
        std::env::set_var(
            exported_function_env_name(name),
            function_export_value(&function),
        );
    }
    if f.clear.contains(VarAttrs::EXPORT) {
        std::env::remove_var(exported_function_env_name(name));
    }

    if f.function_names_only {
        if f.print {
            print_function_name(ctx, name, function_attrs(ctx, name), true, false);
        } else if (f.set | f.clear).is_empty() {
            print_function_name(ctx, name, function_attrs(ctx, name), false, true);
        }
    } else if f.print {
        print_function_definition(name, &function);
    } else if (f.set | f.clear).is_empty() {
        print_function_definition(name, &function);
    }
    0
}

fn print_function_name(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    attrs: VarAttrs,
    prefix_declare: bool,
    source_info: bool,
) {
    let prefix = if prefix_declare {
        format!("{} ", function_declare_prefix(attrs))
    } else {
        String::new()
    };
    if source_info && ctx.env_ref().option("extdebug") {
        if let Some((source, line)) = ctx.shell.function_source(name) {
            println!("{prefix}{name} {line} {source}");
            return;
        }
    }
    println!("{prefix}{name}");
}

fn function_attrs(ctx: &BuiltinCtx<'_>, name: &str) -> VarAttrs {
    let mut attrs = VarAttrs::empty();
    if ctx.env_ref().function_is_readonly(name) {
        attrs.insert(VarAttrs::READONLY);
    }
    if ctx.shell.function_is_trace(name) {
        attrs.insert(VarAttrs::TRACE);
    }
    if std::env::var_os(exported_function_env_name(name)).is_some() {
        attrs.insert(VarAttrs::EXPORT);
    }
    attrs
}

fn function_attrs_match(attrs: VarAttrs, f: &Flags) -> bool {
    let relevant = VarAttrs::READONLY | VarAttrs::TRACE | VarAttrs::EXPORT;
    attrs.contains(f.set & relevant) && (attrs & (f.clear & relevant)).is_empty()
}

fn readonly_attr_target_name(ctx: &BuiltinCtx<'_>, name: &str, flags: &Flags) -> String {
    if flags.set.contains(VarAttrs::NAMEREF) {
        return name.to_string();
    }
    if ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        resolved_attr_target_name(ctx, name)
    } else {
        name.to_string()
    }
}

fn attr_target_name(ctx: &BuiltinCtx<'_>, name: &str, flags: &Flags) -> String {
    if flags.set.contains(VarAttrs::NAMEREF) {
        return name.to_string();
    }
    if ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        resolved_attr_target_name(ctx, name)
    } else {
        name.to_string()
    }
}

fn resolved_attr_target_name(ctx: &BuiltinCtx<'_>, name: &str) -> String {
    ctx.env_ref()
        .resolve_nameref(name)
        .map(|target| {
            nameref_target_base(&target)
                .unwrap_or(target.as_str())
                .to_string()
        })
        .unwrap_or_else(|| name.to_string())
}

fn global_assignment_target_name(ctx: &BuiltinCtx<'_>, name: &str) -> String {
    if ctx.env_ref().global_attrs(name).contains(VarAttrs::NAMEREF) {
        ctx.env_ref()
            .global_get(name)
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    }
}

fn nameref_assignment_target_value(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    rhs: &str,
    append: bool,
) -> String {
    if !append {
        return rhs.to_string();
    }
    let current = ctx
        .env_ref()
        .var_snapshot(name)
        .and_then(|snap| snap.nameref_target.or(snap.scalar))
        .unwrap_or_default();
    format!("{current}{rhs}")
}

fn report_removing_nameref_attribute(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: warning: {name}: removing nameref attribute");
    } else {
        eprintln!("cherubsh: warning: {name}: removing nameref attribute");
    }
}

fn report_circular_name_reference(env: &dyn Environment, name: &str) {
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!("{source}: line {line}: warning: {name}: circular name reference");
    } else {
        eprintln!("cherubsh: warning: {name}: circular name reference");
    }
}

fn nameref_target_base(target: &str) -> Option<&str> {
    if is_valid_name(target) {
        Some(target)
    } else {
        array_reference(target).map(|(name, _)| name)
    }
}

fn nameref_target_is_valid(target: &str) -> bool {
    is_valid_name(target) || array_reference(target).is_some()
}

fn function_declare_prefix(attrs: VarAttrs) -> String {
    let mut out = String::from("declare -f");
    if attrs.contains(VarAttrs::READONLY) {
        out.push('r');
    }
    if attrs.contains(VarAttrs::TRACE) {
        out.push('t');
    }
    if attrs.contains(VarAttrs::EXPORT) {
        out.push('x');
    }
    out
}

fn exported_function_env_name(name: &str) -> String {
    format!("BASH_FUNC_{name}%%")
}

fn assoc_expand_once_malformed_assignment(
    env: &dyn Environment,
    arg: &str,
) -> Option<(String, String, String, bool)> {
    if !env.option("assoc_expand_once") {
        return None;
    }
    let eq = arg.find('=')?;
    let (lhs, rhs, append) = if eq > 0 && arg.as_bytes().get(eq - 1) == Some(&b'+') {
        (&arg[..eq - 1], &arg[eq + 1..], true)
    } else {
        (&arg[..eq], &arg[eq + 1..], false)
    };
    let open = lhs.find('[')?;
    let base = &lhs[..open];
    if !is_valid_name(base) || !matches!(env.kind(base), VarKind::Assoc) {
        return None;
    }
    let raw_key = &lhs[open + 1..];
    let key = raw_key.strip_suffix(']').unwrap_or(raw_key);
    if key.is_empty() || !key.contains('[') || key.contains(']') {
        return None;
    }
    Some((base.to_string(), key.to_string(), rhs.to_string(), append))
}

fn flags_request_array(flags: &Flags) -> bool {
    flags.set.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
}

fn warns_quoted_compound_array_element(env: &dyn Environment, base: &str, rhs: &str) -> bool {
    matches!(env.kind(base), VarKind::Unset | VarKind::Scalar)
        && rhs.trim_start().starts_with('(')
        && rhs.trim_end().ends_with(')')
}

fn report_quoted_compound_array_element_warning(env: &dyn Environment, lhs: &str, rhs: &str) {
    let assignment = format!("{lhs}={rhs}");
    if let (Some(source), Some(line)) = (env.diagnostic_source_name(), env.diagnostic_line()) {
        eprintln!(
            "{source}: line {line}: warning: {assignment}: quoted compound array assignment deprecated"
        );
    } else {
        eprintln!("cherubsh: warning: {assignment}: quoted compound array assignment deprecated");
    }
}

fn should_treat_as_compound(env: &dyn Environment, name: &str, flags: &Flags, rhs: &str) -> bool {
    let rhs = rhs.trim();
    if !(rhs.starts_with('(') && rhs.ends_with(')')) {
        return false;
    }
    flags_request_array(flags)
        || if flags.global {
            env.global_attrs(name)
                .intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
                || matches!(env.global_kind(name), VarKind::Indexed | VarKind::Assoc)
        } else {
            env.attrs(name)
                .intersects(VarAttrs::ARRAY | VarAttrs::ASSOC)
                || matches!(env.kind(name), VarKind::Indexed | VarKind::Assoc)
        }
}

fn prepare_empty_compound_target(ctx: &mut BuiltinCtx<'_>, name: &str, flags: &Flags) {
    let attrs = ctx.env_ref().attrs(name);
    if flags.set.contains(VarAttrs::ASSOC) || matches!(ctx.env_ref().kind(name), VarKind::Assoc) {
        ctx.env().set_array_assoc(name, "", String::new());
        ctx.env().unset_array_elem(name, "");
    } else {
        ctx.env().set_array(name, Vec::new());
    }
    for attr in [
        VarAttrs::ARRAY,
        VarAttrs::ASSOC,
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
    ] {
        if attrs.contains(attr) || flags.set.contains(attr) {
            ctx.env().set_attr(name, attr, true);
            if attr.contains(VarAttrs::EXPORT) {
                ctx.env().export(name);
            }
        }
    }
}

fn declare_filter_matches(snap: &VarSnapshot, flags: &Flags) -> bool {
    if flags.set.is_empty() && flags.clear.is_empty() {
        return true;
    }
    if flags.set.contains(VarAttrs::ARRAY)
        && !(snap.attrs.contains(VarAttrs::ARRAY) || matches!(snap.kind, VarKind::Indexed))
    {
        return false;
    }
    if flags.set.contains(VarAttrs::ASSOC)
        && !(snap.attrs.contains(VarAttrs::ASSOC) || matches!(snap.kind, VarKind::Assoc))
    {
        return false;
    }
    for attr in [
        VarAttrs::INTEGER,
        VarAttrs::LOWERCASE,
        VarAttrs::UPPERCASE,
        VarAttrs::CAPCASE,
        VarAttrs::TRACE,
        VarAttrs::EXPORT,
        VarAttrs::NAMEREF,
        VarAttrs::READONLY,
    ] {
        if flags.set.contains(attr) && !snap.attrs.contains(attr) {
            return false;
        }
    }
    true
}

pub fn run_local_form(ctx: &mut BuiltinCtx<'_>) -> i32 {
    if let Some(status) = try_simple_local(ctx) {
        return status;
    }
    run_declare(ctx, true, "local")
}

fn try_simple_local(ctx: &mut BuiltinCtx<'_>) -> Option<i32> {
    if ctx.args.is_empty()
        || !ctx.assignments.is_empty()
        || ctx.env_ref().option("localvar_inherit")
        || ctx.env_ref().option("allexport")
    {
        return None;
    }

    for (idx, arg) in ctx.args.iter().enumerate() {
        if arg.is_empty()
            || arg == "--"
            || arg.starts_with('-')
            || arg.starts_with('+')
            || ctx.arg_flag(idx) & W_COMPASSIGN != 0
        {
            return None;
        }
        match split_assignment_op(arg) {
            Some((lhs, _rhs, append))
                if !append && is_valid_name(lhs) && simple_local_assignment_target(ctx, lhs) => {}
            None if is_valid_name(arg)
                && !ctx.assignments.iter().any(|(key, _)| key == arg)
                && !ctx.shell.current_function_prefix_assignment(arg) => {}
            _ => return None,
        }
    }

    let mut status = 0;
    for idx in 0..ctx.args.len() {
        let (name, value) = {
            let arg = &ctx.args[idx];
            match split_assignment_op(arg) {
                Some((lhs, rhs, _append)) => (lhs.to_string(), Some(rhs.to_string())),
                None => (arg.clone(), None),
            }
        };

        if let Err(err) = ctx.env().make_local_with_value(&name, value) {
            if let AssignError::ReadOnly(readonly_name) = &err {
                report_builtin_readonly_error(ctx.env_ref(), "local", readonly_name);
            } else {
                report_assign_error(ctx.env_ref(), &err);
            }
            status = 1;
        }
    }
    Some(status)
}

fn simple_local_assignment_target(ctx: &BuiltinCtx<'_>, name: &str) -> bool {
    !ctx.env_ref().attrs(name).intersects(
        VarAttrs::ARRAY
            | VarAttrs::ASSOC
            | VarAttrs::INTEGER
            | VarAttrs::LOWERCASE
            | VarAttrs::UPPERCASE
            | VarAttrs::CAPCASE
            | VarAttrs::NAMEREF
            | VarAttrs::READONLY,
    )
}
