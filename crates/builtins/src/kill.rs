use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::signals::SignalMaskGuard;
use cherubsh_common::JobId;

pub struct Kill;
pub static KILL: Kill = Kill;

impl Builtin for Kill {
    fn name(&self) -> &'static str {
        "kill"
    }
    fn synopsis(&self) -> &'static str {
        "kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut signum: i32 = libc::SIGTERM;
        let mut list_only = false;
        let mut idx = 0;
        while idx < ctx.args.len() {
            let arg = &ctx.args[idx];
            if arg == "-l" || arg == "-L" {
                list_only = true;
                idx += 1;
                continue;
            }
            if arg == "-s" {
                idx += 1;
                if idx >= ctx.args.len() {
                    report_diagnostic(ctx.env_ref(), "kill", "-s: option requires an argument");
                    return 2;
                }
                signum = parse_signal(&ctx.args[idx]).unwrap_or(-1);
                if signum < 0 {
                    report_diagnostic(
                        ctx.env_ref(),
                        "kill",
                        &format!("{}: invalid signal specification", ctx.args[idx]),
                    );
                    return 1;
                }
                idx += 1;
                continue;
            }
            if arg == "-n" {
                idx += 1;
                if idx >= ctx.args.len() {
                    report_diagnostic(ctx.env_ref(), "kill", "-n: option requires an argument");
                    return 2;
                }
                signum = match ctx.args[idx].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "kill",
                            &format!("{}: invalid signal specification", ctx.args[idx]),
                        );
                        return 1;
                    }
                };
                idx += 1;
                continue;
            }
            if let Some(stripped) = arg.strip_prefix('-') {
                if let Some(num) = parse_signal(stripped) {
                    signum = num;
                    idx += 1;
                    continue;
                }
                report_diagnostic(
                    ctx.env_ref(),
                    "kill",
                    &format!("{stripped}: invalid signal specification"),
                );
                return 1;
            }
            break;
        }

        let rest = &ctx.args[idx..];
        if list_only {
            if rest.is_empty() {
                for (idx, n) in (1..=31).enumerate() {
                    if let Some(s) = cherubsh_shell_signal_name(n) {
                        print!("{:>2}) SIG{s}", n);
                        if (idx + 1) % 5 == 0 {
                            println!();
                        } else {
                            print!("\t");
                        }
                    }
                }
                println!();
                return 0;
            }
            for arg in rest {
                if arg.eq_ignore_ascii_case("EXIT") {
                    println!("0");
                    continue;
                }
                if let Ok(n) = arg.parse::<i32>() {
                    let sig = if n > 128 { n - 128 } else { n };
                    match cherubsh_shell_signal_name(sig) {
                        Some(name) => println!("{name}"),
                        None => {
                            report_diagnostic(
                                ctx.env_ref(),
                                "kill",
                                &format!("{arg}: invalid signal specification"),
                            );
                            return 1;
                        }
                    }
                } else {
                    match parse_signal(arg) {
                        Some(n) => println!("{n}"),
                        None => {
                            report_diagnostic(
                                ctx.env_ref(),
                                "kill",
                                &format!("{arg}: invalid signal specification"),
                            );
                            return 1;
                        }
                    }
                }
            }
            return 0;
        }
        if rest.is_empty() {
            eprintln!("kill: usage: {}", self.synopsis());
            return 2;
        }
        let mut status = 0;
        for target in rest {
            // Job spec → resolve to negated pgid for `killpg`.
            if target.starts_with('%') {
                let (jid, pgid) = match resolve_jobspec(ctx, target) {
                    Some(job) => job,
                    None => {
                        report_diagnostic(ctx.env_ref(), "kill", &format!("{target}: no such job"));
                        status = 1;
                        continue;
                    }
                };
                let _sigchld_guard = if signum == libc::SIGCONT {
                    let guard = SignalMaskGuard::block_sigchld();
                    if let Some(table) = ctx.env().jobs_table_mut() {
                        let _ = table.reap_all();
                    }
                    Some(guard)
                } else {
                    None
                };
                let result = unsafe { libc::killpg(pgid, signum) };
                if result != 0 {
                    let err = std::io::Error::last_os_error();
                    eprintln!("cherubsh: kill: ({pgid}) - {err}");
                    status = 1;
                } else if signum == libc::SIGCONT {
                    if let Some(table) = ctx.env().jobs_table_mut() {
                        table.mark_job_running(jid);
                    }
                }
                continue;
            }
            let pid: i32 = match target.parse() {
                Ok(n) => n,
                Err(_) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "kill",
                        &format!("`{target}': not a pid or valid job spec"),
                    );
                    status = 1;
                    continue;
                }
            };
            let result = unsafe { libc::kill(pid, signum) };
            if result != 0 {
                let err = std::io::Error::last_os_error();
                eprintln!("cherubsh: kill: ({pid}) - {err}");
                status = 1;
            } else if targets_current_shell(ctx, pid) {
                std::thread::yield_now();
                ctx.shell.run_pending_traps();
            }
        }
        status
    }
}

fn targets_current_shell(ctx: &BuiltinCtx<'_>, pid: i32) -> bool {
    let env = ctx.env_ref();
    pid == env.bashpid() || pid == env.shell_pid()
}

fn resolve_jobspec(ctx: &mut BuiltinCtx<'_>, target: &str) -> Option<(JobId, i32)> {
    let spec = cherubsh_common::jobs::JobSpec::parse(target)?;
    let table = ctx.env_ref().jobs_table()?;
    let id = table.lookup(&spec).ok()?;
    table.get(id).map(|j| (id, j.pgid))
}

fn parse_signal(name: &str) -> Option<i32> {
    if let Ok(n) = name.parse::<i32>() {
        return Some(n);
    }
    let upper = name.to_ascii_uppercase();
    let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
    match stripped {
        "HUP" => Some(1),
        "INT" => Some(2),
        "QUIT" => Some(3),
        "ILL" => Some(4),
        "TRAP" => Some(5),
        "ABRT" | "IOT" => Some(6),
        "BUS" => Some(7),
        "FPE" => Some(8),
        "KILL" => Some(9),
        "USR1" => Some(10),
        "SEGV" => Some(11),
        "USR2" => Some(12),
        "PIPE" => Some(13),
        "ALRM" => Some(14),
        "TERM" => Some(15),
        "CHLD" => Some(17),
        "CONT" => Some(18),
        "STOP" => Some(19),
        "TSTP" => Some(20),
        "TTIN" => Some(21),
        "TTOU" => Some(22),
        "WINCH" => Some(28),
        _ => None,
    }
}

fn cherubsh_shell_signal_name(n: i32) -> Option<&'static str> {
    match n {
        1 => Some("HUP"),
        2 => Some("INT"),
        3 => Some("QUIT"),
        4 => Some("ILL"),
        5 => Some("TRAP"),
        6 => Some("ABRT"),
        7 => Some("BUS"),
        8 => Some("FPE"),
        9 => Some("KILL"),
        10 => Some("USR1"),
        11 => Some("SEGV"),
        12 => Some("USR2"),
        13 => Some("PIPE"),
        14 => Some("ALRM"),
        15 => Some("TERM"),
        16 => Some("STKFLT"),
        17 => Some("CHLD"),
        18 => Some("CONT"),
        19 => Some("STOP"),
        20 => Some("TSTP"),
        21 => Some("TTIN"),
        22 => Some("TTOU"),
        23 => Some("URG"),
        24 => Some("XCPU"),
        25 => Some("XFSZ"),
        26 => Some("VTALRM"),
        27 => Some("PROF"),
        28 => Some("WINCH"),
        29 => Some("IO"),
        30 => Some("PWR"),
        31 => Some("SYS"),
        _ => None,
    }
}
