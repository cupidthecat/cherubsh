use crate::common::{report_diagnostic, search_path, search_path_value};
use crate::getopt::{GetOpt, OptParser};
use crate::type_cmd::print_function_definition;
use crate::{is_special, lookup_raw, Builtin, BuiltinCtx};

pub struct Command;
pub static COMMAND: Command = Command;

pub const DEFAULT_PATH: &str = "/usr/local/bin:/usr/GNU/bin:/bin:/usr/bin:.";

impl Builtin for Command {
    fn name(&self) -> &'static str {
        "command"
    }
    fn synopsis(&self) -> &'static str {
        "command [-pVv] command [arg ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut use_default_path = false;
        let mut verbose = false;
        let mut very_verbose = false;
        let mut parser = OptParser::new(ctx.args, "pVv");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'p', .. } => use_default_path = true,
                GetOpt::Opt { ch: 'v', .. } => verbose = true,
                GetOpt::Opt { ch: 'V', .. } => very_verbose = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "command", &format!("-{ch}: invalid option"));
                    eprintln!("command: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "command",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("command: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            return 0;
        }
        let name = &rest[0];

        if use_default_path && ctx.env_ref().option("restricted") {
            report_diagnostic(ctx.env_ref(), "command", "-p: restricted");
            return 1;
        }

        if verbose || very_verbose {
            return describe(ctx, name, verbose, very_verbose, use_default_path);
        }

        // Dispatch - preferring builtin > $PATH, skipping shell functions.
        if let Some(b) = lookup_raw(name) {
            let inner_args = rest[1..].to_vec();
            let start = parser.index + 1;
            let inner_flags = if ctx.arg_flags.len() >= start + inner_args.len() {
                ctx.arg_flags[start..start + inner_args.len()].to_vec()
            } else {
                vec![0; inner_args.len()]
            };
            let mut inner = BuiltinCtx {
                args: &inner_args,
                arg_flags: &inner_flags,
                assignments: ctx.assignments,
                redirects: ctx.redirects,
                invoked_via_command: true,
                shell: ctx.shell,
            };
            return b.run(&mut inner);
        }

        // External: leave the actual exec to caller via search_path; here we
        // print "not found" because `command` without -v can't easily wire
        // into the executor from inside a builtin. The simple.rs dispatch
        // pre-handles `command` by stripping it before reaching this point.
        let env_ref: &dyn cherubsh_common::Environment = ctx.env_ref();
        if use_default_path {
            // Effective $PATH override would happen at the parent dispatch.
        }
        if search_path(name, env_ref).is_some() {
            // Couldn't actually exec - exec happens at parent dispatch level.
            // Return 0 to let the parent handle it; if user reaches here it
            // means we're inside a pipeline child stage already.
            return 0;
        }
        eprintln!("cherubsh: command: {name}: not found");
        127
    }
}

fn describe(
    ctx: &mut BuiltinCtx<'_>,
    name: &str,
    verbose: bool,
    very_verbose: bool,
    use_default_path: bool,
) -> i32 {
    if ctx.env_ref().aliases_enabled() && ctx.env_ref().alias_get(name).is_some() {
        let value = ctx.env_ref().alias_get(name).unwrap_or_default();
        if very_verbose {
            println!("{name} is aliased to `{value}'");
        } else {
            println!("alias {name}='{value}'");
        }
        return 0;
    }
    if ctx.env_ref().option("posix")
        && lookup_raw(name).is_some()
        && ctx.env_ref().builtin_enabled(name)
        && is_special(name)
    {
        if very_verbose {
            println!("{name} is a special shell builtin");
        } else {
            println!("{name}");
        }
        return 0;
    }
    if let Some(function) = ctx.shell.function_get(name) {
        if very_verbose {
            println!("{name} is a function");
            print_function_definition(name, &function);
        } else {
            println!("{name}");
        }
        return 0;
    }
    if is_keyword(name) {
        if very_verbose {
            println!("{name} is a shell keyword");
        } else {
            println!("{name}");
        }
        return 0;
    }
    if lookup_raw(name).is_some() {
        if very_verbose {
            println!("{name} is a shell builtin");
        } else {
            println!("{name}");
        }
        return 0;
    }
    let env_ref: &dyn cherubsh_common::Environment = ctx.env_ref();
    let target_path = if use_default_path {
        search_path_value(name, DEFAULT_PATH)
    } else {
        search_path(name, env_ref)
    };
    match target_path {
        Some(path) => {
            if very_verbose {
                println!("{name} is {}", path.display());
            } else {
                println!("{}", path.display());
            }
            0
        }
        None => {
            if verbose {
                // bash uses exit 1, no stderr message
            } else {
                report_diagnostic(ctx.env_ref(), "command", &format!("{name}: not found"));
            }
            1
        }
    }
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "case"
            | "esac"
            | "for"
            | "select"
            | "while"
            | "until"
            | "do"
            | "done"
            | "in"
            | "function"
            | "time"
            | "{"
            | "}"
            | "!"
            | "[["
            | "]]"
            | "coproc"
    )
}
