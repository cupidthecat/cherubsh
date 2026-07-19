use std::path::{Path, PathBuf};

use crate::common::{diagnostic_subject, errno_message, report_assign_error, report_diagnostic};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::AssignError;

pub struct Cd;
pub static CD: Cd = Cd;

impl Builtin for Cd {
    fn name(&self) -> &'static str {
        "cd"
    }
    fn synopsis(&self) -> &'static str {
        "cd [-L|-P [-e]] [-@] [dir]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        if ctx.env_ref().option("restricted") {
            report_diagnostic(ctx.env_ref(), "cd", "restricted");
            return 1;
        }
        let mut physical = false;
        let mut check_after = false;
        let mut treat_as_xattr = false;
        let mut parser = OptParser::new(ctx.args, "LPe@");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'L', .. } => physical = false,
                GetOpt::Opt { ch: 'P', .. } => physical = true,
                GetOpt::Opt { ch: 'e', .. } => check_after = true,
                GetOpt::Opt { ch: '@', .. } => treat_as_xattr = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    eprintln!("cherubsh: cd: -{ch}: invalid option");
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    eprintln!("cherubsh: cd: -{ch}: option requires an argument");
                    return 2;
                }
            }
        }
        let _ = treat_as_xattr;

        let rest = parser.remaining(ctx.args);
        if rest.len() > 1 {
            report_diagnostic(ctx.env_ref(), "cd", "too many arguments");
            return 1;
        }
        let target_arg = rest.first().cloned();
        let mut print_after;
        let mut effective_target;
        match target_arg.as_deref() {
            None => {
                let Some(home) = ctx.env_ref().get("HOME") else {
                    report_diagnostic(ctx.env_ref(), "cd", "HOME not set");
                    return 1;
                };
                effective_target = home;
                print_after = false;
            }
            Some("-") => {
                let Some(prev) = ctx.env_ref().get("OLDPWD") else {
                    report_diagnostic(ctx.env_ref(), "cd", "OLDPWD not set");
                    return 1;
                };
                effective_target = prev;
                print_after = true;
            }
            Some(other) => {
                effective_target = other.to_string();
                print_after = false;
            }
        }

        // CDPATH only applies when target is relative AND doesn't start with ./../etc.
        let cdpath_eligible = !effective_target.starts_with('/')
            && !effective_target.starts_with("./")
            && !effective_target.starts_with("../")
            && effective_target != "."
            && effective_target != "..";
        let mut resolved_via_cdpath = false;
        if cdpath_eligible {
            if let Some(cdpath) = ctx.env_ref().get("CDPATH") {
                for dir in cdpath.split(':') {
                    let base = if dir.is_empty() { "." } else { dir };
                    let candidate = PathBuf::from(base).join(&effective_target);
                    if candidate.is_dir() {
                        effective_target = candidate.to_string_lossy().to_string();
                        resolved_via_cdpath = true;
                        break;
                    }
                }
            }
        }

        if !directory_exists(ctx, &effective_target) {
            let variable_target = target_arg
                .as_deref()
                .filter(|_| ctx.env_ref().option("cdable_vars"))
                .and_then(|name| ctx.env_ref().get(name))
                .filter(|value| directory_exists(ctx, value));
            if let Some(value) = variable_target {
                effective_target = value;
                print_after = true;
            } else if ctx.env_ref().option("interactive") && ctx.env_ref().option("cdspell") {
                if let Some(corrected) = spell_directory(ctx, &effective_target) {
                    effective_target = corrected;
                    print_after = true;
                }
            }
        }

        let target_path = PathBuf::from(&effective_target);
        let absolute = if target_path.is_absolute() {
            target_path
        } else {
            let cur = if physical {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            } else {
                ctx.env_ref()
                    .logical_pwd()
                    .map(PathBuf::from)
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    })
            };
            cur.join(target_path)
        };

        let final_path = if physical {
            match std::fs::canonicalize(&absolute) {
                Ok(p) => p,
                Err(err) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "cd",
                        &format!(
                            "{}: {}",
                            diagnostic_subject(&effective_target),
                            errno_message(&err)
                        ),
                    );
                    return 1;
                }
            }
        } else {
            normalize_logical_path(&absolute)
        };

        let chdir_err = std::env::set_current_dir(&final_path);
        if let Err(err) = chdir_err {
            report_diagnostic(
                ctx.env_ref(),
                "cd",
                &format!(
                    "{}: {}",
                    diagnostic_subject(&effective_target),
                    errno_message(&err)
                ),
            );
            return 1;
        }

        if ctx.env_ref().is_readonly("PWD") {
            report_assign_error(ctx.env_ref(), &AssignError::ReadOnly("PWD".to_string()));
            return 1;
        }
        if ctx.env_ref().is_readonly("OLDPWD") {
            report_assign_error(ctx.env_ref(), &AssignError::ReadOnly("OLDPWD".to_string()));
            return 1;
        }

        let oldpwd = ctx.env_ref().get("PWD").unwrap_or_default();
        let new_pwd = final_path.display().to_string();
        let _ = ctx.env().assign("OLDPWD", oldpwd);
        ctx.env().set_logical_pwd(new_pwd.clone());

        if check_after && physical {
            if let Err(err) = std::fs::canonicalize(&final_path) {
                report_diagnostic(
                    ctx.env_ref(),
                    "cd",
                    &format!("error checking new directory: {}", errno_message(&err)),
                );
                return 1;
            }
        }

        if print_after || resolved_via_cdpath {
            println!("{new_pwd}");
        }
        0
    }
}

/// Logical normalization (bash `-L` default): collapse `.` and `..` lexically
/// without resolving symlinks.
fn normalize_logical_path(p: &Path) -> PathBuf {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for part in p.iter() {
        let s = part.to_str().unwrap_or("");
        match s {
            "" | "." => continue,
            ".." => {
                if parts.last().is_some() {
                    parts.pop();
                }
            }
            _ => parts.push(part),
        }
    }
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    out
}

fn directory_exists(ctx: &BuiltinCtx<'_>, target: &str) -> bool {
    let path = PathBuf::from(target);
    let path = if path.is_absolute() {
        path
    } else {
        ctx.env_ref()
            .logical_pwd()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .join(path)
    };
    path.is_dir()
}

fn spell_directory(ctx: &BuiltinCtx<'_>, target: &str) -> Option<String> {
    let absolute = target.starts_with('/');
    let mut lookup = if absolute {
        PathBuf::from("/")
    } else {
        ctx.env_ref()
            .logical_pwd()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    };
    let mut rendered = PathBuf::new();
    let mut changed = false;

    for component in Path::new(target).components() {
        use std::path::Component;
        let Component::Normal(wanted) = component else {
            if matches!(component, Component::ParentDir) {
                lookup.push("..");
                rendered.push("..");
            } else if matches!(component, Component::CurDir) {
                rendered.push(".");
            }
            continue;
        };
        let wanted = wanted.to_string_lossy();
        let mut best: Option<(u8, String)> = None;
        for entry in std::fs::read_dir(&lookup).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let distance = spelling_distance(&name, &wanted);
            if distance >= 3 {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(current, _)| distance <= *current)
            {
                best = Some((distance, name));
            }
            if distance == 0 {
                break;
            }
        }
        let (_, chosen) = best?;
        changed |= chosen != wanted;
        lookup.push(&chosen);
        rendered.push(chosen);
    }
    if !changed || !lookup.is_dir() {
        return None;
    }
    if absolute {
        Some(Path::new("/").join(rendered).to_string_lossy().into_owned())
    } else {
        Some(rendered.to_string_lossy().into_owned())
    }
}

fn spelling_distance(current: &str, wanted: &str) -> u8 {
    if current == wanted {
        return 0;
    }
    let current = current.as_bytes();
    let wanted = wanted.as_bytes();
    let first = current
        .iter()
        .zip(wanted)
        .position(|(left, right)| left != right)
        .unwrap_or(current.len().min(wanted.len()));
    let left = &current[first..];
    let right = &wanted[first..];
    if left.len() >= 2
        && right.len() >= 2
        && left[0] == right[1]
        && left[1] == right[0]
        && left[2..] == right[2..]
    {
        return 1;
    }
    if !left.is_empty() && !right.is_empty() && left[1..] == right[1..] {
        return 2;
    }
    if !left.is_empty() && left[1..] == *right {
        return 2;
    }
    if !right.is_empty() && *left == right[1..] {
        return 2;
    }
    3
}
