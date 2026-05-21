use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::shopt_table::{lookup, SHOPT_OPTIONS};
use crate::{Builtin, BuiltinCtx};

pub struct Shopt;
pub static SHOPT: Shopt = Shopt;

impl Builtin for Shopt {
    fn name(&self) -> &'static str {
        "shopt"
    }
    fn synopsis(&self) -> &'static str {
        "shopt [-pqsu] [-o] [optname ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut set: Option<bool> = None;
        let mut quiet = false;
        let mut print_form = false;
        let mut set_o_mode = false;
        let mut saw_set = false;
        let mut saw_unset = false;
        let mut parser = OptParser::new(ctx.args, "psuqo");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 's', .. } => {
                    saw_set = true;
                    set = Some(true);
                }
                GetOpt::Opt { ch: 'u', .. } => {
                    saw_unset = true;
                    set = Some(false);
                }
                GetOpt::Opt { ch: 'q', .. } => quiet = true,
                GetOpt::Opt { ch: 'p', .. } => print_form = true,
                GetOpt::Opt { ch: 'o', .. } => set_o_mode = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "shopt", &format!("-{ch}: invalid option"));
                    eprintln!("shopt: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "shopt",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("shopt: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        if saw_set && saw_unset {
            report_diagnostic(
                ctx.env_ref(),
                "shopt",
                "cannot set and unset shell options simultaneously",
            );
            return 1;
        }
        let rest = parser.remaining(ctx.args);

        let opt_list: Vec<&str> = if set_o_mode {
            crate::options::iter_long().map(|o| o.long).collect()
        } else {
            SHOPT_OPTIONS.iter().map(|o| o.name).collect()
        };

        if rest.is_empty() {
            for name in &opt_list {
                let on = ctx.env_ref().option(name);
                if let Some(want) = set {
                    if on != want {
                        continue;
                    }
                }
                if quiet {
                    continue;
                }
                if set_o_mode && print_form {
                    println!("set {}o {}", if on { "-" } else { "+" }, name);
                } else if print_form {
                    println!("shopt {}{}", if on { "-s " } else { "-u " }, name);
                } else if set_o_mode {
                    println!("{:<15}\t{}", name, if on { "on" } else { "off" });
                } else {
                    println!("{:<20}\t{}", name, if on { "on" } else { "off" });
                }
            }
            return 0;
        }

        let mut status = 0;
        for name in rest {
            if !set_o_mode && lookup(name).is_none() {
                report_diagnostic(
                    ctx.env_ref(),
                    "shopt",
                    &format!("{name}: invalid shell option name"),
                );
                status = 1;
                continue;
            }
            if set_o_mode && crate::options::lookup_long(name).is_none() {
                report_diagnostic(
                    ctx.env_ref(),
                    "shopt",
                    &format!("{name}: invalid option name"),
                );
                status = 1;
                continue;
            }
            match set {
                Some(on) => ctx.env().set_option(name, on),
                None => {
                    let on = ctx.env_ref().option(name);
                    if !on {
                        status = 1;
                    }
                    if quiet {
                        continue;
                    } else if set_o_mode && print_form {
                        println!("set {}o {}", if on { "-" } else { "+" }, name);
                    } else if print_form {
                        println!("shopt {}{}", if on { "-s " } else { "-u " }, name);
                    } else if set_o_mode {
                        println!("{:<20}\t{}", name, if on { "on" } else { "off" });
                    } else {
                        println!("{:<20}\t{}", name, if on { "on" } else { "off" });
                    }
                }
            }
        }
        status
    }
}
