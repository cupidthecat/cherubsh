use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx};

pub struct Umask;
pub static UMASK: Umask = Umask;

impl Builtin for Umask {
    fn name(&self) -> &'static str {
        "umask"
    }
    fn synopsis(&self) -> &'static str {
        "umask [-p] [-S] [mode]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut symbolic = false;
        let mut print_form = false;
        let mut index = 0;
        while let Some(arg) = ctx.args.get(index) {
            if arg == "--" {
                index += 1;
                break;
            }
            if arg == "-p" {
                print_form = true;
                index += 1;
                continue;
            }
            if arg == "-S" {
                symbolic = true;
                index += 1;
                continue;
            }
            if arg.starts_with('-')
                && arg.len() > 2
                && arg[1..].chars().all(|ch| matches!(ch, 'p' | 'S'))
            {
                for ch in arg[1..].chars() {
                    match ch {
                        'p' => print_form = true,
                        'S' => symbolic = true,
                        _ => {}
                    }
                }
                index += 1;
                continue;
            }
            if arg.starts_with('-') && arg != "-" {
                let opt = arg.chars().nth(1).unwrap_or('-');
                report_diagnostic(ctx.env_ref(), "umask", &format!("-{opt}: invalid option"));
                eprintln!("umask: usage: {}", self.synopsis());
                return 2;
            }
            break;
        }
        let rest = &ctx.args[index..];

        if rest.is_empty() {
            let mask = ctx.env_ref().umask_get();
            if symbolic {
                if print_form {
                    println!("umask -S {}", format_symbolic(mask));
                } else {
                    println!("{}", format_symbolic(mask));
                }
            } else if print_form {
                println!("umask {:04o}", mask);
            } else {
                println!("{:04o}", mask);
            }
            return 0;
        }

        let arg = &rest[0];
        if let Ok(mask) = u32::from_str_radix(arg, 8) {
            if mask > 0o777 {
                report_diagnostic(
                    ctx.env_ref(),
                    "umask",
                    &format!("{arg}: octal number out of range"),
                );
                return 1;
            }
            ctx.env().umask_set(mask);
            0
        } else if arg.chars().all(|ch| ch.is_ascii_digit()) {
            report_diagnostic(
                ctx.env_ref(),
                "umask",
                &format!("{arg}: octal number out of range"),
            );
            1
        } else {
            let cur = ctx.env_ref().umask_get();
            match apply_symbolic(arg, cur) {
                Ok(new_mask) => {
                    ctx.env().umask_set(new_mask);
                    0
                }
                Err(SymbolicError::Character(ch)) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "umask",
                        &format!("`{ch}': invalid symbolic mode character"),
                    );
                    1
                }
                Err(SymbolicError::Operator(ch)) => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "umask",
                        &format!("`{ch}': invalid symbolic mode operator"),
                    );
                    1
                }
            }
        }
    }
}

fn format_symbolic(mask: u32) -> String {
    // umask's bit set means "deny", so we display the *allowed* permissions.
    let allowed = !mask & 0o777;
    let mut out = String::with_capacity(17);
    let groups = [
        ('u', (allowed >> 6) & 0o7),
        ('g', (allowed >> 3) & 0o7),
        ('o', allowed & 0o7),
    ];
    let mut first = true;
    for (label, bits) in groups {
        if !first {
            out.push(',');
        }
        first = false;
        out.push(label);
        out.push('=');
        if bits & 0o4 != 0 {
            out.push('r');
        }
        if bits & 0o2 != 0 {
            out.push('w');
        }
        if bits & 0o1 != 0 {
            out.push('x');
        }
    }
    out
}

enum SymbolicError {
    Character(char),
    Operator(char),
}

fn apply_symbolic(spec: &str, current: u32) -> Result<u32, SymbolicError> {
    let mut allowed = !current & 0o777;
    for clause in spec.split(',') {
        if clause.is_empty() {
            return Err(SymbolicError::Character('\0'));
        }
        let mut chars = clause.chars().peekable();
        let mut who: u32 = 0;
        loop {
            match chars.peek() {
                Some('u') => {
                    who |= 0o700;
                    chars.next();
                }
                Some('g') => {
                    who |= 0o070;
                    chars.next();
                }
                Some('o') => {
                    who |= 0o007;
                    chars.next();
                }
                Some('a') => {
                    who |= 0o777;
                    chars.next();
                }
                _ => break,
            }
        }
        if who == 0 {
            who = 0o777;
        }
        loop {
            let op = chars.next().ok_or(SymbolicError::Operator('\0'))?;
            if !matches!(op, '+' | '-' | '=') {
                return Err(SymbolicError::Operator(op));
            }
            let mut perm_bits: u32 = 0;
            while let Some(&p) = chars.peek() {
                if matches!(p, '+' | '-' | '=') {
                    break;
                }
                perm_bits |= symbolic_perm_bits(p, allowed, who)?;
                chars.next();
            }
            let mask_bits = perm_bits & who;
            match op {
                '+' => allowed |= mask_bits,
                '-' => allowed &= !mask_bits,
                '=' => {
                    allowed &= !who;
                    allowed |= mask_bits;
                }
                _ => return Err(SymbolicError::Operator(op)),
            }
            if chars.peek().is_none() {
                break;
            }
        }
    }
    Ok(!allowed & 0o777)
}

fn symbolic_perm_bits(ch: char, allowed: u32, who: u32) -> Result<u32, SymbolicError> {
    let bits = match ch {
        'r' => 0o444,
        'w' => 0o222,
        'x' => 0o111,
        'X' => {
            if allowed & 0o111 != 0 {
                0o111
            } else {
                0
            }
        }
        'u' => copy_class_bits((allowed >> 6) & 0o7, who),
        'g' => copy_class_bits((allowed >> 3) & 0o7, who),
        'o' => copy_class_bits(allowed & 0o7, who),
        _ => return Err(SymbolicError::Character(ch)),
    };
    Ok(bits)
}

fn copy_class_bits(class_bits: u32, who: u32) -> u32 {
    let mut bits = 0;
    if who & 0o700 != 0 {
        bits |= class_bits << 6;
    }
    if who & 0o070 != 0 {
        bits |= class_bits << 3;
    }
    if who & 0o007 != 0 {
        bits |= class_bits;
    }
    bits
}
