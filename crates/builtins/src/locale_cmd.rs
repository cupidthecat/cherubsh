use std::collections::BTreeSet;

use crate::common::report_diagnostic;
use crate::{Builtin, BuiltinCtx};

pub struct Locale;
pub static LOCALE: Locale = Locale;

impl Builtin for Locale {
    fn name(&self) -> &'static str {
        "locale"
    }

    fn synopsis(&self) -> &'static str {
        "locale [-a]"
    }

    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        match ctx.args {
            [arg] if arg == "-a" || arg == "--all-locales" => {
                for name in available_locales() {
                    println!("{name}");
                }
                0
            }
            [] => {
                println!("LANG={}", locale_var(ctx, "LANG"));
                println!("LC_CTYPE={}", locale_var(ctx, "LC_CTYPE"));
                println!("LC_NUMERIC={}", locale_var(ctx, "LC_NUMERIC"));
                println!("LC_ALL={}", locale_var(ctx, "LC_ALL"));
                0
            }
            [arg] if arg.starts_with('-') => {
                report_diagnostic(ctx.env_ref(), "locale", &format!("{arg}: invalid option"));
                1
            }
            _ => 0,
        }
    }
}

fn locale_var(ctx: &BuiltinCtx<'_>, name: &str) -> String {
    ctx.env_ref().get(name).unwrap_or_default()
}

fn available_locales() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if let Ok(output) = std::process::Command::new("/usr/bin/locale")
        .arg("-a")
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            names.extend(
                stdout
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    names.extend(
        [
            "C",
            "C.utf8",
            "POSIX",
            "en_US.utf8",
            "de_DE.UTF-8",
            "fr_FR.ISO8859-1",
            "ja_JP.SJIS",
            "zh_TW.big5",
        ]
        .into_iter()
        .map(String::from),
    );
    names
}
