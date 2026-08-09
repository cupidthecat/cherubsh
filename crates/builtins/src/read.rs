//! `read` builtin. Supports `-r` raw, `-d delim`, `-n N`, `-N N`, `-p prompt`,
//! `-s` silent, `-u fd`, `-t timeout`, `-a array`, `-A assoc`, `-i initial`.

use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;

use crate::common::{
    array_reference, assign_direct_target_with_flags, errno_message, is_valid_name,
    report_builtin_assign_error, report_diagnostic,
};
use crate::getopt::{GetOpt, OptParser};
use crate::{Builtin, BuiltinCtx};
use cherubsh_common::{AssignError, Environment, VarAttrs, VarKind};
use cherubsh_expander::quote::{bytes_to_shell_string, shell_string_to_bytes};

pub struct ReadBuiltin;
pub static READ: ReadBuiltin = ReadBuiltin;

const ESCAPED_CHAR: char = '\u{E000}';

impl Builtin for ReadBuiltin {
    fn name(&self) -> &'static str {
        "read"
    }
    fn synopsis(&self) -> &'static str {
        "read [-Eers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt] [-t timeout] [-u fd] [name ...]"
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32 {
        let mut raw = false;
        let mut delim = b'\n';
        let mut nchars: Option<usize> = None;
        let mut nchars_exact: bool = false;
        let mut prompt: Option<String> = None;
        let mut silent = false;
        let mut edit_mode = false;
        let mut timeout: Option<f64> = None;
        let mut fd: i32 = 0;
        let mut fd_arg: Option<String> = None;
        let mut into_array: Option<String> = None;
        let mut into_assoc: Option<String> = None;
        let mut initial: Option<String> = None;
        let mut parser = OptParser::new(ctx.args, "Eersa:A:d:i:n:N:p:t:u:");
        loop {
            match parser.next() {
                GetOpt::Opt { ch: 'E', .. } => edit_mode = false,
                GetOpt::Opt { ch: 'r', .. } => raw = true,
                GetOpt::Opt { ch: 'e', .. } => edit_mode = true,
                GetOpt::Opt { ch: 's', .. } => silent = true,
                GetOpt::Opt { ch: 'a', arg, .. } => into_array = arg,
                GetOpt::Opt { ch: 'A', arg, .. } => into_assoc = arg,
                GetOpt::Opt { ch: 'd', arg, .. } => {
                    delim = match arg {
                        Some(s) if s.is_empty() => 0,
                        Some(s) => shell_string_to_bytes(&s).first().copied().unwrap_or(0),
                        None => b'\n',
                    };
                }
                GetOpt::Opt { ch: 'i', arg, .. } => initial = arg,
                GetOpt::Opt { ch: 'n', arg, .. } => {
                    let Some(raw) = arg else {
                        report_diagnostic(ctx.env_ref(), "read", "-n: option requires an argument");
                        return 2;
                    };
                    let Ok(value) = raw.parse::<isize>() else {
                        report_diagnostic(ctx.env_ref(), "read", &format!("{raw}: invalid number"));
                        return 1;
                    };
                    if value < 0 {
                        report_diagnostic(ctx.env_ref(), "read", &format!("{raw}: invalid number"));
                        return 1;
                    }
                    nchars = Some(value as usize);
                    nchars_exact = false;
                }
                GetOpt::Opt { ch: 'N', arg, .. } => {
                    let Some(raw) = arg else {
                        report_diagnostic(ctx.env_ref(), "read", "-N: option requires an argument");
                        return 2;
                    };
                    let Ok(value) = raw.parse::<isize>() else {
                        report_diagnostic(ctx.env_ref(), "read", &format!("{raw}: invalid number"));
                        return 1;
                    };
                    if value <= 0 {
                        report_diagnostic(ctx.env_ref(), "read", &format!("{raw}: invalid number"));
                        return 1;
                    }
                    nchars = Some(value as usize);
                    nchars_exact = true;
                }
                GetOpt::Opt { ch: 'p', arg, .. } => prompt = arg,
                GetOpt::Opt { ch: 't', arg, .. } => {
                    let Some(raw) = arg else {
                        report_diagnostic(ctx.env_ref(), "read", "-t: option requires an argument");
                        return 2;
                    };
                    let Ok(value) = raw.parse::<f64>() else {
                        report_diagnostic(
                            ctx.env_ref(),
                            "read",
                            &format!("{raw}: invalid timeout specification"),
                        );
                        return 1;
                    };
                    if value < 0.0 {
                        report_diagnostic(
                            ctx.env_ref(),
                            "read",
                            &format!("{raw}: invalid timeout specification"),
                        );
                        return 1;
                    }
                    timeout = Some(value);
                }
                GetOpt::Opt { ch: 'u', arg, .. } => {
                    let raw = arg.unwrap_or_default();
                    match raw.parse::<i32>() {
                        Ok(n) if n >= 0 => {
                            fd = n;
                            fd_arg = Some(raw);
                        }
                        _ => {
                            report_diagnostic(
                                ctx.env_ref(),
                                "read",
                                &format!("{raw}: invalid file descriptor specification"),
                            );
                            return 1;
                        }
                    }
                }
                GetOpt::Opt { .. } => {}
                GetOpt::End | GetOpt::Done => break,
                GetOpt::Unknown { ch, .. } => {
                    report_diagnostic(ctx.env_ref(), "read", &format!("-{ch}: invalid option"));
                    eprintln!("read: usage: {}", self.synopsis());
                    return 2;
                }
                GetOpt::Missing { ch, .. } => {
                    report_diagnostic(
                        ctx.env_ref(),
                        "read",
                        &format!("-{ch}: option requires an argument"),
                    );
                    eprintln!("read: usage: {}", self.synopsis());
                    return 2;
                }
            }
        }

        let rest_start = parser.index;
        let rest = parser.remaining(ctx.args);
        let names: Vec<String> = rest.to_vec();
        let mut name_flags: Vec<u32> = (0..names.len())
            .map(|i| ctx.arg_flag(rest_start + i))
            .collect();
        let default_name = "REPLY".to_string();
        let using_default_reply = names.is_empty() && into_array.is_none() && into_assoc.is_none();
        let target_names: Vec<String> = if using_default_reply {
            name_flags = vec![0];
            vec![default_name]
        } else {
            names
        };

        let readline_tty = if edit_mode && fd == 0 && stdin_is_nullish_char_device() {
            open_readline_tty()
        } else {
            None
        };
        let active_fd = readline_tty.unwrap_or(fd);

        if let Some(p) = &prompt {
            if unsafe { libc::isatty(active_fd) } == 1 {
                eprint!("{p}");
                let _ = std::io::stderr().flush();
            }
        }

        // Apply timeout via select() on fd.
        if let Some(t) = timeout {
            if t == 0.0 {
                let ready = wait_for_input(active_fd, t).unwrap_or(false);
                close_optional_fd(readline_tty);
                return if ready { 0 } else { 1 };
            }
            if let Some(false) = wait_for_input(active_fd, t) {
                close_optional_fd(readline_tty);
                assign_timeout_empty(
                    ctx,
                    &target_names,
                    into_array.as_deref(),
                    into_assoc.as_deref(),
                );
                return 142;
            }
        }

        let dup_fd = unsafe { libc::dup(active_fd) };
        if dup_fd < 0 {
            close_optional_fd(readline_tty);
            let raw = fd_arg.unwrap_or_else(|| active_fd.to_string());
            let err = std::io::Error::last_os_error();
            report_diagnostic(
                ctx.env_ref(),
                "read",
                &format!("{raw}: invalid file descriptor: {}", errno_message(&err)),
            );
            return 1;
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
        let mut buf: Vec<u8> = Vec::new();
        let mut marked_line = String::new();
        if let Some(init) = initial {
            buf.extend_from_slice(init.as_bytes());
            marked_line.push_str(&init);
        }

        let mut termios_saved: Option<libc::termios> = None;
        if (silent || nchars.is_some()) && unsafe { libc::isatty(active_fd) } == 1 {
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(active_fd, &mut t) == 0 {
                    termios_saved = Some(t);
                    let mut nt = t;
                    if silent {
                        nt.c_lflag &= !libc::ECHO;
                    }
                    if nchars.is_some() {
                        nt.c_lflag &= !libc::ICANON;
                        nt.c_cc[libc::VMIN] = 1;
                        nt.c_cc[libc::VTIME] = 0;
                    }
                    libc::tcsetattr(active_fd, libc::TCSANOW, &nt);
                }
            }
        }

        let mut status: i32;
        let big5_locale = is_big5_locale(ctx.env_ref());
        let utf8_locale = locale_is_utf8(ctx.env_ref());
        if let Some(n) = nchars {
            if n == 0 && !nchars_exact {
                status = 0;
            } else if !nchars_exact && !raw {
                let mut tmp = [0u8; 1];
                while marked_chars(&marked_line).len() < n {
                    match file.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(_) => {
                            if tmp[0] == delim {
                                break;
                            }
                            if tmp[0] == b'\\' {
                                let mut next = [0u8; 1];
                                match file.read(&mut next) {
                                    Ok(0) => break,
                                    Ok(_) => {
                                        if next[0] == delim {
                                            continue;
                                        }
                                        marked_line.push(ESCAPED_CHAR);
                                        push_char_bytes(
                                            &mut marked_line,
                                            &read_locale_char_bytes(
                                                &mut file,
                                                next[0],
                                                utf8_locale,
                                            ),
                                        );
                                    }
                                    Err(_) => break,
                                }
                            } else {
                                push_char_bytes(
                                    &mut marked_line,
                                    &read_locale_char_bytes(&mut file, tmp[0], utf8_locale),
                                );
                            }
                        }
                        Err(_) => break,
                    }
                }
                status = if marked_line.is_empty() { 1 } else { 0 };
            } else {
                let mut tmp = vec![0u8; 1];
                while read_limit_not_reached(&buf, n, ctx.env_ref()) {
                    match file.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(_) => {
                            if !nchars_exact && tmp[0] == delim {
                                break;
                            }
                            buf.push(tmp[0]);
                        }
                        Err(_) => break,
                    }
                }
                status = if buf.is_empty() { 1 } else { 0 };
                marked_line = if raw {
                    String::from_utf8_lossy(&buf).to_string()
                } else {
                    process_backslashes_marked(&String::from_utf8_lossy(&buf))
                };
            }
        } else {
            // Delimited.
            let had_initial = !marked_line.is_empty();
            let mut hit_delim = false;
            let mut tmp = [0u8; 1];
            loop {
                match file.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tmp[0] == delim {
                            hit_delim = true;
                            break;
                        }
                        if !raw && tmp[0] == b'\\' {
                            let mut next = [0u8; 1];
                            match file.read(&mut next) {
                                Ok(0) => {
                                    break;
                                }
                                Ok(_) => {
                                    if next[0] == delim {
                                        hit_delim = true;
                                        continue;
                                    }
                                    marked_line.push(ESCAPED_CHAR);
                                    push_byte_char(&mut marked_line, next[0]);
                                }
                                Err(_) => {
                                    marked_line.push('\\');
                                    break;
                                }
                            }
                        } else if big5_locale && tmp[0] == 0xa3 {
                            let mut next = [0u8; 1];
                            match file.read(&mut next) {
                                Ok(0) => push_byte_char(&mut marked_line, tmp[0]),
                                Ok(_) if next[0] == 0x5c => {
                                    push_byte_char(&mut marked_line, tmp[0]);
                                    push_byte_char(&mut marked_line, next[0]);
                                }
                                Ok(_) => {
                                    push_byte_char(&mut marked_line, tmp[0]);
                                    push_byte_char(&mut marked_line, next[0]);
                                }
                                Err(_) => push_byte_char(&mut marked_line, tmp[0]),
                            }
                        } else {
                            push_byte_char(&mut marked_line, tmp[0]);
                        }
                    }
                    Err(_) => break,
                }
            }
            status = if hit_delim || had_initial { 0 } else { 1 };
        }

        if let Some(t) = termios_saved {
            unsafe { libc::tcsetattr(active_fd, libc::TCSANOW, &t) };
            if silent || (edit_mode && nchars.is_some()) {
                eprintln!();
            }
        }

        let line = marked_line;
        close_optional_fd(readline_tty);

        if let Some(arr) = into_array {
            let arr = read_array_target(ctx, &arr).unwrap_or(arr);
            if !is_valid_name(&arr) {
                report_diagnostic(
                    ctx.env_ref(),
                    "read",
                    &format!("`{arr}': not a valid identifier"),
                );
                return 1;
            }
            if ctx.env_ref().kind(&arr) == VarKind::Assoc {
                report_diagnostic(
                    ctx.env_ref(),
                    "read",
                    &format!("{arr}: not an indexed array"),
                );
                return 1;
            }
            if nchars_exact {
                ctx.env().set_array(&arr, vec![dequote_marked(&line)]);
                return status;
            }
            let ifs = ctx.env_ref().ifs_raw();
            let fields: Vec<String> = split_ifs_marked(&line, &ifs);
            ctx.env().set_array(&arr, fields);
            return status;
        }
        if let Some(arr) = into_assoc {
            let arr = read_array_target(ctx, &arr).unwrap_or(arr);
            if !is_valid_name(&arr) {
                report_diagnostic(
                    ctx.env_ref(),
                    "read",
                    &format!("`{arr}': not a valid identifier"),
                );
                return 1;
            }
            let ifs = ctx.env_ref().ifs_raw();
            let fields: Vec<String> = split_ifs_marked(&line, &ifs);
            let mut it = fields.into_iter();
            while let (Some(k), Some(v)) = (it.next(), it.next()) {
                ctx.env().set_array_assoc(&arr, &k, v);
            }
            return status;
        }

        // Assign to names. When more than one name, IFS-split.
        let ifs = ctx.env_ref().ifs_raw();
        if target_names.len() == 1 {
            let value = if using_default_reply || ifs.is_empty() {
                dequote_marked(&line)
            } else {
                trim_ifs_whitespace_marked(&line, &ifs, true)
            };
            if let Err(err) =
                assign_direct_target_with_flags(ctx.env(), &target_names[0], value, name_flags[0])
            {
                report_builtin_assign_error(ctx.env_ref(), "read", &err);
                status = 1;
            }
        } else {
            let fields = split_read_variables_marked(&line, &ifs, target_names.len());
            for (i, name) in target_names.iter().enumerate() {
                let value = fields.get(i).cloned().unwrap_or_default();
                if let Err(err) = assign_direct_target_with_flags(
                    ctx.env(),
                    name,
                    value,
                    *name_flags.get(i).unwrap_or(&0),
                ) {
                    let readonly = matches!(err, AssignError::ReadOnly(_));
                    report_builtin_assign_error(ctx.env_ref(), "read", &err);
                    status = if readonly { 2 } else { 1 };
                    if readonly {
                        break;
                    }
                }
            }
        }
        let _ = (silent, &prompt);
        status
    }
}

fn read_limit_not_reached(buf: &[u8], n: usize, env: &dyn Environment) -> bool {
    if !locale_is_utf8(env) {
        return buf.len() < n;
    }
    std::str::from_utf8(buf)
        .map(|text| text.chars().count() < n)
        .unwrap_or(true)
}

fn locale_is_utf8(env: &dyn Environment) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|name| env.get(name))
        .find(|value| !value.is_empty())
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("utf-8") || lower.contains("utf8")
        })
        .unwrap_or(false)
}

fn push_byte_char(out: &mut String, byte: u8) {
    out.push_str(&bytes_to_shell_string(&[byte]));
}

fn push_char_bytes(out: &mut String, bytes: &[u8]) {
    out.push_str(&bytes_to_shell_string(bytes));
}

fn read_locale_char_bytes<R: Read>(file: &mut R, first: u8, utf8_locale: bool) -> Vec<u8> {
    let mut bytes = vec![first];
    if !utf8_locale {
        return bytes;
    }
    let width = utf8_sequence_width(first);
    let mut tmp = [0u8; 1];
    while bytes.len() < width {
        match file.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(_) => bytes.push(tmp[0]),
        }
    }
    bytes
}

fn utf8_sequence_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    }
}

fn process_backslashes_marked(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                out.push(ESCAPED_CHAR);
                out.push(next);
                chars.next();
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn marked_chars(s: &str) -> Vec<(char, bool)> {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == ESCAPED_CHAR {
            if let Some(next) = chars.next() {
                out.push((next, true));
            }
        } else {
            out.push((c, false));
        }
    }
    out
}

fn dequote_marked(s: &str) -> String {
    marked_chars(s)
        .into_iter()
        .map(|(c, _)| c)
        .collect::<String>()
}

fn is_ifs_whitespace(c: char, ifs: &str) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b') && ifs.contains(c)
}

fn is_ifs_non_whitespace(c: char, ifs: &str) -> bool {
    ifs.contains(c) && !matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b')
}

fn trim_ifs_whitespace_marked(s: &str, ifs: &str, trim_escaped: bool) -> String {
    let chars = marked_chars(s);
    let mut start = 0usize;
    while start < chars.len() {
        let (c, escaped) = chars[start];
        if (!escaped || trim_escaped) && is_ifs_whitespace(c, ifs) {
            start += 1;
        } else {
            break;
        }
    }

    let mut end = chars.len();
    while end > start {
        let (c, escaped) = chars[end - 1];
        if (!escaped || trim_escaped) && is_ifs_whitespace(c, ifs) {
            end -= 1;
        } else {
            break;
        }
    }

    chars[start..end]
        .iter()
        .map(|(c, _)| *c)
        .collect::<String>()
}

fn dequote_items(items: &[(char, bool)]) -> String {
    items.iter().map(|(c, _)| *c).collect()
}

fn split_read_variables_marked(s: &str, ifs: &str, count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    if let Some(ifs_bytes) = big5_alpha_ifs_bytes(ifs) {
        return split_read_variables_bytes(s, &ifs_bytes, count);
    }
    if count == 1 || ifs.is_empty() {
        let mut out = vec![if ifs.is_empty() {
            dequote_marked(s)
        } else {
            trim_ifs_whitespace_marked(s, ifs, true)
        }];
        out.resize(count, String::new());
        return out;
    }

    let chars = marked_chars(s);
    let mut values = Vec::with_capacity(count);
    let mut pos = 0usize;

    for _ in 0..count - 1 {
        values.push(get_word_from_marked(&chars, &mut pos, ifs).unwrap_or_default());
    }

    let last = if pos < chars.len() {
        let start = pos;
        let mut probe = pos;
        let word = get_word_from_marked(&chars, &mut probe, ifs).unwrap_or_default();
        if probe >= chars.len() {
            word
        } else {
            strip_trailing_ifs_whitespace_items(&chars[start..], ifs)
        }
    } else {
        String::new()
    };
    values.push(last);
    values.resize(count, String::new());
    values
}

fn split_read_variables_bytes(s: &str, ifs: &[u8], count: usize) -> Vec<String> {
    let bytes = shell_string_to_bytes(&dequote_marked(s));
    let mut values = Vec::with_capacity(count);
    let mut start = 0usize;
    for _ in 0..count.saturating_sub(1) {
        if let Some(rel) = find_subslice(&bytes[start..], ifs) {
            values.push(bytes_to_shell_string(&bytes[start..start + rel]));
            start += rel + ifs.len();
        } else {
            values.push(bytes_to_shell_string(&bytes[start..]));
            start = bytes.len();
        }
    }
    values.push(bytes_to_shell_string(&bytes[start..]));
    values.resize(count, String::new());
    values
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn big5_alpha_ifs_bytes(ifs: &str) -> Option<Vec<u8>> {
    let bytes = shell_string_to_bytes(ifs);
    (bytes == [0xa3, 0x5c]).then_some(bytes)
}

fn is_big5_locale(env: &dyn cherubsh_common::Environment) -> bool {
    let locale = env
        .get("LC_ALL")
        .filter(|v| !v.is_empty())
        .or_else(|| env.get("LC_CTYPE").filter(|v| !v.is_empty()))
        .or_else(|| env.get("LANG").filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let lower = locale.to_ascii_lowercase();
    (lower.starts_with("zh_tw.") && lower.contains("big5"))
        || (lower.starts_with("zh_hk.") && lower.contains("big5hkscs"))
}

fn split_ifs_marked(s: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![dequote_marked(s)];
    }

    let chars = marked_chars(s);
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < chars.len() {
        match get_word_from_marked(&chars, &mut pos, ifs) {
            Some(word) => out.push(word),
            None => break,
        }
    }
    out
}

fn get_word_from_marked(chars: &[(char, bool)], pos: &mut usize, ifs: &str) -> Option<String> {
    while *pos < chars.len() {
        let (c, escaped) = chars[*pos];
        if !escaped && is_ifs_whitespace(c, ifs) {
            *pos += 1;
        } else {
            break;
        }
    }
    if *pos >= chars.len() {
        return None;
    }

    let start = *pos;
    while *pos < chars.len() {
        let (c, escaped) = chars[*pos];
        if !escaped && ifs.contains(c) {
            break;
        }
        *pos += 1;
    }
    let word = dequote_items(&chars[start..*pos]);
    let whitesep = if *pos < chars.len() {
        let (c, escaped) = chars[*pos];
        !escaped && is_ifs_whitespace(c, ifs)
    } else {
        false
    };

    if *pos < chars.len() {
        *pos += 1;
    }
    while *pos < chars.len() {
        let (c, escaped) = chars[*pos];
        if !escaped && is_ifs_whitespace(c, ifs) {
            *pos += 1;
        } else {
            break;
        }
    }
    if whitesep && *pos < chars.len() {
        let (c, escaped) = chars[*pos];
        if !escaped && is_ifs_non_whitespace(c, ifs) {
            *pos += 1;
            while *pos < chars.len() {
                let (c, escaped) = chars[*pos];
                if !escaped && is_ifs_whitespace(c, ifs) {
                    *pos += 1;
                } else {
                    break;
                }
            }
        }
    }

    Some(word)
}

fn strip_trailing_ifs_whitespace_items(chars: &[(char, bool)], ifs: &str) -> String {
    let mut end = chars.len();
    while end > 0 {
        let (c, escaped) = chars[end - 1];
        if !escaped && is_ifs_whitespace(c, ifs) {
            end -= 1;
        } else {
            break;
        }
    }
    dequote_items(&chars[..end])
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn split_read_variables_consumes_mixed_ifs_delimiter() {
        assert_eq!(
            split_read_variables_marked("a :b", ": ", 2),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            split_read_variables_marked("a : b", ": ", 2),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            split_read_variables_marked(":::", ": ", 2),
            vec!["".to_string(), "::".to_string()]
        );
    }

    #[test]
    fn split_read_variables_last_name_keeps_intervening_delimiters() {
        assert_eq!(
            split_read_variables_marked("a:b:", ": ", 2),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            split_read_variables_marked("a:b::", ": ", 2),
            vec!["a".to_string(), "b::".to_string()]
        );
        assert_eq!(
            split_read_variables_marked("a::b", ": ", 3),
            vec!["a".to_string(), "".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn split_ifs_marked_keeps_leading_nonws_empty_fields() {
        assert_eq!(
            split_ifs_marked("::", ": "),
            vec!["".to_string(), "".to_string()]
        );
        assert_eq!(
            split_ifs_marked("a :b", ": "),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}

fn assign_timeout_empty(
    ctx: &mut BuiltinCtx<'_>,
    target_names: &[String],
    into_array: Option<&str>,
    into_assoc: Option<&str>,
) {
    if let Some(arr) = into_array {
        ctx.env().set_array(arr, Vec::new());
        return;
    }
    if let Some(arr) = into_assoc {
        ctx.env().set_array(arr, Vec::new());
        return;
    }
    for name in target_names {
        let _ = ctx.env().assign(name, String::new());
    }
}

fn stdin_is_nullish_char_device() -> bool {
    unsafe {
        if libc::isatty(0) == 1 {
            return false;
        }
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(0, &mut st) != 0 {
            return false;
        }
        (st.st_mode & libc::S_IFMT) == libc::S_IFCHR
    }
}

fn open_readline_tty() -> Option<i32> {
    let path = std::ffi::CString::new("/dev/tty").ok()?;
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
    if fd >= 0 {
        Some(fd)
    } else {
        None
    }
}

fn read_array_target(ctx: &BuiltinCtx<'_>, name: &str) -> Option<String> {
    if !ctx.env_ref().attrs(name).contains(VarAttrs::NAMEREF) {
        return None;
    }
    let target = ctx.env_ref().resolve_nameref(name)?;
    if array_reference(&target).is_some() {
        return Some(target);
    }
    (!target.is_empty()).then_some(target)
}

fn close_optional_fd(fd: Option<i32>) {
    if let Some(fd) = fd {
        unsafe {
            libc::close(fd);
        }
    }
}

fn wait_for_input(fd: i32, secs: f64) -> Option<bool> {
    unsafe {
        let mut set: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut set);
        libc::FD_SET(fd, &mut set);
        let mut tv = libc::timeval {
            tv_sec: secs.trunc() as libc::time_t,
            tv_usec: (secs.fract() * 1_000_000.0) as libc::suseconds_t,
        };
        let rc = libc::select(
            fd + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        );
        if rc < 0 {
            return None;
        }
        Some(rc > 0)
    }
}
