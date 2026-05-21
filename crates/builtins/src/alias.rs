use crate::common::{is_valid_name, report_diagnostic};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Alias;
pub static ALIAS: Alias = Alias;

impl Builtin for Alias {
    fn name(&self) -> &'static str {
        "alias"
    }
    fn synopsis(&self) -> &'static str {
        "alias [-p] [name[=value] ... ]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut print_all = false;
        let mut parser = OptParser::new(ctx.args, "p");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'p', .. } => print_all = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "alias", &format!("-{ch}: invalid option"));
                    eprintln!("alias: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "alias",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("alias: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);

        if print_all || rest.is_empty() {
            for (name, value) in ctx.env_ref().alias_iter() {
                println!("alias {}={}", name, alias_quote(&value));
            }
            return 0;
        }

        let mut status = 0;
        for arg in rest {
            if let Some((name, value)) = arg.split_once('=') {
                if !is_valid_alias_name(name) {
                    report_diagnostic(
                        ctx.env_ref(),
                        "alias",
                        &format!("`{name}': invalid alias name"),
                    );
                    status = 1;
                    continue;
                }
                ctx.env().alias_set(name, value.to_string());
            } else {
                match ctx.env_ref().alias_get(arg) {
                    Some(value) => println!("alias {}={}", arg, alias_quote(&value)),
                    None => {
                        report_diagnostic(ctx.env_ref(), "alias", &format!("{arg}: not found"));
                        status = 1;
                    }
                }
            }
        }
        status
    }
}

fn is_valid_alias_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // bash permits any chars except metas - keep close to that.
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '!' | '%' | ':' | '@'))
        || is_valid_name(name)
}

fn alias_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
