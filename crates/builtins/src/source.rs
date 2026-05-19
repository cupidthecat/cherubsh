use std::path::{Path, PathBuf};

use crate::common::{report_diagnostic, search_path};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Dot;
pub static DOT: Dot = Dot;
pub struct Source;
pub static SOURCE: Source = Source;

impl Builtin for Dot {
    fn name(&self) -> &'static str {
        "."
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        ". filename [arguments]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_source(ctx, ".", self.synopsis())
    }
}

impl Builtin for Source {
    fn name(&self) -> &'static str {
        "source"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "source filename [arguments]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_source(ctx, "source", self.synopsis())
    }
}

fn run_source(ctx: &mut BuiltinCtx<'_>, diagnostic_name: &str, synopsis: &str) -> i32 {
    let Some(filename) = ctx.args.first() else {
        report_diagnostic(ctx.env_ref(), diagnostic_name, "filename argument required");
        eprintln!("{diagnostic_name}: usage: {synopsis}");
        return 2;
    };
    if filename.starts_with('-') && filename != "-" {
        let opt = filename.chars().nth(1).unwrap_or('-');
        report_diagnostic(
            ctx.env_ref(),
            diagnostic_name,
            &format!("-{opt}: invalid option"),
        );
        eprintln!("{diagnostic_name}: usage: {synopsis}");
        return 2;
    }

    if ctx.env_ref().option("restricted") && filename.contains('/') {
        report_diagnostic(
            ctx.env_ref(),
            diagnostic_name,
            &format!("{filename}: restricted"),
        );
        return 1;
    }

    let path = if filename.contains('/') {
        PathBuf::from(filename)
    } else if ctx.env_ref().option("sourcepath") {
        let env_ref: &dyn cherubsh_common::Environment = ctx.env_ref();
        match search_path(filename, env_ref) {
            Some(p) => p,
            None => PathBuf::from(filename),
        }
    } else {
        PathBuf::from(filename)
    };

    if path.is_dir() {
        report_diagnostic(ctx.env_ref(), ".", &format!("{filename}: is a directory"));
        return 1;
    }

    let src = match std::fs::read(&path) {
        Ok(bytes) => {
            if bytes.iter().take(256).any(|byte| *byte == 0) {
                report_diagnostic(
                    ctx.env_ref(),
                    &path.display().to_string(),
                    "cannot execute binary file",
                );
                return 126;
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(err) => {
            let posix_missing_name = ctx.env_ref().option("posix") && !filename.contains('/');
            if posix_missing_name {
                report_diagnostic(
                    ctx.env_ref(),
                    diagnostic_name,
                    &format!("{filename}: file not found"),
                );
            } else {
                let message = if err.kind() == std::io::ErrorKind::NotFound {
                    "No such file or directory".to_string()
                } else {
                    err.to_string()
                };
                report_diagnostic(ctx.env_ref(), &path.display().to_string(), &message);
            }
            if ctx.env_ref().option("posix")
                && !ctx.env_ref().option("interactive")
                && !ctx.invoked_via_command
            {
                ctx.shell.request_exit(1);
            }
            return 1;
        }
    };

    // Save positionals and replace with passed args for the duration of the
    // source.
    let saved = ctx.env_ref().positionals_clone();
    let mut temporary_positionals = None;
    if ctx.args.len() > 1 {
        let mut new_positionals = Vec::with_capacity(ctx.args.len());
        new_positionals.push(saved.first().cloned().unwrap_or_default());
        new_positionals.extend_from_slice(&ctx.args[1..]);
        temporary_positionals = Some(new_positionals.clone());
        ctx.env().set_positionals(new_positionals);
    }

    let source_name = path.display().to_string();
    let status = ctx.shell.run_source_named(&src, &source_name);

    if let Some(temporary) = temporary_positionals {
        if ctx.env_ref().positionals_clone() == temporary {
            ctx.env().set_positionals(saved);
        }
    }
    let _ = Path::new("");
    status
}
