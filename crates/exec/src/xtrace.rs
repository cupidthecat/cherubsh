use std::io::Write;

use cherubsh_expander::expand_string_to_string;

use crate::ExecContext;

pub(crate) fn trace(ctx: &mut ExecContext<'_>, line: &str) {
    if !ctx.env.option("xtrace") {
        return;
    }

    let raw_ps4 = ctx.env.get("PS4").unwrap_or_else(|| "+ ".to_string());
    let mut runner = crate::runner::ExecRunner::with_functions(&ctx.functions);
    let mut ps4 = expand_string_to_string(&raw_ps4, ctx.env, &mut runner).unwrap_or(raw_ps4);
    if ctx.env.running_trap().is_some() {
        if let Some(first) = ps4.chars().next() {
            ps4.insert(0, first);
        }
    }
    let text = format!("{ps4}{line}\n");
    write_trace(ctx, text.as_bytes());
}

fn write_trace(ctx: &mut ExecContext<'_>, bytes: &[u8]) {
    if let Some(fd) = ctx
        .env
        .get("BASH_XTRACEFD")
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|fd| *fd >= 0)
    {
        unsafe {
            let _ = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
        }
        return;
    }

    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(bytes);
    let _ = stderr.flush();
}
