use std::io::{BufRead, BufReader};
use std::os::unix::io::FromRawFd;

use cherubsh_common::{VarAttrs, VarKind};
use cherubsh_expander::quote::{bytes_to_shell_string, shell_string_to_bytes};

use crate::common::{array_reference, is_valid_name, report_diagnostic};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct Mapfile;
pub static MAPFILE: Mapfile = Mapfile;

pub struct Readarray;
pub static READARRAY: Readarray = Readarray;

impl Builtin for Mapfile {
    fn name(&self) -> &'static str {
        "mapfile"
    }
    fn synopsis(&self) -> &'static str {
        "mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_mapfile(ctx)
    }
}

impl Builtin for Readarray {
    fn name(&self) -> &'static str {
        "readarray"
    }
    fn synopsis(&self) -> &'static str {
        "readarray [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        run_mapfile(ctx)
    }
}

fn run_mapfile(ctx: &mut BuiltinCtx<'_>) -> i32 {
    let mut delim = vec![b'\n'];
    let mut count: Option<usize> = None;
    let mut origin: i64 = 0;
    let mut explicit_origin = false;
    let mut skip: usize = 0;
    let mut trim_delim = false;
    let mut fd: i32 = 0;
    let mut callback: Option<String> = None;
    let mut callback_quantum: usize = 5000;

    let mut parser = OptParser::new(ctx.args, "d:n:O:s:tu:C:c:");
    loop {
        match parser.next() {
            GetOpt::Opt { ch: 'd', arg, .. } => {
                if let Some(s) = arg {
                    delim = delimiter_bytes(&s);
                }
            }
            GetOpt::Opt { ch: 'n', arg, .. } => {
                count = match parse_nonnegative(arg.as_deref()) {
                    Some(n) => Some(n),
                    None => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "mapfile",
                            &format!("{}: invalid line count", arg.unwrap_or_default()),
                        );
                        return 1;
                    }
                };
            }
            GetOpt::Opt { ch: 'O', arg, .. } => {
                origin = match parse_nonnegative(arg.as_deref()) {
                    Some(n) => n as i64,
                    None => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "mapfile",
                            &format!("{}: invalid array origin", arg.unwrap_or_default()),
                        );
                        return 1;
                    }
                };
                explicit_origin = true;
            }
            GetOpt::Opt { ch: 's', arg, .. } => {
                skip = match parse_nonnegative(arg.as_deref()) {
                    Some(n) => n,
                    None => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "mapfile",
                            &format!("{}: invalid line count", arg.unwrap_or_default()),
                        );
                        return 1;
                    }
                };
            }
            GetOpt::Opt { ch: 't', .. } => trim_delim = true,
            GetOpt::Opt { ch: 'u', arg, .. } => {
                fd = match arg.as_deref().and_then(|s| s.parse::<i32>().ok()) {
                    Some(n) if n >= 0 => n,
                    _ => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "mapfile",
                            &format!(
                                "{}: invalid file descriptor specification",
                                arg.unwrap_or_default()
                            ),
                        );
                        return 1;
                    }
                };
            }
            GetOpt::Opt { ch: 'C', arg, .. } => {
                callback = arg;
            }
            GetOpt::Opt { ch: 'c', arg, .. } => {
                callback_quantum = match parse_positive(arg.as_deref()) {
                    Some(n) => n,
                    None => {
                        report_diagnostic(
                            ctx.env_ref(),
                            "mapfile",
                            &format!("{}: invalid callback quantum", arg.unwrap_or_default()),
                        );
                        return 1;
                    }
                };
            }
            GetOpt::Opt { .. } => {}
            GetOpt::End | GetOpt::Done => break,
            GetOpt::Unknown { ch, .. } => {
                report_diagnostic(ctx.env_ref(), "mapfile", &format!("-{ch}: invalid option"));
                return 2;
            }
            GetOpt::Missing { ch, .. } => {
                report_diagnostic(
                    ctx.env_ref(),
                    "mapfile",
                    &format!("-{ch}: option requires an argument"),
                );
                return 2;
            }
        }
    }
    let rest = parser.remaining(ctx.args);
    let requested_array_name = rest.first().map(String::as_str).unwrap_or("MAPFILE");
    if requested_array_name.is_empty() {
        report_diagnostic(ctx.env_ref(), "mapfile", "empty array variable name");
        return 1;
    }
    let resolved_array_name = mapfile_array_target(ctx, requested_array_name);
    let array_name = resolved_array_name
        .as_deref()
        .unwrap_or(requested_array_name);
    if !is_valid_name(array_name) {
        report_diagnostic(
            ctx.env_ref(),
            "mapfile",
            &format!("`{array_name}': not a valid identifier"),
        );
        return 1;
    }
    if ctx.env_ref().is_readonly(array_name) {
        report_diagnostic(
            ctx.env_ref(),
            "mapfile",
            &format!("{array_name}: readonly variable"),
        );
        return 1;
    }
    if matches!(ctx.env_ref().kind(array_name), VarKind::Assoc)
        || ctx.env_ref().attrs(array_name).contains(VarAttrs::ASSOC)
    {
        eprintln!("cherubsh: mapfile: {array_name}: not an indexed array");
        return 1;
    }
    if requested_array_name == array_name
        && ctx
            .env_ref()
            .attrs(requested_array_name)
            .contains(VarAttrs::NAMEREF)
    {
        report_removing_nameref_attribute(ctx, requested_array_name);
    }

    let dup_fd = unsafe { libc::dup(fd) };
    if dup_fd < 0 {
        let err = std::io::Error::last_os_error();
        report_diagnostic(
            ctx.env_ref(),
            "mapfile",
            &format!(
                "{fd}: invalid file descriptor: {}",
                crate::common::errno_message(&err)
            ),
        );
        return 1;
    }

    // Read from fd.
    let file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
    let mut reader = BufReader::new(file);
    let mut idx_seen: usize = 0;
    let mut stored: usize = 0;

    if !explicit_origin {
        ctx.env().set_array(array_name, Vec::new());
    }

    loop {
        if let Some(c) = count {
            if c != 0 && stored >= c {
                break;
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        let n = match read_until_delim(&mut reader, &delim, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                eprintln!("cherubsh: mapfile: {err}");
                return 1;
            }
        };
        let _ = n;
        idx_seen += 1;
        if idx_seen <= skip {
            continue;
        }
        if (trim_delim || delim == [0]) && buf.ends_with(&delim) {
            let new_len = buf.len().saturating_sub(delim.len());
            buf.truncate(new_len);
        }

        let line = bytes_to_shell_string(&buf);
        let array_index = origin + stored as i64;
        stored += 1;
        if let Some(cb) = callback.as_deref() {
            if stored.is_multiple_of(callback_quantum) {
                let command = format!("{cb} {array_index} {}", single_quote(&line));
                let _ = ctx.shell.run_source(&command);
            }
        }
        ctx.env().set_array_indexed(array_name, array_index, line);
    }
    0
}

fn report_removing_nameref_attribute(ctx: &BuiltinCtx<'_>, name: &str) {
    if let (Some(source), Some(line)) = (
        ctx.env_ref().diagnostic_source_name(),
        ctx.env_ref().diagnostic_line(),
    ) {
        eprintln!("{source}: line {line}: warning: {name}: removing nameref attribute");
    } else {
        eprintln!("cherubsh: warning: {name}: removing nameref attribute");
    }
}

fn mapfile_array_target(ctx: &BuiltinCtx<'_>, name: &str) -> Option<String> {
    if !ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        return None;
    }
    let target = ctx.env_ref().resolve_nameref(name)?;
    if array_reference(&target).is_some() {
        return Some(target);
    }
    (!target.is_empty()).then_some(target)
}

fn parse_nonnegative(value: Option<&str>) -> Option<usize> {
    let value = value?;
    if value.starts_with('-') {
        return None;
    }
    value.parse::<usize>().ok()
}

fn parse_positive(value: Option<&str>) -> Option<usize> {
    parse_nonnegative(value).filter(|n| *n > 0)
}

fn delimiter_bytes(value: &str) -> Vec<u8> {
    shell_string_to_bytes(value)
        .first()
        .copied()
        .map(|b| vec![b])
        .unwrap_or_else(|| vec![0])
}

fn read_until_delim<R: BufRead>(
    reader: &mut R,
    delim: &[u8],
    buf: &mut Vec<u8>,
) -> std::io::Result<usize> {
    if delim.len() == 1 {
        return reader.read_until(delim[0], buf);
    }

    let start = buf.len();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte)? {
            0 => break,
            n => {
                buf.extend_from_slice(&byte[..n]);
                if buf.ends_with(delim) {
                    break;
                }
            }
        }
    }
    Ok(buf.len() - start)
}

fn single_quote(value: &str) -> String {
    let mut out = String::from("'");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiter_preserves_non_utf8_shell_byte() {
        let value = bytes_to_shell_string(&[0xff]);
        assert_eq!(delimiter_bytes(&value), vec![0xff]);
    }
}
