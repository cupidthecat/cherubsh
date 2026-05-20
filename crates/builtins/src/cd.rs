use std::path::{Path, PathBuf};

use crate::common::{diagnostic_subject, errno_message, report_diagnostic};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

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
        let target_arg = rest.first().cloned();
        let print_after;
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
