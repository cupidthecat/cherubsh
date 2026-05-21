use std::path::PathBuf;

use crate::common::{report_diagnostic, search_path};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Hash;
pub static HASH: Hash = Hash;

impl Builtin for Hash {
    fn name(&self) -> &'static str {
        "hash"
    }
    fn synopsis(&self) -> &'static str {
        "hash [-lr] [-p pathname] [-dt] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut list_form = false;
        let mut clear = false;
        let mut delete = false;
        let mut type_only = false;
        let mut force_path: Option<String> = None;
        let mut parser = OptParser::new(ctx.args, "lrp:dt");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'l', .. } => list_form = true,
                GetOpt::Opt { ch: 'r', .. } => clear = true,
                GetOpt::Opt { ch: 'p', arg, .. } => force_path = arg,
                GetOpt::Opt { ch: 'd', .. } => delete = true,
                GetOpt::Opt { ch: 't', .. } => type_only = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "hash", &format!("-{ch}: invalid option"));
                    eprintln!("hash: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "hash",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("hash: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }

        if clear {
            ctx.env().hash_clear();
            return 0;
        }

        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            if delete {
                report_diagnostic(ctx.env_ref(), "hash", "-d: option requires an argument");
                return 1;
            }
            let mut entries = ctx.env_ref().hash_iter_with_hits();
            entries.sort_by_key(|(name, _, _)| bash_command_bucket(name));
            if entries.is_empty() {
                eprintln!("hash: hash table empty");
                return 0;
            }
            if list_form {
                for (name, path, _) in entries {
                    println!("builtin hash -p {} {}", path.display(), name);
                }
            } else {
                println!("hits\tcommand");
                for (_, path, hits) in entries {
                    println!("{hits:4}\t{}", path.display());
                }
            }
            return 0;
        }

        let mut status = 0;
        if let Some(forced) = force_path {
            if !ctx.env_ref().option("hashall") {
                report_diagnostic(ctx.env_ref(), "hash", "hashing disabled");
                return 1;
            }
            if PathBuf::from(&forced).is_dir() {
                report_diagnostic(ctx.env_ref(), "hash", &format!("{forced}: Is a directory"));
                return 1;
            }
            if ctx.env_ref().option("restricted") {
                if forced.contains('/') {
                    report_diagnostic(ctx.env_ref(), "hash", &format!("{forced}: restricted"));
                    return 1;
                }
                let env_ref: &dyn cherubsh_common::Environment = ctx.env_ref();
                let Some(path) = search_path(&forced, env_ref) else {
                    report_diagnostic(ctx.env_ref(), "hash", &format!("{forced}: not found"));
                    return 1;
                };
                for name in rest {
                    ctx.env().hash_set(name, path.clone());
                }
                return 0;
            }
            for name in rest {
                ctx.env().hash_set(name, PathBuf::from(&forced));
            }
            return 0;
        }
        for name in rest {
            if delete {
                if ctx.env_ref().hash_get(name).is_none() {
                    report_diagnostic(ctx.env_ref(), "hash", &format!("{name}: not found"));
                    status = 1;
                    continue;
                }
                ctx.env().hash_remove(name);
            } else if type_only {
                match ctx.env_ref().hash_get(name) {
                    Some(p) if list_form => println!("builtin hash -p {} {}", p.display(), name),
                    Some(p) if rest.len() > 1 => println!("{name}\t{}", p.display()),
                    Some(p) => println!("{}", p.display()),
                    None => {
                        report_diagnostic(ctx.env_ref(), "hash", &format!("{name}: not found"));
                        status = 1;
                    }
                }
            } else {
                if name.contains('/') {
                    ctx.env().hash_set(name, PathBuf::from(name));
                    continue;
                }
                let env_ref: &dyn cherubsh_common::Environment = ctx.env_ref();
                match search_path(name, env_ref) {
                    Some(path) => ctx.env().hash_set(name, path),
                    None => {
                        ctx.env().hash_remove(name);
                        report_diagnostic(ctx.env_ref(), "hash", &format!("{name}: not found"));
                        status = 1;
                    }
                }
            }
        }
        status
    }
}

fn bash_command_bucket(value: &str) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in value.bytes() {
        let old = hash;
        hash = old
            .wrapping_add(old << 1)
            .wrapping_add(old << 4)
            .wrapping_add(old << 7)
            .wrapping_add(old << 8)
            .wrapping_add(old << 24);
        hash ^= u32::from(byte);
    }
    hash & 255
}
