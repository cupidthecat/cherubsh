use std::ffi::CStr;

use cherubsh_expander::quote::shell_string_to_bytes;

use crate::{common, Builtin, BuiltinCtx};

pub struct Echo;
pub static ECHO: Echo = Echo;

impl Builtin for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn synopsis(&self) -> &'static str {
        "echo [-neE] [arg ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let xpg_echo = ctx.env_ref().option("xpg_echo");
        let mut no_newline = false;
        let mut interpret_escapes = xpg_echo;
        let mut idx = 0;
        while idx < ctx.args.len() {
            let arg = &ctx.args[idx];
            if !arg.starts_with('-') || arg.len() < 2 {
                break;
            }
            // Must be a valid -[neE]+ sequence; otherwise it's a literal.
            let valid = arg[1..].chars().all(|c| matches!(c, 'n' | 'e' | 'E'));
            if !valid {
                break;
            }
            for c in arg[1..].chars() {
                match c {
                    'n' => no_newline = true,
                    'e' => interpret_escapes = true,
                    'E' => interpret_escapes = false,
                    _ => unreachable!(),
                }
            }
            idx += 1;
        }
        let mut first = true;
        let mut out = Vec::new();
        let suppress_trailing_newline = no_newline;
        for arg in &ctx.args[idx..] {
            if !first {
                out.push(b' ');
            }
            first = false;
            if interpret_escapes {
                let (bytes, stop) = interpret_echo_escapes(arg);
                out.extend_from_slice(&bytes);
                if stop {
                    if let Err(err) = write_stdout_all(&out) {
                        return report_write_error(ctx, err);
                    }
                    return 0;
                }
            } else {
                out.extend_from_slice(&shell_string_to_bytes(arg));
            }
        }
        if !suppress_trailing_newline {
            out.push(b'\n');
        }
        if let Err(err) = write_stdout_all(&out) {
            return report_write_error(ctx, err);
        }
        0
    }
}

fn write_stdout_all(mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let written = unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                bytes.as_ptr().cast::<libc::c_void>(),
                bytes.len(),
            )
        };
        if written < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if written == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "failed to write whole buffer",
            ));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn report_write_error(ctx: &BuiltinCtx<'_>, err: std::io::Error) -> i32 {
    if err.kind() == std::io::ErrorKind::BrokenPipe {
        return 1;
    }
    common::report_diagnostic(
        ctx.env_ref(),
        "echo",
        &format!("write error: {}", errno_message(&err)),
    );
    1
}

fn errno_message(err: &std::io::Error) -> String {
    let Some(errno) = err.raw_os_error() else {
        return err.to_string();
    };
    let ptr = unsafe { libc::strerror(errno) };
    if ptr.is_null() {
        err.to_string()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Return (bytes, found_c) where found_c indicates `\c` was hit (suppress all
/// further output).
fn interpret_echo_escapes(s: &str) -> (Vec<u8>, bool) {
    let bytes = shell_string_to_bytes(s);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            out.push(b'\\');
            break;
        }
        i += 1;
        match bytes[i] {
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'c' => return (out, true),
            b'e' | b'E' => out.push(0x1b),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            b'\\' => out.push(b'\\'),
            b'0' => {
                // \0NNN - up to 3 octal digits.
                i += 1;
                let mut val: u32 = 0;
                let mut count = 0;
                while count < 3 && i < bytes.len() && (bytes[i] >= b'0' && bytes[i] <= b'7') {
                    val = val * 8 + (bytes[i] - b'0') as u32;
                    i += 1;
                    count += 1;
                }
                out.push((val & 0xff) as u8);
                continue;
            }
            b'x' => {
                i += 1;
                let mut val: u32 = 0;
                let mut count = 0;
                while count < 2 && i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    val = val * 16 + hex_val(bytes[i]) as u32;
                    i += 1;
                    count += 1;
                }
                if count == 0 {
                    out.push(b'\\');
                    out.push(b'x');
                    continue;
                }
                out.push((val & 0xff) as u8);
                continue;
            }
            b'u' | b'U' => {
                let marker = bytes[i];
                let max_digits = if marker == b'u' { 4 } else { 8 };
                i += 1;
                let mut val: u32 = 0;
                let mut count = 0;
                while count < max_digits && i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    val = val * 16 + hex_val(bytes[i]) as u32;
                    i += 1;
                    count += 1;
                }
                if count == 0 {
                    out.push(b'\\');
                    out.push(marker);
                    continue;
                }
                if let Some(ch) = char::from_u32(val) {
                    let mut encoded = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                }
                continue;
            }
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        i += 1;
    }
    (out, false)
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}
