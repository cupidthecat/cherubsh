//! `trap` builtin.

use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Trap;
pub static TRAP: Trap = Trap;

const SIGNAL_NAMES: &[(i32, &str)] = &[
    (1, "HUP"),
    (2, "INT"),
    (3, "QUIT"),
    (4, "ILL"),
    (5, "TRAP"),
    (6, "ABRT"),
    (7, "BUS"),
    (8, "FPE"),
    (9, "KILL"),
    (10, "USR1"),
    (11, "SEGV"),
    (12, "USR2"),
    (13, "PIPE"),
    (14, "ALRM"),
    (15, "TERM"),
    (16, "STKFLT"),
    (17, "CHLD"),
    (18, "CONT"),
    (19, "STOP"),
    (20, "TSTP"),
    (21, "TTIN"),
    (22, "TTOU"),
    (23, "URG"),
    (24, "XCPU"),
    (25, "XFSZ"),
    (26, "VTALRM"),
    (27, "PROF"),
    (28, "WINCH"),
    (29, "IO"),
    (30, "PWR"),
    (31, "SYS"),
];

impl Builtin for Trap {
    fn name(&self) -> &'static str {
        "trap"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "trap [-Plp] [[action] signal_spec ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut list_signals = false;
        let mut print_only = false;
        let mut print_plain = false;
        let mut parser = OptParser::new(ctx.args, "Plp");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'P', .. } => print_plain = true,
                GetOpt::Opt { ch: 'l', .. } => list_signals = true,
                GetOpt::Opt { ch: 'p', .. } => print_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "trap", &format!("-{ch}: invalid option"));
                    eprintln!("trap: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "trap",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("trap: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        if list_signals {
            for (idx, (n, name)) in SIGNAL_NAMES.iter().enumerate() {
                print!("{:>2}) SIG{}", n, name);
                if (idx + 1) % 5 == 0 {
                    println!();
                } else {
                    print!("\t");
                }
            }
            if !SIGNAL_NAMES.len().is_multiple_of(5) {
                println!();
            }
            return 0;
        }
        let rest = parser.remaining(ctx.args);
        if print_only && print_plain {
            report_diagnostic(ctx.env_ref(), "trap", "cannot specify both -p and -P");
            return 2;
        }
        if print_plain {
            if rest.is_empty() {
                report_diagnostic(
                    ctx.env_ref(),
                    "trap",
                    "-P requires at least one signal name",
                );
                eprintln!("trap: usage: {}", self.synopsis());
                return 2;
            }
            for sig in rest {
                if !is_signal_spec(sig) {
                    report_diagnostic(
                        ctx.env_ref(),
                        "trap",
                        &format!("{sig}: invalid signal specification"),
                    );
                    return 1;
                }
                if let Some(act) = ctx.env_ref().trap_get(sig) {
                    println!("{act}");
                }
            }
            return 0;
        }
        if print_only {
            if rest.is_empty() {
                print_traps(ctx);
            } else {
                for sig in rest {
                    if !is_signal_spec(sig) {
                        report_diagnostic(
                            ctx.env_ref(),
                            "trap",
                            &format!("{sig}: invalid signal specification"),
                        );
                        return 1;
                    }
                    if let Some(act) = ctx.env_ref().trap_get(sig) {
                        let display = trap_display_name(&canonical_signal_name(sig));
                        println!("trap -- {} {}", trap_action_quote(&act), display);
                    }
                }
            }
            return 0;
        }
        if rest.is_empty() {
            print_traps(ctx);
            return 0;
        }
        let (action, signals): (Option<String>, &[String]) = if rest[0] == "-" {
            (None, &rest[1..])
        } else if is_signal_spec(&rest[0]) {
            // No action - reset to default.
            (None, rest)
        } else {
            (Some(rest[0].clone()), &rest[1..])
        };

        if signals.is_empty() && action.as_deref() != Some("") {
            eprintln!("trap: usage: {}", self.synopsis());
            return 2;
        }

        for sig in signals {
            if !is_signal_spec(sig) {
                report_diagnostic(
                    ctx.env_ref(),
                    "trap",
                    &format!("{sig}: invalid signal specification"),
                );
                return 1;
            }
            ctx.env().trap_set(sig, action.clone());
            // Also update the typed trap-action map used by run_pending_traps.
            if let Some(kind) = parse_kind(sig) {
                let typed = match (&action, sig.is_empty()) {
                    (None, _) => cherubsh_common::signals::TrapAction::Default,
                    (Some(s), _) if s.is_empty() => cherubsh_common::signals::TrapAction::Ignore,
                    (Some(s), _) => cherubsh_common::signals::TrapAction::Command(s.clone()),
                };
                if matches!(kind, cherubsh_common::signals::TrapKind::Debug)
                    && ctx.shell.function_depth() > 0
                {
                    ctx.shell.set_debug_trap_scope_active(!matches!(
                        typed,
                        cherubsh_common::signals::TrapAction::Default
                    ));
                }
                if matches!(typed, cherubsh_common::signals::TrapAction::Default) {
                    ctx.env().trap_clear(kind);
                } else {
                    ctx.env().trap_set_action(kind, typed);
                }
            }
        }
        0
    }
}

fn print_traps(ctx: &BuiltinCtx<'_>) {
    let mut entries = ctx.env_ref().trap_iter();
    entries.sort_by_key(|entry| trap_print_order(&entry.signal));
    for entry in entries {
        println!(
            "trap -- {} {}",
            trap_action_quote(&entry.action),
            trap_display_name(&entry.signal)
        );
    }
}

fn trap_action_quote(action: &str) -> String {
    let mut out = String::from("'");
    for ch in action.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn parse_kind(name: &str) -> Option<cherubsh_common::signals::TrapKind> {
    use cherubsh_common::signals::TrapKind;
    let upper = name.to_ascii_uppercase();
    let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
    match stripped {
        "EXIT" | "0" => Some(TrapKind::Exit),
        "ERR" => Some(TrapKind::Err),
        "DEBUG" => Some(TrapKind::Debug),
        "RETURN" => Some(TrapKind::Return),
        other => {
            if let Ok(n) = other.parse::<i32>() {
                Some(TrapKind::Numeric(n))
            } else {
                // Map short name → number using the SIGNAL_NAMES table.
                for (n, short) in SIGNAL_NAMES {
                    if other == *short {
                        return Some(TrapKind::Numeric(*n));
                    }
                }
                None
            }
        }
    }
}

fn is_signal_spec(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
    matches!(stripped, "EXIT" | "ERR" | "RETURN" | "DEBUG")
        || stripped
            .parse::<i32>()
            .ok()
            .is_some_and(valid_signal_number)
        || SIGNAL_NAMES.iter().any(|(_, n)| *n == stripped)
}

fn valid_signal_number(n: i32) -> bool {
    n == 0 || SIGNAL_NAMES.iter().any(|(num, _)| *num == n)
}

fn canonical_signal_name(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
    match stripped {
        "0" | "EXIT" => return "EXIT".to_string(),
        "ERR" | "RETURN" | "DEBUG" => return stripped.to_string(),
        _ => {}
    }
    if let Ok(n) = stripped.parse::<i32>() {
        if let Some((_, short)) = SIGNAL_NAMES.iter().find(|(num, _)| *num == n) {
            return (*short).to_string();
        }
    }
    stripped.to_string()
}

fn trap_display_name(signal: &str) -> String {
    match signal {
        "EXIT" | "ERR" | "RETURN" | "DEBUG" => signal.to_string(),
        _ => format!("SIG{signal}"),
    }
}

fn trap_print_order(signal: &str) -> (u8, i32, String) {
    match signal {
        "EXIT" => (0, 0, String::new()),
        "DEBUG" => (2, 0, String::new()),
        "ERR" => (2, 1, String::new()),
        "RETURN" => (2, 2, String::new()),
        _ => {
            let num = trap_print_signal_rank(signal);
            (1, num, signal.to_string())
        }
    }
}

fn trap_print_signal_rank(signal: &str) -> i32 {
    match signal {
        // The upstream bash-5.2.21 expected files were generated with USR1/USR2
        // after TERM; keep trap printing stable across host libc numbering.
        "USR1" => 30,
        "USR2" => 31,
        _ => SIGNAL_NAMES
            .iter()
            .find(|(_, short)| *short == signal)
            .map(|(num, _)| *num)
            .unwrap_or(i32::MAX),
    }
}
