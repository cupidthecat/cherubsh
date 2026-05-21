use cherubsh_common::{VarAttrs, W_ASSIGNMENT, W_COMPASSIGN};
use cherubsh_expander::assignment::expand_assignment_word;
use cherubsh_expander::NullRunner;
use cherubsh_parser::WordDesc;

use crate::common::{
    apply_expanded_assignment, assign_value, format_var, is_valid_name, parse_assignment_op,
    report_assign_error, report_diagnostic,
};
use crate::getopt::{GetOpt, OptParser};
use crate::type_cmd::function_export_value;
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Export;
pub static EXPORT: Export = Export;

impl Builtin for Export {
    fn name(&self) -> &'static str {
        "export"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::ASSIGNMENT | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "export [-fn] [name[=value] ...] or export -p"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        // Apply implicit assignments first (`export FOO=bar` parses bar into
        // assignments; the simple-command pre-parses VAR=VAL prefix words).
        for (key, value) in ctx.assignments {
            if ctx.env().is_readonly(key) {
                report_diagnostic(ctx.env_ref(), key, "readonly variable");
                if posix_special_error_is_fatal(ctx) {
                    ctx.shell.request_exit(1);
                }
                return 1;
            }
            let _ = ctx.env().assign(key, value.clone());
            ctx.env().export(key);
            ctx.env().set_attr(key, VarAttrs::EXPORT, true);
            ctx.shell.note_exported(key);
        }

        let mut function_form = false;
        let mut clear_export = false;
        let mut print_only = false;
        let mut array_form = false;
        let mut assoc_form = false;
        let mut parser = OptParser::new(ctx.args, "faAnp");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'f', .. } => function_form = true,
                GetOpt::Opt { ch: 'a', .. } => array_form = true,
                GetOpt::Opt { ch: 'A', .. } => assoc_form = true,
                GetOpt::Opt { ch: 'n', .. } => clear_export = true,
                GetOpt::Opt { ch: 'p', .. } => print_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: export: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: export: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let rest_start = parser.index;
        let rest = parser.remaining(ctx.args);

        if function_form && !rest.is_empty() {
            return export_functions(ctx, rest, clear_export);
        }

        if print_only || rest.is_empty() {
            for snap in ctx.env_ref().iter_vars() {
                if !snap.attrs.contains(VarAttrs::EXPORT) {
                    continue;
                }
                if function_form {
                    continue;
                }
                println!("{}", format_var(&snap, Some("declare")));
            }
            return 0;
        }

        let mut status = 0;
        for (offset, arg) in rest.iter().enumerate() {
            if let Some((name, value, append)) = parse_assignment_op(arg) {
                if ctx.env().is_readonly(&name) {
                    report_diagnostic(ctx.env_ref(), &name, "readonly variable");
                    if posix_special_error_is_fatal(ctx) {
                        ctx.shell.request_exit(1);
                        return 1;
                    }
                    status = 1;
                    continue;
                }
                if array_form {
                    ctx.env().set_attr(&name, VarAttrs::ARRAY, true);
                }
                if assoc_form {
                    ctx.env().set_attr(&name, VarAttrs::ASSOC, true);
                }

                let arg_flags = ctx.arg_flag(rest_start + offset);
                let compound = (arg.contains("=(") || arg.contains("+=("))
                    && (arg_flags & W_COMPASSIGN != 0 || array_form || assoc_form);
                let assign_status = if compound {
                    let word = WordDesc {
                        text: arg.clone(),
                        flags: W_ASSIGNMENT | W_COMPASSIGN,
                        span: cherubsh_common::Span::dummy(),
                        raw: None,
                    };
                    let mut runner = NullRunner;
                    match expand_assignment_word(&word, ctx.env(), &mut runner) {
                        Ok(Some(assignment)) => apply_expanded_assignment(ctx.env(), &assignment),
                        Ok(None) => 1,
                        Err(err) => {
                            err.into_shell_error(Some(word.span)).report();
                            1
                        }
                    }
                } else {
                    match assign_value(ctx.env(), &name, value, append) {
                        Ok(()) => 0,
                        Err(err) => {
                            report_assign_error(ctx.env_ref(), &err);
                            if posix_special_error_is_fatal(ctx) {
                                ctx.shell.request_exit(1);
                                return 1;
                            }
                            1
                        }
                    }
                };
                if assign_status != 0 {
                    status = 1;
                    continue;
                }
                let export_name = export_target_name(ctx, &name);
                ctx.env().export(&export_name);
                ctx.env().set_attr(&export_name, VarAttrs::EXPORT, true);
                ctx.shell.note_exported(&export_name);
            } else if is_valid_name(arg) {
                if function_form {
                    // bash -f operates on function names; with no function
                    // table export, just mark.
                    continue;
                }
                let export_name = export_target_name(ctx, arg);
                if clear_export {
                    ctx.env().set_attr(&export_name, VarAttrs::EXPORT, false);
                } else {
                    if array_form {
                        ctx.env().set_attr(&export_name, VarAttrs::ARRAY, true);
                    }
                    if assoc_form {
                        ctx.env().set_attr(&export_name, VarAttrs::ASSOC, true);
                    }
                    ctx.env().export(&export_name);
                    ctx.env().set_attr(&export_name, VarAttrs::EXPORT, true);
                    if !ctx.shell.current_function_prefix_assignment(&export_name) {
                        ctx.shell.note_exported(&export_name);
                    }
                }
            } else {
                report_diagnostic(
                    ctx.env_ref(),
                    "export",
                    &format!("`{arg}': not a valid identifier"),
                );
                if posix_special_error_is_fatal(ctx) {
                    ctx.shell.request_exit(1);
                    return 1;
                }
                status = 1;
            }
        }
        status
    }
}

fn posix_special_error_is_fatal(ctx: &BuiltinCtx<'_>) -> bool {
    ctx.env_ref().option("posix")
        && !ctx.env_ref().option("interactive")
        && !ctx.invoked_via_command
}

fn export_target_name(ctx: &BuiltinCtx<'_>, name: &str) -> String {
    if ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        ctx.env_ref()
            .resolve_nameref(name)
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    }
}

fn export_functions(ctx: &mut BuiltinCtx<'_>, names: &[String], clear_export: bool) -> i32 {
    let mut status = 0;
    for name in names {
        if !function_name_exportable(name) {
            report_diagnostic(ctx.env_ref(), "export", &format!("{name}: cannot export"));
            status = 1;
            continue;
        }
        let env_name = exported_function_env_name(name);
        if clear_export {
            std::env::remove_var(env_name);
            continue;
        }
        let Some(function) = ctx.shell.function_get(name) else {
            report_diagnostic(ctx.env_ref(), "export", &format!("{name}: not a function"));
            status = 1;
            continue;
        };
        std::env::set_var(env_name, function_export_value(&function));
    }
    status
}

fn exported_function_env_name(name: &str) -> String {
    format!("BASH_FUNC_{name}%%")
}

fn function_name_exportable(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('=')
        && !name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}
