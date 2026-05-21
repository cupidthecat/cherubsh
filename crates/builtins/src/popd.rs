use std::path::PathBuf;

use crate::common::report_diagnostic;
use crate::dirs::{print_dirs, resolve_index, usage, DirsOptions, StackIndex};
use crate::{Builtin, BuiltinCtx};

pub struct Popd;
pub static POPD: Popd = Popd;

impl Builtin for Popd {
    fn name(&self) -> &'static str {
        "popd"
    }
    fn synopsis(&self) -> &'static str {
        "popd [-n] [+N | -N]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut skip_cd = false;
        let mut selected: Option<(String, StackIndex)> = None;
        for arg in ctx.args {
            if arg == "--" {
                break;
            }
            if arg == "-n" {
                skip_cd = true;
                continue;
            }
            let Some(index) = parse_stack_index(arg) else {
                report_diagnostic(ctx.env_ref(), "popd", &format!("{arg}: invalid argument"));
                usage("popd", "popd [-n] [+N | -N]");
                return 2;
            };
            match index {
                Ok(index) => selected = Some((arg.clone(), index)),
                Err(()) => {
                    report_diagnostic(ctx.env_ref(), "popd", &format!("{arg}: invalid number"));
                    usage("popd", "popd [-n] [+N | -N]");
                    return 2;
                }
            }
        }

        let stack = ctx.env_ref().dirs_iter();
        if stack.len() < 2 {
            report_diagnostic(ctx.env_ref(), "popd", "directory stack empty");
            return 1;
        }
        let (arg, index) = selected.unwrap_or_else(|| ("+0".to_string(), StackIndex::Left(0)));
        let remove = if skip_cd && matches!(index, StackIndex::Left(0)) {
            1
        } else {
            let Some(i) = resolve_index(stack.len(), index) else {
                report_diagnostic(
                    ctx.env_ref(),
                    "popd",
                    &format!("{arg}: directory stack index out of range"),
                );
                return 1;
            };
            i
        };
        if remove >= stack.len() {
            report_diagnostic(
                ctx.env_ref(),
                "popd",
                &format!("{arg}: directory stack index out of range"),
            );
            return 1;
        }
        remove_index(ctx, stack, remove, skip_cd)
    }
}

fn parse_stack_index(arg: &str) -> Option<Result<StackIndex, ()>> {
    if let Some(rest) = arg.strip_prefix('+') {
        return Some(parse_number(rest).map(StackIndex::Left));
    }
    if let Some(rest) = arg.strip_prefix('-') {
        if rest == "n" {
            return None;
        }
        return Some(parse_number(rest).map(StackIndex::Right));
    }
    None
}

fn parse_number(rest: &str) -> Result<usize, ()> {
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(());
    }
    rest.parse::<usize>().map_err(|_| ())
}

fn remove_index(
    ctx: &mut BuiltinCtx<'_>,
    mut stack: Vec<PathBuf>,
    remove: usize,
    skip_cd: bool,
) -> i32 {
    let old_top = stack[0].clone();
    stack.remove(remove);
    if stack.is_empty() {
        ctx.env().dirs_set_stack(Vec::new());
        return 0;
    }
    let new_top = stack[0].clone();
    let saved = stack.iter().skip(1).cloned().collect::<Vec<_>>();
    ctx.env().dirs_set_stack(saved);
    if remove == 0 && !skip_cd {
        if let Err(err) = std::env::set_current_dir(&new_top) {
            report_diagnostic(
                ctx.env_ref(),
                "popd",
                &format!("{}: {err}", new_top.display()),
            );
            return 1;
        }
        assign_pwd(ctx, old_top, new_top);
    }
    print_dirs(ctx.env_ref(), DirsOptions::default(), None, false)
}

fn assign_pwd(ctx: &mut BuiltinCtx<'_>, old: PathBuf, new: PathBuf) {
    let _ = ctx.env().assign("OLDPWD", old.display().to_string());
    let _ = ctx.env().assign("PWD", new.display().to_string());
}
