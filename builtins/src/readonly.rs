use cherubsh_common::{VarAttrs, VarKind, W_ASSIGNMENT, W_COMPASSIGN};
use cherubsh_expander::assignment::expand_assignment_word;
use cherubsh_expander::NullRunner;
use cherubsh_parser::WordDesc;

use crate::common::{
    apply_expanded_assignment, array_reference, assign_value, format_var, is_valid_name,
    parse_assignment_op, report_assign_error, report_builtin_readonly_error, report_diagnostic,
};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Readonly;
pub static READONLY: Readonly = Readonly;

impl Builtin for Readonly {
    fn name(&self) -> &'static str {
        "readonly"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::ASSIGNMENT | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "readonly [-aAf] [name[=value] ...] or readonly -p"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        for (key, value) in ctx.assignments {
            let _ = ctx.env().assign(key, value.clone());
            ctx.env().set_attr(key, VarAttrs::READONLY, true);
        }

        let mut function_form = false;
        let mut array_form = false;
        let mut assoc_form = false;
        let mut nameref_form = false;
        let mut print_only = false;
        let mut parser = OptParser::new(ctx.args, "afAnp");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'a', .. } => array_form = true,
                GetOpt::Opt { ch: 'A', .. } => assoc_form = true,
                GetOpt::Opt { ch: 'f', .. } => function_form = true,
                GetOpt::Opt { ch: 'n', .. } => nameref_form = true,
                GetOpt::Opt { ch: 'p', .. } => print_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "readonly", &format!("-{ch}: invalid option"));
                    eprintln!("readonly: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "readonly",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("readonly: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest_start = parser.index;
        let rest = parser.remaining(ctx.args);

        if print_only || rest.is_empty() {
            let prefix = if ctx.env_ref().option("posix") {
                "readonly"
            } else {
                "declare"
            };
            for snap in ctx.env_ref().iter_vars() {
                if !snap.attrs.contains(VarAttrs::READONLY) {
                    continue;
                }
                if array_form
                    && !(snap.attrs.contains(VarAttrs::ARRAY)
                        || matches!(snap.kind, VarKind::Indexed))
                {
                    continue;
                }
                if assoc_form
                    && !(snap.attrs.contains(VarAttrs::ASSOC)
                        || matches!(snap.kind, VarKind::Assoc))
                {
                    continue;
                }
                println!("{}", format_var(&snap, Some(prefix)));
            }
            return 0;
        }

        let mut status = 0;
        for (offset, arg) in rest.iter().enumerate() {
            if nameref_form {
                continue;
            }
            if let Some((name, value, append)) = parse_assignment_op(arg) {
                let readonly_name = readonly_target_name(ctx, &name);
                if readonly_name != name && array_reference(&readonly_name).is_some() {
                    report_diagnostic(
                        ctx.env_ref(),
                        "readonly",
                        &format!("`{readonly_name}': not a valid identifier"),
                    );
                    status = 1;
                    continue;
                }
                if array_form && readonly_name == name {
                    ctx.env().set_attr(&name, VarAttrs::ARRAY, true);
                }
                if assoc_form && readonly_name == name {
                    ctx.env().set_attr(&name, VarAttrs::ASSOC, true);
                }

                let arg_flags = ctx.arg_flag(rest_start + offset);
                let compound = (arg.contains("=(") || arg.contains("+=("))
                    && (arg_flags & W_COMPASSIGN != 0 || array_form || assoc_form);
                if compound && arg_flags & W_COMPASSIGN == 0 && ctx.env_ref().is_readonly(&name) {
                    report_builtin_readonly_error(ctx.env_ref(), "readonly", &name);
                    status = 1;
                    continue;
                }
                let assign_status = if compound {
                    let word = WordDesc {
                        text: arg.clone(),
                        flags: W_ASSIGNMENT | W_COMPASSIGN,
                        span: cherubsh_common::Span::dummy(),
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
                            1
                        }
                    }
                };
                if assign_status != 0 {
                    status = 1;
                    continue;
                }
                ctx.env().set_attr(&readonly_name, VarAttrs::READONLY, true);
            } else if is_valid_name(arg) {
                if function_form {
                    ctx.env().function_set_readonly(arg);
                    continue;
                }
                let readonly_name = readonly_target_name(ctx, arg);
                if readonly_name != *arg && array_reference(&readonly_name).is_some() {
                    report_diagnostic(
                        ctx.env_ref(),
                        "readonly",
                        &format!("`{readonly_name}': not a valid identifier"),
                    );
                    status = 1;
                    continue;
                }
                if array_form && readonly_name == arg.as_str() {
                    ctx.env().set_attr(arg, VarAttrs::ARRAY, true);
                }
                if assoc_form && readonly_name == arg.as_str() {
                    ctx.env().set_attr(arg, VarAttrs::ASSOC, true);
                }
                ctx.env().set_attr(&readonly_name, VarAttrs::READONLY, true);
            } else {
                report_diagnostic(
                    ctx.env_ref(),
                    "readonly",
                    &format!("`{arg}': not a valid identifier"),
                );
                status = 1;
            }
        }
        status
    }
}

fn readonly_target_name(ctx: &BuiltinCtx<'_>, name: &str) -> String {
    if ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        ctx.env_ref()
            .resolve_nameref(name)
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    }
}
