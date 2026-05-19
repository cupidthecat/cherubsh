//! Raw-mode keyboard input → `KeyEvent`.

use std::io::{self, Read};

use crate::key::KeyEvent;

/// Read one key event. Blocks until a complete sequence arrives.
pub fn read_key() -> io::Result<Option<KeyEvent>> {
    let mut stdin = io::stdin();
    let mut buf = [0u8; 1];
    if stdin.read(&mut buf)? == 0 {
        return Ok(None);
    }
    let b = buf[0];
    match b {
        0x1b => read_escape_sequence(),
        0x7f | 0x08 => Ok(Some(KeyEvent::Backspace)),
        b'\r' | b'\n' => Ok(Some(KeyEvent::Enter)),
        b'\t' => Ok(Some(KeyEvent::Tab)),
        0..=0x1f => {
            // Ctrl-A..Ctrl-Z, plus a few extras: 0x01..0x1a → 'a'..'z'.
            let letter = (b + b'`') as char;
            Ok(Some(KeyEvent::Ctrl(letter)))
        }
        0x80..=0xff => {
            // Multi-byte UTF-8: read continuation bytes.
            let count = if b & 0b1110_0000 == 0b1100_0000 {
                1
            } else if b & 0b1111_0000 == 0b1110_0000 {
                2
            } else if b & 0b1111_1000 == 0b1111_0000 {
                3
            } else {
                0
            };
            let mut bytes = vec![b];
            for _ in 0..count {
                let mut more = [0u8; 1];
                if stdin.read(&mut more)? == 0 {
                    break;
                }
                bytes.push(more[0]);
            }
            match std::str::from_utf8(&bytes) {
                Ok(s) => {
                    let c = s.chars().next().unwrap_or('\u{FFFD}');
                    Ok(Some(KeyEvent::Char(c)))
                }
                Err(_) => Ok(Some(KeyEvent::Raw(format!("{:?}", bytes)))),
            }
        }
        _ => Ok(Some(KeyEvent::Char(b as char))),
    }
}

fn read_escape_sequence() -> io::Result<Option<KeyEvent>> {
    let mut stdin = io::stdin();
    // Try to read next byte with short timeout via `poll`.
    let mut second = [0u8; 1];
    // Use poll(2) for non-blocking-with-timeout
    let mut pfd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pfd, 1, 50) };
    if rc <= 0 {
        return Ok(Some(KeyEvent::Esc));
    }
    if stdin.read(&mut second)? == 0 {
        return Ok(Some(KeyEvent::Esc));
    }
    match second[0] {
        b'[' => {
            let mut third = [0u8; 1];
            if stdin.read(&mut third)? == 0 {
                return Ok(Some(KeyEvent::Esc));
            }
            match third[0] {
                b'A' => Ok(Some(KeyEvent::Up)),
                b'B' => Ok(Some(KeyEvent::Down)),
                b'C' => Ok(Some(KeyEvent::Right)),
                b'D' => Ok(Some(KeyEvent::Left)),
                b'H' => Ok(Some(KeyEvent::Home)),
                b'F' => Ok(Some(KeyEvent::End)),
                c if c.is_ascii_digit() => {
                    let mut digits = vec![c];
                    loop {
                        let mut nx = [0u8; 1];
                        if stdin.read(&mut nx)? == 0 {
                            break;
                        }
                        if nx[0] == b'~' {
                            break;
                        }
                        digits.push(nx[0]);
                    }
                    let raw = String::from_utf8_lossy(&digits).to_string();
                    Ok(Some(match raw.as_str() {
                        "2" => KeyEvent::Insert,
                        "3" => KeyEvent::Delete,
                        "5" => KeyEvent::PageUp,
                        "6" => KeyEvent::PageDown,
                        _ => KeyEvent::Raw(format!("\\e[{}~", raw)),
                    }))
                }
                c => Ok(Some(KeyEvent::Raw(format!("\\e[{}", c as char)))),
            }
        }
        b'O' => {
            let mut third = [0u8; 1];
            if stdin.read(&mut third)? == 0 {
                return Ok(Some(KeyEvent::Esc));
            }
            match third[0] {
                b'H' => Ok(Some(KeyEvent::Home)),
                b'F' => Ok(Some(KeyEvent::End)),
                c @ b'P'..=b'S' => Ok(Some(KeyEvent::Function(c - b'P' + 1))),
                c => Ok(Some(KeyEvent::Raw(format!("\\eO{}", c as char)))),
            }
        }
        c if c.is_ascii() => Ok(Some(KeyEvent::Meta(c as char))),
        _ => Ok(Some(KeyEvent::Esc)),
    }
}
