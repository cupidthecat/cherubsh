use std::ffi::CStr;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_common::Environment;

use crate::options::{DIST_VERSION, PATCH_LEVEL};
use crate::state::ShellState;

const RL_PROMPT_START_IGNORE: u8 = 0x01;
const RL_PROMPT_END_IGNORE: u8 = 0x02;

/// Trait so tests can pin the clock when verifying time-bearing escapes.
pub trait Clock {
    fn now(&self) -> SystemTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// `prompt_again(state, level)` - print PS1 or PS2 (level 1 vs 2) to stderr.
/// Mirrors parse.y:5577.
pub fn prompt_again(state: &ShellState, level: u8) {
    let key = match level {
        2 => "PS2",
        _ => "PS1",
    };
    let raw = state.get(key).unwrap_or_else(|| {
        if level == 2 {
            String::from("> ")
        } else {
            String::from("\\s-\\v\\$ ")
        }
    });
    let decoded = decode_prompt_string(state, &raw);
    use std::io::Write;
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(decoded.as_bytes());
    let _ = stderr.flush();
}

pub fn decode_prompt_string(state: &ShellState, raw: &str) -> String {
    decode_with_clock(state, raw, &SystemClock)
}

pub fn decode_with_clock(state: &ShellState, raw: &str, clock: &dyn Clock) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Posix `!` history-number escape.
        if state.posixly_correct && c == b'!' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'!' {
                out.push('!');
                i += 2;
                continue;
            } else {
                out.push_str(&history_number(state).to_string());
                i += 1;
                continue;
            }
        }
        if c != b'\\' {
            out.push(c as char);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            out.push('\\');
            i += 1;
            continue;
        }
        let esc = bytes[i + 1];
        match esc {
            b'0'..=b'7' => {
                // Octal byte escape \nnn (up to 3 octal digits).
                let mut n = 0u32;
                let mut digits = 0;
                let mut j = i + 1;
                while digits < 3 && j < bytes.len() && (b'0'..=b'7').contains(&bytes[j]) {
                    n = n * 8 + (bytes[j] - b'0') as u32;
                    j += 1;
                    digits += 1;
                }
                if n <= 0xFF {
                    out.push(n as u8 as char);
                }
                i = j;
            }
            b'a' => {
                out.push('\x07');
                i += 2;
            }
            b'e' => {
                out.push('\x1b');
                i += 2;
            }
            b'n' => {
                if state.no_line_editing {
                    out.push('\n');
                } else {
                    out.push('\r');
                    out.push('\n');
                }
                i += 2;
            }
            b'r' => {
                out.push('\r');
                i += 2;
            }
            b'\\' => {
                out.push('\\');
                i += 2;
            }
            b'[' => {
                if !state.no_line_editing {
                    out.push(RL_PROMPT_START_IGNORE as char);
                }
                i += 2;
            }
            b']' => {
                if !state.no_line_editing {
                    out.push(RL_PROMPT_END_IGNORE as char);
                }
                i += 2;
            }
            b's' => {
                out.push_str(&base_name(&state.shell_name));
                i += 2;
            }
            b'v' => {
                out.push_str(DIST_VERSION);
                i += 2;
            }
            b'V' => {
                out.push_str(&format!("{DIST_VERSION}.{PATCH_LEVEL}"));
                i += 2;
            }
            b'u' => {
                out.push_str(&current_user_name());
                i += 2;
            }
            b'h' => {
                let host = current_host_name();
                let short = host.split('.').next().unwrap_or(&host);
                out.push_str(short);
                i += 2;
            }
            b'H' => {
                out.push_str(&current_host_name());
                i += 2;
            }
            b'w' => {
                out.push_str(&render_pwd(state, false));
                i += 2;
            }
            b'W' => {
                out.push_str(&render_pwd(state, true));
                i += 2;
            }
            b'$' => {
                out.push(if euid_is_root() { '#' } else { '$' });
                i += 2;
            }
            b'#' => {
                out.push_str(&state.current_command_number.to_string());
                i += 2;
            }
            b'!' => {
                out.push_str(&history_number(state).to_string());
                i += 2;
            }
            b'j' => {
                let count = state
                    .jobs
                    .list()
                    .iter()
                    .filter(|job| job.state != cherubsh_common::JobState::Done)
                    .count();
                out.push_str(&count.to_string());
                i += 2;
            }
            b'l' => {
                out.push_str(&tty_basename());
                i += 2;
            }
            b'd' => {
                out.push_str(&strftime_now(clock, "%a %b %e"));
                i += 2;
            }
            b't' => {
                out.push_str(&strftime_now(clock, "%H:%M:%S"));
                i += 2;
            }
            b'T' => {
                out.push_str(&strftime_now(clock, "%I:%M:%S"));
                i += 2;
            }
            b'@' => {
                out.push_str(&strftime_now(clock, "%I:%M %p"));
                i += 2;
            }
            b'A' => {
                out.push_str(&strftime_now(clock, "%H:%M"));
                i += 2;
            }
            b'D' => {
                // \D{format}
                if i + 2 < bytes.len() && bytes[i + 2] == b'{' {
                    let start = i + 3;
                    let end = (start..bytes.len())
                        .find(|&j| bytes[j] == b'}')
                        .unwrap_or(bytes.len());
                    let fmt = std::str::from_utf8(&bytes[start..end]).unwrap_or("%X");
                    let fmt = if fmt.is_empty() { "%X" } else { fmt };
                    out.push_str(&strftime_now(clock, fmt));
                    i = if end < bytes.len() { end + 1 } else { end };
                } else {
                    out.push('\\');
                    out.push('D');
                    i += 2;
                }
            }
            other => {
                out.push('\\');
                out.push(other as char);
                i += 2;
            }
        }
    }
    expand_dollar_vars(state, &out)
}

fn expand_dollar_vars(state: &ShellState, raw: &str) -> String {
    // Minimal `$NAME` / `${NAME}` expansion so prompts read PS1='${USER}\$' look right
    // before the full expander lands. Other expansions (command substitution, etc.) wait
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c != b'$' || i + 1 >= bytes.len() {
            out.push(c as char);
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        if next == b'{' {
            if let Some(end) = bytes[i + 2..].iter().position(|b| *b == b'}') {
                let name = std::str::from_utf8(&bytes[i + 2..i + 2 + end]).unwrap_or("");
                if let Some(value) = state.get(name) {
                    out.push_str(&value);
                }
                i = i + 2 + end + 1;
                continue;
            }
        } else if next.is_ascii_alphabetic() || next == b'_' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let name = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("");
            if let Some(value) = state.get(name) {
                out.push_str(&value);
            }
            i = j;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    out
}

fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn current_user_name() -> String {
    if let Some(name) = std::env::var_os("USER") {
        if let Some(s) = name.to_str() {
            return s.to_string();
        }
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() {
            let cstr = CStr::from_ptr((*pw).pw_name);
            if let Ok(s) = cstr.to_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn current_host_name() -> String {
    let mut buf = [0u8; 256];
    let result = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if result != 0 {
        return String::new();
    }
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

fn render_pwd(state: &ShellState, basename_only: bool) -> String {
    let pwd = state
        .get("PWD")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| Some(p.display().to_string()))
        })
        .unwrap_or_else(|| ".".to_string());
    if basename_only {
        let home = state.get("HOME").unwrap_or_default();
        if pwd == "/" {
            return "/".to_string();
        }
        if home == pwd {
            return "~".to_string();
        }
        let base = Path::new(&pwd)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or(pwd.clone());
        return base;
    }
    let home = state.get("HOME").unwrap_or_default();
    if !home.is_empty() && pwd.starts_with(&home) {
        let mut tilde = String::from("~");
        tilde.push_str(&pwd[home.len()..]);
        return tilde;
    }
    pwd
}

fn euid_is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn history_number(state: &ShellState) -> u64 {
    state.history_table.len() as u64 + 1
}

fn tty_basename() -> String {
    unsafe {
        let name = libc::ttyname(0);
        if name.is_null() {
            return String::from("tty");
        }
        let cstr = CStr::from_ptr(name);
        let s = cstr.to_string_lossy();
        return Path::new(s.as_ref())
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| s.to_string());
    }
}

fn strftime_now(clock: &dyn Clock, fmt: &str) -> String {
    let now = clock.now();
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    unsafe {
        let tm = libc::localtime(&secs as *const i64);
        if tm.is_null() {
            return String::new();
        }
        let c_fmt = match std::ffi::CString::new(fmt) {
            Ok(value) => value,
            Err(_) => return String::new(),
        };
        let mut buf = [0u8; 256];
        let written = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            c_fmt.as_ptr(),
            tm,
        );
        if written == 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..written]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            // 2024-06-15 12:00:00 UTC - far from epoch so no timezone wraparound.
            UNIX_EPOCH + std::time::Duration::from_secs(1_718_452_800)
        }
    }

    fn state() -> ShellState {
        let mut s = ShellState::default();
        s.shell_name = "cherubsh".into();
        s
    }

    #[test]
    fn decode_literal_pass_through() {
        let s = state();
        assert_eq!(decode_prompt_string(&s, "hello"), "hello");
    }

    #[test]
    fn decode_dollar_sign_user_vs_root() {
        let s = state();
        let out = decode_prompt_string(&s, "\\$");
        assert!(out == "$" || out == "#");
    }

    #[test]
    fn decode_octal_escape() {
        let s = state();
        assert_eq!(decode_prompt_string(&s, "\\101"), "A");
    }

    #[test]
    fn decode_shell_name() {
        let s = state();
        assert_eq!(decode_prompt_string(&s, "\\s"), "cherubsh");
    }

    #[test]
    fn decode_version() {
        let s = state();
        assert_eq!(decode_prompt_string(&s, "\\v"), DIST_VERSION);
        assert_eq!(decode_prompt_string(&s, "\\V"), "5.2.21");
    }

    #[test]
    fn decode_unknown_escape_preserved() {
        let s = state();
        assert_eq!(decode_prompt_string(&s, "\\Z"), "\\Z");
    }

    #[test]
    fn decode_command_number() {
        let mut s = state();
        s.current_command_number = 42;
        assert_eq!(decode_prompt_string(&s, "\\#"), "42");
    }

    #[test]
    fn decode_history_number_uses_history_table() {
        let mut s = state();
        assert_eq!(decode_prompt_string(&s, "\\!"), "1");
        s.history_table
            .add("echo first", cherubsh_common::HistControl::empty());
        assert_eq!(decode_prompt_string(&s, "\\!"), "2");
    }

    #[test]
    fn decode_job_count_uses_live_jobs() {
        let mut s = state();
        assert_eq!(decode_prompt_string(&s, "\\j"), "0");
        s.jobs.add(
            123,
            123,
            "sleep 10".into(),
            true,
            true,
            vec![cherubsh_common::JobProcess {
                pid: 123,
                status_raw: 0,
                state: cherubsh_common::JobState::Running,
                command: "sleep 10".into(),
            }],
        );
        assert_eq!(decode_prompt_string(&s, "\\j"), "1");
    }

    #[test]
    fn decode_d_with_format() {
        let s = state();
        let out = decode_with_clock(&s, "\\D{%Y}", &FixedClock);
        assert_eq!(out, "2024");
    }

    #[test]
    fn decode_brackets_when_line_editing_enabled() {
        let s = state();
        let out = decode_prompt_string(&s, "\\[\\]");
        assert_eq!(out.as_bytes(), &[0x01, 0x02]);
    }

    #[test]
    fn decode_brackets_dropped_when_no_line_editing() {
        let mut s = state();
        s.no_line_editing = true;
        let out = decode_prompt_string(&s, "\\[X\\]");
        assert_eq!(out, "X");
    }

    #[test]
    fn decode_variable_expansion() {
        let mut s = state();
        s.set("FOO", "bar".into());
        assert_eq!(decode_prompt_string(&s, "$FOO"), "bar");
        assert_eq!(decode_prompt_string(&s, "${FOO}!"), "bar!");
    }
}
