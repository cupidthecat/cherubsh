//! `history` builtin.

use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::report_diagnostic;
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};

pub struct History;
pub static HISTORY: History = History;

impl Builtin for History {
    fn name(&self) -> &'static str {
        "history"
    }
    fn synopsis(&self) -> &'static str {
        "history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut clear = false;
        let mut delete: Option<String> = None;
        let mut do_append = false;
        let mut do_read_new = false;
        let mut do_read_all = false;
        let mut do_write = false;
        let mut do_save_arg = false;
        let mut do_expand = false;
        let mut parser = OptParser::new(ctx.args, "cd:anrwps");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'c', .. } => clear = true,
                GetOpt::Opt { ch: 'd', arg, .. } => delete = arg.clone(),
                GetOpt::Opt { ch: 'a', .. } => do_append = true,
                GetOpt::Opt { ch: 'n', .. } => do_read_new = true,
                GetOpt::Opt { ch: 'r', .. } => do_read_all = true,
                GetOpt::Opt { ch: 'w', .. } => do_write = true,
                GetOpt::Opt { ch: 'p', .. } => do_expand = true,
                GetOpt::Opt { ch: 's', .. } => do_save_arg = true,
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "history", &format!("-{ch}: invalid option"));
                    eprintln!("history: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "history",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("history: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }
        let rest = parser.remaining(ctx.args).to_vec();

        if clear {
            if let Some(t) = ctx.env().history_mut() {
                t.clear();
            }
        }
        if let Some(spec) = delete {
            return handle_delete(ctx, &spec);
        }
        if do_save_arg {
            if rest.is_empty() {
                eprintln!("cherubsh: history: -s: missing argument");
                return 2;
            }
            let joined = rest.join(" ");
            let current_added = ctx.env_ref().history_last_line_added();
            let control = ctx.env_ref().histcontrol();
            if let Some(t) = ctx.env().history_mut() {
                if current_added {
                    t.replace_last(&joined, control);
                } else {
                    t.add_forced(&joined, None);
                }
            }
            return 0;
        }
        if do_expand {
            return expand_args(ctx, &rest);
        }
        if do_append || do_read_new || do_read_all || do_write {
            let file_ops = [do_append, do_read_new, do_read_all, do_write]
                .into_iter()
                .filter(|on| *on)
                .count();
            if file_ops > 1 {
                report_diagnostic(
                    ctx.env_ref(),
                    "history",
                    "cannot use more than one of -anrw",
                );
                return 1;
            }
            let filename = rest.into_iter().next();
            return file_op(ctx, do_append, do_read_new, do_read_all, do_write, filename);
        }

        // Plain listing.
        let limit: Option<usize> = ctx.args.first().and_then(|s| s.parse::<usize>().ok());
        let table = match ctx.env_ref().history() {
            Some(t) => t,
            None => return 0,
        };
        let total = table.len();
        let start = limit.map(|n| total.saturating_sub(n)).unwrap_or(0);
        let time_format = ctx.env_ref().get("HISTTIMEFORMAT");
        for (i, entry) in table.iter().enumerate().skip(start) {
            let time = time_format
                .as_deref()
                .map(|fmt| format_history_time(fmt, entry.timestamp))
                .unwrap_or_default();
            println!("{:>5}  {}{}", table.base() + i + 1, time, entry.line);
        }
        0
    }
}

fn format_history_time(format: &str, timestamp: Option<u64>) -> String {
    let ts = timestamp.unwrap_or_else(current_epoch);
    let Ok(fmt) = CString::new(format) else {
        return String::new();
    };
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    let t = ts as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::new();
    }
    let mut buf = vec![0u8; 256];
    let len = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm) };
    if len == 0 {
        return String::new();
    }
    unsafe { CStr::from_ptr(buf.as_ptr().cast()) }
        .to_string_lossy()
        .into_owned()
}

fn current_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn handle_delete(ctx: &mut BuiltinCtx<'_>, spec: &str) -> i32 {
    let Some(parsed) = parse_delete_spec(spec) else {
        report_diagnostic(
            ctx.env_ref(),
            "history",
            &format!("{spec}: history position out of range"),
        );
        return 1;
    };
    let (start, end, start_text, end_text) = parsed;
    let Some(table) = ctx.env_ref().history() else {
        return 1;
    };
    let Some(start_idx) = resolve_history_position(table, start) else {
        report_diagnostic(
            ctx.env_ref(),
            "history",
            &format!("{start_text}: history position out of range"),
        );
        return 1;
    };
    let Some(end_idx) = end
        .map(|n| resolve_history_position(table, n))
        .unwrap_or(Some(start_idx))
    else {
        report_diagnostic(
            ctx.env_ref(),
            "history",
            &format!(
                "{}: history position out of range",
                end_text.as_deref().unwrap_or(spec)
            ),
        );
        return 1;
    };
    if let Some(t) = ctx.env().history_mut() {
        if end.is_some() {
            let (s, e) = if start_idx <= end_idx {
                (start_idx, end_idx)
            } else {
                (end_idx, start_idx)
            };
            t.remove_range(s, e);
        } else {
            t.remove_at(start_idx);
        }
    }
    0
}

fn parse_delete_spec(spec: &str) -> Option<(i64, Option<i64>, String, Option<String>)> {
    let (start, used) = parse_signed_prefix(spec)?;
    let start_text = spec[..used].to_string();
    if used == spec.len() {
        return Some((start, None, start_text, None));
    }
    let rest = &spec[used..];
    let rest = rest.strip_prefix('-')?;
    let (end, end_used) = parse_signed_prefix(rest)?;
    if end_used == rest.len() {
        Some((
            start,
            Some(end),
            start_text,
            Some(rest[..end_used].to_string()),
        ))
    } else {
        None
    }
}

fn parse_signed_prefix(input: &str) -> Option<(i64, usize)> {
    let bytes = input.as_bytes();
    let mut idx = 0;
    if matches!(bytes.first(), Some(b'-')) {
        idx = 1;
    }
    let digit_start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == digit_start {
        return None;
    }
    Some((input[..idx].parse::<i64>().ok()?, idx))
}

fn resolve_history_position(
    table: &cherubsh_common::history::HistoryTable,
    n: i64,
) -> Option<usize> {
    if table.is_empty() || n == 0 {
        return None;
    }
    let first = table.base() + 1;
    let last = table.base() + table.len();
    let idx = if n > 0 {
        n as usize
    } else {
        let idx = last as i64 + 1 + n;
        if idx <= 0 {
            return None;
        }
        idx as usize
    };
    (first..=last).contains(&idx).then_some(idx)
}

fn expand_args(ctx: &mut BuiltinCtx<'_>, args: &[String]) -> i32 {
    // We expand via the same engine the reader_loop uses. Since that lives
    // in crates/shell, we approximate by checking the history table for
    // simple `!!`/`!n`/`!str` lookups.
    let Some(table) = ctx.env_ref().history() else {
        return 1;
    };
    let mut status = 0;
    for arg in args {
        match expand_one(table, arg) {
            Some(s) => println!("{}", s),
            None => {
                if arg.starts_with("!!:") {
                    crate::common::report_diagnostic(
                        ctx.env_ref(),
                        "history",
                        &format!("{arg}: history expansion failed"),
                    );
                } else {
                    crate::common::report_diagnostic(
                        ctx.env_ref(),
                        "history",
                        &format!("{arg}: event not found"),
                    );
                }
                status = 1;
            }
        }
    }
    status
}

fn expand_one(table: &cherubsh_common::history::HistoryTable, s: &str) -> Option<String> {
    if s == "!!" {
        return table.last().map(|e| e.line.clone());
    }
    if let Some(rest) = s.strip_prefix('!') {
        if let Ok(n) = rest.parse::<i64>() {
            if n > 0 {
                return table.get(n as usize).map(|e| e.line.clone());
            }
            if n < 0 {
                return table.nth_last((-n) as usize).map(|e| e.line.clone());
            }
            return None;
        }
        for entry in table.iter().rev() {
            if entry.line.starts_with(rest) {
                return Some(entry.line.clone());
            }
        }
        return None;
    }
    Some(s.to_string())
}

fn file_op(
    ctx: &mut BuiltinCtx<'_>,
    append: bool,
    read_new: bool,
    read_all: bool,
    write: bool,
    filename: Option<String>,
) -> i32 {
    let path: PathBuf = if let Some(filename) = filename {
        PathBuf::from(filename)
    } else if matches!(ctx.env_ref().get("HISTFILE"), Some(value) if value.is_empty()) {
        return 0;
    } else {
        ctx.env_ref()
            .histfile()
            .unwrap_or_else(|| PathBuf::from(".bash_history"))
    };
    let _ = read_new;
    if read_all {
        if let Some(t) = ctx.env().history_mut() {
            if let Err(e) = t.load_from(&path) {
                eprintln!("cherubsh: history: {}: {}", path.display(), e);
                return 1;
            }
        }
    }
    if write {
        if let Some(t) = ctx.env().history_mut() {
            if let Err(e) = t.write_to(&path, usize::MAX, false) {
                eprintln!("cherubsh: history: {}: {}", path.display(), e);
                return 1;
            }
        }
    }
    if append {
        if let Some(t) = ctx.env().history_mut() {
            if let Err(e) = t.append_to(&path, false) {
                eprintln!("cherubsh: history: {}: {}", path.display(), e);
                return 1;
            }
        }
    }
    0
}
