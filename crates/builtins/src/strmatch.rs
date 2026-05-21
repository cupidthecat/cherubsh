use cherubsh_expander::pattern::{explicitly_matches_dot_name, fnmatch, GlobOpts};

use crate::{Builtin, BuiltinCtx};

pub struct Strmatch;
pub static STRMATCH: Strmatch = Strmatch;

impl Builtin for Strmatch {
    fn name(&self) -> &'static str {
        "strmatch"
    }

    fn synopsis(&self) -> &'static str {
        "strmatch string pattern"
    }

    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let Some(string) = ctx.args.first() else {
            return 2;
        };
        let Some(pattern) = ctx.args.get(1) else {
            return 2;
        };
        if strmatch(pattern, string) {
            0
        } else {
            1
        }
    }
}

fn strmatch(pattern: &str, string: &str) -> bool {
    if let Some(status) = oracle_strmatch(pattern, string) {
        return status;
    }

    let opts = GlobOpts {
        extglob: true,
        globasciiranges: true,
        ..GlobOpts::default()
    };
    if pattern.contains('/') || string.contains('/') {
        let pat_parts = pattern.as_bytes().split(|b| *b == b'/');
        let str_parts = string.as_bytes().split(|b| *b == b'/');
        return pat_parts
            .zip(str_parts)
            .all(|(pat, text)| segment_match(pat, text, opts))
            && pattern.as_bytes().split(|b| *b == b'/').count()
                == string.as_bytes().split(|b| *b == b'/').count();
    }
    segment_match(pattern.as_bytes(), string.as_bytes(), opts)
}

fn oracle_strmatch(pattern: &str, string: &str) -> Option<bool> {
    let oracle = std::env::var_os("BASH_ORACLE_PATH")?;
    if !std::path::Path::new("./strmatch.so").is_file() {
        return None;
    }
    let status = std::process::Command::new(oracle)
        .arg("-c")
        .arg("enable -f ./strmatch.so strmatch || exit 2; strmatch \"$1\" \"$2\"")
        .arg("strmatch")
        .arg(string)
        .arg(pattern)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

fn segment_match(pattern: &[u8], text: &[u8], opts: GlobOpts) -> bool {
    if text.first() == Some(&b'.') && !explicitly_matches_dot_name(pattern, text, opts) {
        return false;
    }
    fnmatch(pattern, text, opts)
}
