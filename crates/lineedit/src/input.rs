use std::collections::VecDeque;
use std::io;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::key::KeyEvent;

/// Incrementally decodes terminal input bytes into line-editing events.
///
/// `push` keeps incomplete UTF-8, escape, CSI, and bracketed-paste sequences
/// until another chunk arrives. `finish` flushes an incomplete final sequence
/// using the same replacement and EOF behavior as the interactive reader.
#[derive(Debug, Default)]
pub struct InputDecoder {
    pending: Vec<u8>,
    escape_is_command: bool,
    paste_scan_from: usize,
}

impl InputDecoder {
    pub fn new(escape_is_command: bool) -> Self {
        Self {
            pending: Vec::new(),
            escape_is_command,
            paste_scan_from: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<KeyEvent> {
        self.pending.extend_from_slice(bytes);
        self.decode_available(false)
    }

    pub fn finish(&mut self) -> Vec<KeyEvent> {
        self.decode_available(true)
    }

    fn decode_available(&mut self, eof: bool) -> Vec<KeyEvent> {
        let mut events = Vec::new();
        let mut offset = 0;
        loop {
            let undecoded = &self.pending[offset..];
            if !eof && undecoded.starts_with(BRACKETED_PASTE_START) {
                let search_from = self
                    .paste_scan_from
                    .saturating_sub(offset)
                    .max(BRACKETED_PASTE_START.len());
                if find_subslice(&undecoded[search_from..], BRACKETED_PASTE_END).is_none() {
                    self.paste_scan_from = self
                        .pending
                        .len()
                        .saturating_sub(BRACKETED_PASTE_END.len() - 1)
                        .max(offset + BRACKETED_PASTE_START.len());
                    break;
                }
            }
            let Some((consumed, event)) = decode_event(undecoded, self.escape_is_command, eof)
            else {
                break;
            };
            offset += consumed;
            self.paste_scan_from = offset;
            events.push(event);
        }
        if offset > 0 {
            self.pending.drain(..offset);
            self.paste_scan_from = self.paste_scan_from.saturating_sub(offset);
        }
        events
    }
}

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn decode_event(bytes: &[u8], escape_is_command: bool, eof: bool) -> Option<(usize, KeyEvent)> {
    let first = *bytes.first()?;
    if first != 0x1b {
        return decode_plain_event(bytes, eof);
    }
    if bytes.len() == 1 {
        return eof.then_some((1, KeyEvent::Esc));
    }
    if escape_is_command {
        return Some((1, KeyEvent::Esc));
    }

    match bytes[1] {
        b'[' => decode_csi_event(bytes, eof),
        b'O' => {
            if bytes.len() < 3 {
                return eof.then_some((bytes.len(), KeyEvent::Esc));
            }
            let event = match bytes[2] {
                b'H' => KeyEvent::Home,
                b'F' => KeyEvent::End,
                byte @ b'P'..=b'S' => KeyEvent::Function(byte - b'P' + 1),
                byte => KeyEvent::Raw(format!("\\eO{}", byte as char)),
            };
            Some((3, event))
        }
        0x1b => Some((2, KeyEvent::Raw("\\e\\e".to_string()))),
        _ => decode_utf8_at(&bytes[1..], eof)
            .map(|(consumed, character)| (consumed + 1, KeyEvent::Meta(character))),
    }
}

fn decode_plain_event(bytes: &[u8], eof: bool) -> Option<(usize, KeyEvent)> {
    let byte = bytes[0];
    let event = match byte {
        0x7f | 0x08 => KeyEvent::Backspace,
        b'\r' | b'\n' => KeyEvent::Enter,
        b'\t' => KeyEvent::Tab,
        0..=0x1f => KeyEvent::Ctrl(control_name(byte)),
        0x80..=0xff => {
            return decode_utf8_at(bytes, eof)
                .map(|(consumed, character)| (consumed, KeyEvent::Char(character)));
        }
        _ => KeyEvent::Char(byte as char),
    };
    Some((1, event))
}

fn decode_utf8_at(bytes: &[u8], eof: bool) -> Option<(usize, char)> {
    let first = bytes[0];
    let width = if first & 0b1110_0000 == 0b1100_0000 {
        2
    } else if first & 0b1111_0000 == 0b1110_0000 {
        3
    } else if first & 0b1111_1000 == 0b1111_0000 {
        4
    } else {
        1
    };
    if bytes.len() < width && !eof {
        return None;
    }
    let consumed = width.min(bytes.len());
    let character = std::str::from_utf8(&bytes[..consumed])
        .ok()
        .and_then(|value| value.chars().next())
        .unwrap_or('\u{fffd}');
    Some((consumed, character))
}

fn decode_csi_event(bytes: &[u8], eof: bool) -> Option<(usize, KeyEvent)> {
    const MAX_CSI_BYTES: usize = 64;
    let available = bytes.len().saturating_sub(2).min(MAX_CSI_BYTES);
    let final_offset = bytes[2..2 + available]
        .iter()
        .position(|byte| (0x40..=0x7e).contains(byte));
    let Some(final_offset) = final_offset else {
        if !eof && available < MAX_CSI_BYTES {
            return None;
        }
        let sequence_end = 2 + available;
        return Some((
            sequence_end,
            KeyEvent::Raw(format!(
                "\\e[{}",
                String::from_utf8_lossy(&bytes[2..sequence_end])
            )),
        ));
    };
    let sequence_end = final_offset + 3;
    let sequence = String::from_utf8_lossy(&bytes[2..sequence_end]);
    if sequence == "200~" {
        if let Some(end_offset) = find_subslice(&bytes[sequence_end..], BRACKETED_PASTE_END) {
            let paste_end = sequence_end + end_offset;
            return Some((
                paste_end + BRACKETED_PASTE_END.len(),
                KeyEvent::Paste(String::from_utf8_lossy(&bytes[sequence_end..paste_end]).into()),
            ));
        }
        return eof.then(|| {
            (
                bytes.len(),
                KeyEvent::Paste(String::from_utf8_lossy(&bytes[sequence_end..]).into()),
            )
        });
    }
    let event = match sequence.as_ref() {
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
        _ => KeyEvent::Raw(format!("\\e[{sequence}")),
    };
    Some((sequence_end, event))
}

pub fn read_key() -> io::Result<Option<KeyEvent>> {
    read_key_mode(false)
}

pub fn read_key_mode(escape_is_command: bool) -> io::Result<Option<KeyEvent>> {
    let Some(byte) = read_stdin_byte()? else {
        return Ok(None);
    };
    let mut decoder = InputDecoder::new(escape_is_command);
    if let Some(event) = decoder.push(&[byte]).into_iter().next() {
        return Ok(Some(event));
    }

    if byte == 0x1b && !escape_sequence_ready() {
        return Ok(decoder.finish().into_iter().next());
    }
    if byte == 0x1b && escape_is_command {
        if let Some(next_byte) = read_stdin_byte()? {
            push_stdin_byte(next_byte);
        }
        return Ok(decoder.finish().into_iter().next());
    }

    loop {
        let Some(next_byte) = read_stdin_byte()? else {
            return Ok(decoder.finish().into_iter().next());
        };
        if let Some(event) = decoder.push(&[next_byte]).into_iter().next() {
            return Ok(Some(event));
        }
    }
}

fn escape_sequence_ready() -> bool {
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut descriptor, 1, 50) > 0 }
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
    wait_for_stdin()?;
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

fn wait_for_stdin() -> io::Result<()> {
    let deadline = *input_deadline().lock().expect("input deadline lock");
    let Some(deadline) = deadline else {
        return Ok(());
    };
    let now = Instant::now();
    if now >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "readline timed out",
        ));
    }
    let remaining = deadline.saturating_duration_since(now);
    let milliseconds = remaining
        .as_millis()
        .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
        .min(libc::c_int::MAX as u128) as libc::c_int;
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
    if ready == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "readline timed out",
        ));
    }
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn set_input_deadline(deadline: Option<Instant>) {
    *input_deadline().lock().expect("input deadline lock") = deadline;
}

fn input_deadline() -> &'static Mutex<Option<Instant>> {
    static DEADLINE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    DEADLINE.get_or_init(|| Mutex::new(None))
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
    use super::{control_name, InputDecoder};
    use crate::KeyEvent;

    #[test]
    fn control_bytes_have_readline_names() {
        assert_eq!(control_name(0), '@');
        assert_eq!(control_name(1), 'a');
        assert_eq!(control_name(26), 'z');
        assert_eq!(control_name(31), '_');
    }

    #[test]
    fn incremental_decoder_preserves_chunk_boundaries() {
        let mut decoder = InputDecoder::new(false);
        assert!(decoder.push(b"\xc3").is_empty());
        assert_eq!(decoder.push(b"\xa9\x1b["), vec![KeyEvent::Char('é')]);
        assert_eq!(decoder.push(b"A"), vec![KeyEvent::Up]);
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn incremental_decoder_collects_bracketed_paste() {
        let mut decoder = InputDecoder::new(false);
        assert!(decoder.push(b"\x1b[200~hello\n").is_empty());
        assert_eq!(
            decoder.push(b"world\x1b[201~"),
            vec![KeyEvent::Paste("hello\nworld".to_string())]
        );
    }

    #[test]
    fn incremental_decoder_flushes_incomplete_input() {
        let mut decoder = InputDecoder::new(false);
        assert!(decoder.push(b"\x1b").is_empty());
        assert_eq!(decoder.finish(), vec![KeyEvent::Esc]);

        let mut decoder = InputDecoder::new(false);
        assert!(decoder.push(b"\xf0\x9f").is_empty());
        assert_eq!(decoder.finish(), vec![KeyEvent::Char('\u{fffd}')]);
    }

    #[test]
    fn incremental_decoder_bounds_unterminated_csi_sequences() {
        let mut decoder = InputDecoder::new(false);
        let mut sequence = b"\x1b[".to_vec();
        sequence.extend(std::iter::repeat_n(b'1', 64));
        let events = decoder.push(&sequence);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], KeyEvent::Raw(format!("\\e[{}", "1".repeat(64))));
    }

    #[test]
    fn incremental_decoder_handles_large_plain_chunks_linearly() {
        let mut decoder = InputDecoder::new(false);
        let input = vec![b'a'; 64 * 1024];
        let events = decoder.push(&input);
        assert_eq!(events.len(), input.len());
        assert!(events.iter().all(|event| *event == KeyEvent::Char('a')));
    }
}
