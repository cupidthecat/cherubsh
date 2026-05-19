use crate::common::{
    array_reference, is_valid_name, report_diagnostic, unset_array_reference,
    unset_array_reference_preserving_arrayref,
};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};
use cherubsh_common::{VarAttrs, VarKind, W_ARRAYREF, W_QUOTED};

pub struct Unset;
pub static UNSET: Unset = Unset;

impl Builtin for Unset {
    fn name(&self) -> &'static str {
        "unset"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "unset [-f] [-v] [-n] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        #[derive(Copy, Clone, PartialEq)]
        enum Kind {
            Default,
            Var,
            Func,
            Nameref,
        }
        let mut kind = Kind::Default;
        let mut saw_func = false;
        let mut saw_var = false;
        let mut parser = OptParser::new(ctx.args, "fvn");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'f', .. } => {
                    saw_func = true;
                    kind = Kind::Func;
                }
                GetOpt::Opt { ch: 'v', .. } => {
                    saw_var = true;
                    kind = Kind::Var;
                }
                GetOpt::Opt { ch: 'n', .. } => kind = Kind::Nameref,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "unset", &format!("-{ch}: invalid option"));
                    eprintln!("unset: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "unset",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("unset: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        if saw_func && saw_var {
            report_diagnostic(
                ctx.env_ref(),
                "unset",
                "cannot simultaneously unset a function and a variable",
            );
            return 2;
        }
        let mut status = 0;
        for (arg_index, name) in parser.remaining(ctx.args).iter().enumerate() {
            if kind == Kind::Func {
                if ctx.env_ref().function_is_readonly(name) {
                    report_diagnostic(
                        ctx.env_ref(),
                        "unset",
                        &format!("{name}: cannot unset: readonly function"),
                    );
                    status = 1;
                    continue;
                }
                ctx.shell.function_remove(name);
                std::env::remove_var(format!("BASH_FUNC_{name}%%"));
                continue;
            }
            let arg_flag = ctx.arg_flag(parser.index + arg_index);
            if kind == Kind::Var
                && arg_flag & W_ARRAYREF != 0
                && arg_flag & W_QUOTED == 0
                && name.contains("$(")
                && name.contains(char::is_whitespace)
            {
                for part in name.split_whitespace() {
                    report_diagnostic(
                        ctx.env_ref(),
                        "unset",
                        &format!("`{part}': not a valid identifier"),
                    );
                }
                status = 1;
                continue;
            }
            let preserve_array_ref =
                arg_flag & W_ARRAYREF != 0 && (kind == Kind::Var || arg_flag & W_QUOTED != 0);
            let unset_result = if preserve_array_ref {
                unset_array_reference_preserving_arrayref(ctx.env(), name)
            } else {
                unset_array_reference(ctx.env(), name)
            };
            match unset_result {
                Ok(true) => continue,
                Ok(false) => {}
                Err(message) => {
                    report_diagnostic(ctx.env_ref(), "unset", &message);
                    status = 1;
                    continue;
                }
            }
            if !is_valid_name(name) {
                if kind == Kind::Default || kind == Kind::Func {
                    continue;
                }
                report_diagnostic(
                    ctx.env_ref(),
                    "unset",
                    &format!("`{name}': not a valid identifier"),
                );
                status = 1;
                continue;
            }
            match kind {
                Kind::Default => {
                    if ctx.env_ref().kind(name) == VarKind::Unset
                        && ctx.shell.function_get(name).is_some()
                    {
                        if ctx.env_ref().function_is_readonly(name) {
                            report_diagnostic(
                                ctx.env_ref(),
                                "unset",
                                &format!("{name}: cannot unset: readonly function"),
                            );
                            status = 1;
                            continue;
                        }
                        ctx.shell.function_remove(name);
                        std::env::remove_var(format!("BASH_FUNC_{name}%%"));
                        continue;
                    }
                    let target = unset_variable_target(ctx, name);
                    if ctx.env().is_readonly(&target) {
                        report_diagnostic(
                            ctx.env_ref(),
                            "unset",
                            &format!("{target}: cannot unset: readonly variable"),
                        );
                        status = 1;
                        continue;
                    }
                    if nameref_scalar_array_unset_is_noop(ctx, name, &target) {
                        continue;
                    }
                    if array_reference(&target).is_some() {
                        match unset_array_reference_preserving_arrayref(ctx.env(), &target) {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(message) => {
                                report_diagnostic(ctx.env_ref(), "unset", &message);
                                status = 1;
                                continue;
                            }
                        }
                    }
                    ctx.env().unset(&target);
                }
                Kind::Func => {
                    if ctx.env_ref().function_is_readonly(name) {
                        report_diagnostic(
                            ctx.env_ref(),
                            "unset",
                            &format!("{name}: cannot unset: readonly function"),
                        );
                        status = 1;
                        continue;
                    }
                    ctx.shell.function_remove(name);
                    std::env::remove_var(format!("BASH_FUNC_{name}%%"));
                }
                Kind::Var => {
                    let target = unset_variable_target(ctx, name);
                    if ctx.env().is_readonly(&target) {
                        report_diagnostic(
                            ctx.env_ref(),
                            "unset",
                            &format!("{target}: cannot unset: readonly variable"),
                        );
                        status = 1;
                        continue;
                    }
                    if nameref_scalar_array_unset_is_noop(ctx, name, &target) {
                        continue;
                    }
                    if array_reference(&target).is_some() {
                        match unset_array_reference_preserving_arrayref(ctx.env(), &target) {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(message) => {
                                report_diagnostic(ctx.env_ref(), "unset", &message);
                                status = 1;
                                continue;
                            }
                        }
                    }
                    ctx.env().unset(&target);
                }
                Kind::Nameref => {
                    // Unset the nameref itself, not its target.
                    if ctx.env().is_readonly(name) {
                        report_diagnostic(
                            ctx.env_ref(),
                            "unset",
                            &format!("{name}: cannot unset: readonly variable"),
                        );
                        status = 1;
                        continue;
                    }
                    if !ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
                        continue;
                    }
                    ctx.env().unset(name);
                }
            }
        }
        status
    }
}

fn unset_variable_target(ctx: &BuiltinCtx<'_>, name: &str) -> String {
    if ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        ctx.env_ref()
            .resolve_nameref(name)
            .filter(|target| !target.is_empty())
            .unwrap_or_else(|| name.to_string())
    } else {
        name.to_string()
    }
}

fn nameref_scalar_array_unset_is_noop(ctx: &BuiltinCtx<'_>, name: &str, target: &str) -> bool {
    if !ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        return false;
    }
    let Some((base, subscript)) = array_reference(target) else {
        return false;
    };
    if ctx.env_ref().kind(base) != VarKind::Scalar {
        return false;
    }
    subscript
        .trim()
        .parse::<i64>()
        .is_ok_and(|index| index != 0)
}
