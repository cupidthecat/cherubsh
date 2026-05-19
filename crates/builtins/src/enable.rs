use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::{iter_builtins, lookup_raw, Builtin, BuiltinCtx};

pub struct Enable;
pub static ENABLE: Enable = Enable;

impl Builtin for Enable {
    fn name(&self) -> &'static str {
        "enable"
    }
    fn synopsis(&self) -> &'static str {
        "enable [-a] [-dnps] [-f filename] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut list_all = false;
        let mut disable = false;
        let mut list_only = false;
        let mut posix_only = false;
        let mut load: Option<String> = None;
        let mut parser = OptParser::new(ctx.args, "adnpsf:");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'a', .. } => list_all = true,
                GetOpt::Opt { ch: 'd', .. } => list_all = true, // delete loadable, treat as list for now
                GetOpt::Opt { ch: 'n', .. } => disable = true,
                GetOpt::Opt { ch: 'p', .. } => list_only = true,
                GetOpt::Opt { ch: 's', .. } => posix_only = true,
                GetOpt::Opt { ch: 'f', arg, .. } => load = arg,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "enable", &format!("-{ch}: invalid option"));
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "enable",
                        &format!("-{ch}: option requires an argument"),
                    );
                    return 2;
                }
            }
        }

        if load.is_some() {
            eprintln!("cherubsh: enable: -f: dynamic loading not available");
            return 1;
        }
        let rest = parser.remaining(ctx.args);

        if rest.is_empty() {
            let mut builtins = iter_builtins().collect::<Vec<_>>();
            builtins.sort_by(|a, b| a.name().cmp(b.name()));
            for b in builtins {
                if posix_only && !b.flags().contains(crate::BuiltinFlags::SPECIAL) {
                    continue;
                }
                let on = ctx.env_ref().builtin_enabled(b.name());
                if !list_all {
                    if disable && on {
                        continue;
                    }
                    if !disable && !on {
                        continue;
                    }
                }
                if list_only {
                    println!("enable {}{}", if on { "" } else { "-n " }, b.name());
                } else {
                    println!("enable {}{}", if on { "" } else { "-n " }, b.name());
                }
            }
            return 0;
        }

        let mut status = 0;
        for name in rest {
            if lookup_raw(name).is_none() {
                report_diagnostic(
                    ctx.env_ref(),
                    "enable",
                    &format!("{name}: not a shell builtin"),
                );
                status = 1;
                continue;
            }
            ctx.env().builtin_set_enabled(name, !disable);
        }
        status
    }
}
