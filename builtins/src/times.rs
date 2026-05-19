use crate::{Builtin, BuiltinCtx, BuiltinFlags};

pub struct Times;
pub static TIMES: Times = Times;

impl Builtin for Times {
    fn name(&self) -> &'static str {
        "times"
    }
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::SPECIAL | BuiltinFlags::POSIX
    }
    fn synopsis(&self) -> &'static str {
        "times"
    }
    fn run(&self, _ctx: &mut BuiltinCtx<'_>) -> i32 {
        unsafe {
            let mut self_usage: libc::rusage = std::mem::zeroed();
            let mut child_usage: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut self_usage) != 0 {
                return 1;
            }
            if libc::getrusage(libc::RUSAGE_CHILDREN, &mut child_usage) != 0 {
                return 1;
            }
            println!(
                "{}",
                format!(
                    "{}\n{}",
                    format_usage(&self_usage),
                    format_usage(&child_usage)
                )
            );
        }
        0
    }
}

unsafe fn format_usage(u: &libc::rusage) -> String {
    let ut = u.ru_utime.tv_sec as f64 + u.ru_utime.tv_usec as f64 / 1_000_000.0;
    let st = u.ru_stime.tv_sec as f64 + u.ru_stime.tv_usec as f64 / 1_000_000.0;
    format!(
        "{}m{:.3}s {}m{:.3}s",
        (ut as u64) / 60,
        ut - ((ut as u64 / 60) * 60) as f64,
        (st as u64) / 60,
        st - ((st as u64 / 60) * 60) as f64
    )
}
