use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::{iter_builtins, lookup_raw, Builtin, BuiltinCtx};

pub struct Help;
pub static HELP: Help = Help;

impl Builtin for Help {
    fn name(&self) -> &'static str {
        "help"
    }
    fn synopsis(&self) -> &'static str {
        "help [-dms] [pattern ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut short_desc = false;
        let mut man_format = false;
        let mut synopsis_only = false;
        let mut parser = OptParser::new(ctx.args, "dms");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'd', .. } => short_desc = true,
                GetOpt::Opt { ch: 'm', .. } => man_format = true,
                GetOpt::Opt { ch: 's', .. } => synopsis_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "help", &format!("-{ch}: invalid option"));
                    eprintln!("help: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "help",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("help: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            print_bash_help_list();
            return 0;
        }
        let mut status = 0;
        for pattern in rest {
            let matches = help_matches(pattern);
            if matches.is_empty() {
                report_diagnostic(
                    ctx.env_ref(),
                    "help",
                    &format!(
                        "no help topics match `{pattern}'.  Try `help help' or `man -k {pattern}' or `info {pattern}'."
                    ),
                );
                status = 1;
                continue;
            }
            if synopsis_only && pattern.contains('*') {
                println!("Shell commands matching keyword `{pattern}'");
                println!();
            }
            for name in matches {
                if man_format && name == ":" {
                    print_colon_man();
                } else if name == ":" && !synopsis_only && !short_desc {
                    print_colon_long();
                } else if synopsis_only {
                    println!("{name}: {}", help_synopsis(name));
                } else if short_desc {
                    println!("{name} - {}", help_description(name));
                } else if let Some(b) = lookup_raw(name) {
                    println!("{name}: {}", b.synopsis());
                } else {
                    println!("{name}: {}", help_synopsis(name));
                }
            }
        }
        status
    }
}

fn help_matches(pattern: &str) -> Vec<&'static str> {
    if let Some(exact) =
        normalize_help_name(pattern).and_then(|name| lookup_raw(name).map(|_| name))
    {
        return vec![exact];
    }
    if pattern == ":" {
        return vec![":"];
    }
    let prefix = pattern.strip_suffix('*').unwrap_or(pattern);
    let mut names = iter_builtins()
        .filter_map(|b| {
            let name = b.name();
            name.starts_with(prefix).then_some(name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn normalize_help_name(pattern: &str) -> Option<&'static str> {
    match pattern {
        "." => Some("."),
        ":" => Some(":"),
        "[" => Some("["),
        name => lookup_raw(name).map(|b| b.name()),
    }
}

fn help_synopsis(name: &str) -> &'static str {
    match name {
        "." => ". [-p path] filename [arguments]",
        ":" => ":",
        "read" => "read [-Eers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt] [-t timeout] [-u fd] [name ...]",
        "readonly" => "readonly [-aAf] [name[=value] ...] or readonly -p",
        "source" => "source [-p path] filename [arguments]",
        "trap" => "trap [-Plp] [[action] signal_spec ...]",
        "compopt" => "compopt [-o|+o option] [-DEI] [name ...]",
        _ => lookup_raw(name).map(|b| b.synopsis()).unwrap_or(""),
    }
}

fn help_description(name: &str) -> &'static str {
    match name {
        "shift" => "Shift positional parameters.",
        ":" => "Null command.",
        _ => help_synopsis(name),
    }
}

fn print_bash_help_list() {
    println!("GNU bash, version 5.3.15(1)-release (x86_64-pc-linux-gnu)");
    print!("{BASH_HELP_LIST_BODY}");
}

fn print_colon_long() {
    print!(
        ":: :
    Null command.
    
    No effect; the command does nothing.
    
    Exit Status:
    Always succeeds.
"
    );
}

fn print_colon_man() {
    print!(
        "NAME
    : - Null command.

SYNOPSIS
    :

DESCRIPTION
    Null command.
    
    No effect; the command does nothing.
    
    Exit Status:
    Always succeeds.

SEE ALSO
    bash(1)

IMPLEMENTATION
    Copyright (C) 2025 Free Software Foundation, Inc.

"
    );
}

const BASH_HELP_LIST_BODY: &str =
    "These shell commands are defined internally.  Type `help' to see this list.
Type `help name' to find out more about the function `name'.
Use `info bash' to find out more about the shell in general.
Use `man -k' or `info' to find out more about commands not in this list.

A star (*) next to a name means that the command is disabled.

 ! PIPELINE                              history [-c] [-d offset] [n] or hist>
 job_spec [&]                            if COMMANDS; then COMMANDS; [ elif C>
 (( expression ))                        jobs [-lnprs] [jobspec ...] or jobs >
 . [-p path] filename [arguments]        kill [-s sigspec | -n signum | -sigs>
 :                                       let arg [arg ...]
 [ arg... ]                              local [option] name[=value] ...
 [[ expression ]]                        logout [n]
 alias [-p] [name[=value] ... ]          mapfile [-d delim] [-n count] [-O or>
 bg [job_spec ...]                       popd [-n] [+N | -N]
 bind [-lpsvPSVX] [-m keymap] [-f file>  printf [-v var] format [arguments]
 break [n]                               pushd [-n] [+N | -N | dir]
 builtin [shell-builtin [arg ...]]       pwd [-LP]
 caller [expr]                           read [-Eers] [-a array] [-d delim] [>
 case WORD in [PATTERN [| PATTERN]...)>  readarray [-d delim] [-n count] [-O >
 cd [-L|[-P [-e]]] [-@] [dir]            readonly [-aAf] [name[=value] ...] o>
 command [-pVv] command [arg ...]        return [n]
 compgen [-V varname] [-abcdefgjksuv] >  select NAME [in WORDS ... ;] do COMM>
 complete [-abcdefgjksuv] [-pr] [-DEI]>  set [-abefhkmnptuvxBCEHPT] [-o optio>
 compopt [-o|+o option] [-DEI] [name .>  shift [n]
 continue [n]                            shopt [-pqsu] [-o] [optname ...]
 coproc [NAME] command [redirections]    source [-p path] filename [argument>
 declare [-aAfFgiIlnrtux] [name[=value>  suspend [-f]
 dirs [-clpv] [+N] [-N]                  test [expr]
 disown [-h] [-ar] [jobspec ... | pid >  time [-p] pipeline
 echo [-neE] [arg ...]                   times
 enable [-a] [-dnps] [-f filename] [na>  trap [-Plp] [[action] signal_spec ..>
 eval [arg ...]                          true
 exec [-cl] [-a name] [command [argume>  type [-afptP] name [name ...]
 exit [n]                                typeset [-aAfFgiIlnrtux] name[=value>
 export [-fn] [name[=value] ...] or ex>  ulimit [-SHabcdefiklmnpqrstuvxPRT] [>
 false                                   umask [-p] [-S] [mode]
 fc [-e ename] [-lnr] [first] [last] o>  unalias [-a] name [name ...]
 fg [job_spec]                           unset [-f] [-v] [-n] [name ...]
 for NAME [in WORDS ... ] ; do COMMAND>  until COMMANDS; do COMMANDS-2; done
 for (( exp1; exp2; exp3 )); do COMMAN>  variables - Names and meanings of so>
 function name { COMMANDS ; } or name >  wait [-fn] [-p var] [id ...]
 getopts optstring name [arg ...]        while COMMANDS; do COMMANDS-2; done
 hash [-lr] [-p pathname] [-dt] [name >  { COMMANDS ; }
 help [-dms] [pattern ...]
";
