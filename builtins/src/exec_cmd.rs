use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx, BuiltinFlags};
use cherubsh_expander::quote::shell_string_to_cstring_bytes;

pub struct Exec;
pub static EXEC: Exec = Exec;

impl Builtin for Exec {
    fn name(&self) -> &'static str {
        "exec"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "exec [-cl] [-a name] [command [argument ...]] [redirection ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        // The simple-command dispatcher already applies redirects + checks for
        // the no-command form before reaching the builtin registry. This
        // entry point covers the case where exec is invoked with arguments
        // (rare from inside builtin dispatch; for parity we replicate the
        // behavior: parse opts, resolve command, execvp).
        let mut clear_env = false;
        let mut login_form = false;
        let mut argv0: Option<String> = None;
        let mut parser = OptParser::new(ctx.args, "cla:");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'c', .. } => clear_env = true,
                GetOpt::Opt { ch: 'l', .. } => login_form = true,
                GetOpt::Opt { ch: 'a', arg, .. } => argv0 = arg,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "exec", &format!("-{ch}: invalid option"));
                    eprintln!("exec: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "exec",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("exec: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args);
        if rest.is_empty() {
            // Redirections-only form is handled by the caller.
            return 0;
        }
        if ctx.env_ref().option("restricted") {
            report_diagnostic(ctx.env_ref(), "exec", "restricted");
            return 1;
        }
        let name = &rest[0];
        let Some(path) = ctx.shell.resolve_command(name) else {
            if name.contains('/') {
                if let (Some(source), Some(line)) = (
                    ctx.env_ref().diagnostic_source_name(),
                    ctx.env_ref().diagnostic_line(),
                ) {
                    eprintln!("{source}: line {line}: {name}: No such file or directory");
                } else {
                    eprintln!("cherubsh: {name}: No such file or directory");
                }
            } else {
                report_diagnostic(ctx.env_ref(), "exec", &format!("{name}: not found"));
            }
            request_exit_after_failed_exec(ctx, 127);
            return 127;
        };
        let mut argv: Vec<String> = Vec::with_capacity(rest.len());
        let first_arg = if login_form {
            let base = argv0.unwrap_or_else(|| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .to_string()
            });
            format!("-{base}")
        } else {
            argv0.unwrap_or_else(|| name.to_string())
        };
        argv.push(first_arg);
        argv.extend_from_slice(&rest[1..]);

        let cstrs: Vec<std::ffi::CString> = argv
            .iter()
            .map(|s| std::ffi::CString::new(shell_string_to_cstring_bytes(s)).unwrap_or_default())
            .collect();
        let mut raw: Vec<*const libc::c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
        raw.push(std::ptr::null());
        if clear_env {
            unsafe {
                let path_cstr = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
                let empty: *const *const libc::c_char = std::ptr::null();
                libc::execve(path_cstr.as_ptr(), raw.as_ptr(), empty);
            }
        } else {
            for (key, value) in ctx.assignments {
                std::env::set_var(key, value);
            }
            unsafe {
                let path_cstr = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
                libc::execv(path_cstr.as_ptr(), raw.as_ptr());
            }
        }
        let err = std::io::Error::last_os_error();
        eprintln!("cherubsh: exec: {}: {err}", path.display());
        request_exit_after_failed_exec(ctx, 127);
        127
    }
}

fn request_exit_after_failed_exec(ctx: &mut BuiltinCtx<'_>, status: i32) {
    if !ctx.env_ref().option("interactive") && !ctx.env_ref().option("execfail") {
        ctx.shell.request_exit(status);
    }
}
