use std::collections::VecDeque;
use std::io;
use std::sync::{Mutex, OnceLock};

use crate::key::KeyEvent;

pub fn read_key() -> io::Result<Option<KeyEvent>> {
    read_key_mode(false)
}

pub fn read_key_mode(escape_is_command: bool) -> io::Result<Option<KeyEvent>> {
    let Some(byte) = read_stdin_byte()? else {
        return Ok(None);
    };
    match byte {
        0x1b => read_escape_sequence(escape_is_command),
        0x7f | 0x08 => Ok(Some(KeyEvent::Backspace)),
        b'\r' | b'\n' => Ok(Some(KeyEvent::Enter)),
        b'\t' => Ok(Some(KeyEvent::Tab)),
        0..=0x1f => Ok(Some(KeyEvent::Ctrl(control_name(byte)))),
        0x80..=0xff => decode_utf8_key(byte),
        _ => Ok(Some(KeyEvent::Char(byte as char))),
    }
}

fn read_escape_sequence(escape_is_command: bool) -> io::Result<Option<KeyEvent>> {
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
    if ready <= 0 {
        return Ok(Some(KeyEvent::Esc));
    }
    let Some(second) = read_stdin_byte()? else {
        return Ok(Some(KeyEvent::Esc));
    };
    match second {
        b'[' => {
            let sequence = read_csi_sequence()?;
            Ok(Some(match sequence.as_str() {
                "A" => KeyEvent::Up,
                "B" => KeyEvent::Down,
                "C" => KeyEvent::Right,
                "D" => KeyEvent::Left,
                "H" | "1~" | "7~" => KeyEvent::Home,
                "F" | "4~" | "8~" => KeyEvent::End,
                "2~" => KeyEvent::Insert,
                "3~" => KeyEvent::Delete,
                "5~" => KeyEvent::PageUp,
                "6~" => KeyEvent::PageDown,
                "200~" => KeyEvent::Paste(read_bracketed_paste()?),
                _ => KeyEvent::Raw(format!("\\e[{sequence}")),
            }))
        }
        b'O' => {
            let Some(third) = read_stdin_byte()? else {
                return Ok(Some(KeyEvent::Esc));
            };
            Ok(Some(match third {
                b'H' => KeyEvent::Home,
                b'F' => KeyEvent::End,
                byte @ b'P'..=b'S' => KeyEvent::Function(byte - b'P' + 1),
                byte => KeyEvent::Raw(format!("\\eO{}", byte as char)),
            }))
        }
        byte if escape_is_command => {
            push_stdin_byte(byte);
            Ok(Some(KeyEvent::Esc))
        }
        0x80..=0xff => decode_utf8_char(second).map(|value| value.map(KeyEvent::Meta)),
        0x1b => Ok(Some(KeyEvent::Raw("\\e\\e".to_string()))),
        byte => Ok(Some(KeyEvent::Meta(byte as char))),
    }
}

fn read_csi_sequence() -> io::Result<String> {
    let mut bytes = Vec::new();
    while bytes.len() < 64 {
        let Some(byte) = read_stdin_byte()? else {
            break;
        };
        bytes.push(byte);
        if (0x40..=0x7e).contains(&byte) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_bracketed_paste() -> io::Result<String> {
    const END: &[u8] = b"\x1b[201~";
    let mut bytes = Vec::new();
    loop {
        let Some(byte) = read_stdin_byte()? else {
            break;
        };
        bytes.push(byte);
        if bytes.ends_with(END) {
            bytes.truncate(bytes.len() - END.len());
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn read_literal_char() -> io::Result<Option<char>> {
    let Some(byte) = read_stdin_byte()? else {
        return Ok(None);
    };
    if byte < 0x80 {
        return Ok(Some(byte as char));
    }
    decode_utf8_char(byte)
}

fn decode_utf8_key(first: u8) -> io::Result<Option<KeyEvent>> {
    Ok(decode_utf8_char(first)?.map(KeyEvent::Char))
}

fn decode_utf8_char(first: u8) -> io::Result<Option<char>> {
    let continuation_count = if first & 0b1110_0000 == 0b1100_0000 {
        1
    } else if first & 0b1111_0000 == 0b1110_0000 {
        2
    } else if first & 0b1111_1000 == 0b1111_0000 {
        3
    } else {
        0
    };
    let mut bytes = vec![first];
    for _ in 0..continuation_count {
        let Some(byte) = read_stdin_byte()? else {
            break;
        };
        bytes.push(byte);
    }
    Ok(Some(
        std::str::from_utf8(&bytes)
            .ok()
            .and_then(|value| value.chars().next())
            .unwrap_or('\u{fffd}'),
    ))
}

fn control_name(byte: u8) -> char {
    match byte {
        0 => '@',
        1..=26 => (b'a' + byte - 1) as char,
        27 => '[',
        28 => '\\',
        29 => ']',
        30 => '^',
        _ => '_',
    }
}

fn read_stdin_byte() -> io::Result<Option<u8>> {
    if let Some(byte) = stdin_pushback()
        .lock()
        .expect("input pushback lock")
        .pop_front()
    {
        return Ok(Some(byte));
    }
    let mut byte = [0u8; 1];
    let count = unsafe {
        libc::read(
            libc::STDIN_FILENO,
            byte.as_mut_ptr().cast::<libc::c_void>(),
            1,
        )
    };
    if count == 0 {
        return Ok(None);
    }
    if count < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            return Ok(Some(3));
        }
        return Err(error);
    }
    Ok(Some(byte[0]))
}

fn push_stdin_byte(byte: u8) {
    stdin_pushback()
        .lock()
        .expect("input pushback lock")
        .push_front(byte);
}

fn stdin_pushback() -> &'static Mutex<VecDeque<u8>> {
    static PUSHBACK: OnceLock<Mutex<VecDeque<u8>>> = OnceLock::new();
    PUSHBACK.get_or_init(|| Mutex::new(VecDeque::new()))
}

#[cfg(test)]
mod tests {
    use super::control_name;

    #[test]
    fn control_bytes_have_readline_names() {
        assert_eq!(control_name(0), '@');
        assert_eq!(control_name(1), 'a');
        assert_eq!(control_name(26), 'z');
        assert_eq!(control_name(31), '_');
    }
}
