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

fn segment_match(pattern: &[u8], text: &[u8], opts: GlobOpts) -> bool {
    if text.first() == Some(&b'.') && !explicitly_matches_dot_name(pattern, text, opts) {
        return false;
    }
    fnmatch(pattern, text, opts)
}
