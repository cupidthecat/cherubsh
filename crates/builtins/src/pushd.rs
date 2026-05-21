use std::path::PathBuf;

use crate::common::report_diagnostic;
use crate::dirs::{print_dirs, resolve_index, usage, DirsOptions, StackIndex};
use crate::{Builtin, BuiltinCtx};

pub struct Pushd;
pub static PUSHD: Pushd = Pushd;

impl Builtin for Pushd {
    fn name(&self) -> &'static str {
        "pushd"
    }
    fn synopsis(&self) -> &'static str {
        "pushd [-n] [+N | -N | dir]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut skip_cd = false;
        let mut target: Option<String> = None;
        let mut rotate: Option<(String, StackIndex)> = None;
        for arg in ctx.args {
            if arg == "--" {
                break;
            }
            if arg == "-n" {
                skip_cd = true;
                continue;
            }
            if let Some(index) = parse_stack_index(arg) {
                match index {
                    Ok(index) => {
                        rotate = Some((arg.clone(), index));
                        continue;
                    }
                    Err(()) => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "pushd",
                            &format!("{arg}: invalid number"),
                        );
                        usage("pushd", "pushd [-n] [+N | -N | dir]");
                        return 2;
                    }
                }
            }
            target = Some(arg.clone());
        }

        if let Some((arg, index)) = rotate {
            return rotate_stack(ctx, &arg, index, skip_cd);
        }

        match target {
            Some(dir) => push_dir(ctx, &dir, skip_cd),
            None => swap_top(ctx, skip_cd),
        }
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

fn rotate_stack(ctx: &mut BuiltinCtx<'_>, arg: &str, index: StackIndex, skip_cd: bool) -> i32 {
    let stack = ctx.env_ref().dirs_iter();
    let Some(i) = resolve_index(stack.len(), index) else {
        report_diagnostic(
            ctx.env_ref(),
            "pushd",
            &format!("{arg}: directory stack index out of range"),
        );
        return 1;
    };
    let mut rotated = stack;
    rotated.rotate_left(i);
    install_stack(ctx, rotated, skip_cd)
}

fn swap_top(ctx: &mut BuiltinCtx<'_>, skip_cd: bool) -> i32 {
    let stack = ctx.env_ref().dirs_iter();
    if stack.len() < 2 {
        report_diagnostic(ctx.env_ref(), "pushd", "no other directory");
        return 1;
    }
    let mut swapped = stack;
    swapped.swap(0, 1);
    install_stack(ctx, swapped, skip_cd)
}

fn push_dir(ctx: &mut BuiltinCtx<'_>, dir: &str, skip_cd: bool) -> i32 {
    let cur = current_logical_dir(ctx);
    let old_saved = ctx.env_ref().dirs_iter().into_iter().skip(1);
    let target = absolutize(dir);
    if skip_cd {
        let mut saved = ctx.env_ref().dirs_iter();
        if saved.is_empty() {
            saved.push(cur);
        }
        saved.insert(1, target);
        let new_saved = saved.into_iter().skip(1).collect();
        ctx.env().dirs_set_stack(new_saved);
        return print_dirs(ctx.env_ref(), DirsOptions::default(), None, false);
    }
    if let Err(err) = std::env::set_current_dir(&target) {
        report_diagnostic(ctx.env_ref(), "pushd", &format!("{dir}: {err}"));
        return 1;
    }
    let mut new_saved = vec![cur.clone()];
    new_saved.extend(old_saved);
    ctx.env().dirs_set_stack(new_saved);
    assign_pwd(ctx, cur, target);
    print_dirs(ctx.env_ref(), DirsOptions::default(), None, false)
}

fn install_stack(ctx: &mut BuiltinCtx<'_>, stack: Vec<PathBuf>, skip_cd: bool) -> i32 {
    if stack.is_empty() {
        return 1;
    }
    let new_top = stack[0].clone();
    let old_top = current_logical_dir(ctx);
    let saved = stack.into_iter().skip(1).collect();
    ctx.env().dirs_set_stack(saved);
    if !skip_cd {
        if let Err(err) = std::env::set_current_dir(&new_top) {
            report_diagnostic(
                ctx.env_ref(),
                "pushd",
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

fn absolutize(dir: &str) -> PathBuf {
    let target = PathBuf::from(dir);
    if target.is_absolute() {
        target
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(target)
    }
}

fn current_logical_dir(ctx: &BuiltinCtx<'_>) -> PathBuf {
    ctx.env_ref()
        .get("PWD")
        .filter(|pwd| PathBuf::from(pwd).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}
