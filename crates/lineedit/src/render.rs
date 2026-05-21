//! Prompt + buffer rendering with minimal-diff updates.

use std::io::{self, Write};

use crate::buffer::EditBuffer;

pub struct Renderer {
    prompt: String,
    pub prompt_cols: u16,
    last_rows: u16,
    last_cursor_row: u16,
    last_cursor_col: u16,
    silent: bool,
}

impl Renderer {
    pub fn new(prompt: &str) -> Self {
        Self::with_mode(prompt, false)
    }

    pub fn silent(prompt: &str) -> Self {
        Self::with_mode(prompt, true)
    }

    fn with_mode(prompt: &str, silent: bool) -> Self {
        let visible: String = strip_csi(prompt);
        let cols = visible.chars().count() as u16;
        Self {
            prompt: prompt.to_string(),
            prompt_cols: cols,
            last_rows: 0,
            last_cursor_row: 0,
            last_cursor_col: 0,
            silent,
        }
    }

    pub fn full_redraw(&mut self, buf: &EditBuffer) -> io::Result<()> {
        if self.silent {
            return Ok(());
        }
        let term_cols = term_columns().max(1);
        let mut stderr = io::stderr().lock();
        // Move cursor to start: up by last_cursor_row, then \r.
        if self.last_cursor_row > 0 {
            write!(stderr, "\x1b[{}A", self.last_cursor_row)?;
        }
        write!(stderr, "\r\x1b[J")?;
        stderr.write_all(self.prompt.as_bytes())?;
        let line = buf.contents();
        stderr.write_all(line.as_bytes())?;
        // Compute where cursor ends up after writing the whole line.
        let cursor_logical = (self.prompt_cols as usize) + buf.point();
        let total_logical = (self.prompt_cols as usize) + buf.len();
        let new_rows = (total_logical / term_cols as usize) as u16;
        let cursor_row = (cursor_logical / term_cols as usize) as u16;
        let cursor_col = (cursor_logical % term_cols as usize) as u16;
        // Move cursor from end-of-line back to logical position.
        if new_rows > cursor_row {
            write!(stderr, "\x1b[{}A", new_rows - cursor_row)?;
        }
        write!(stderr, "\r")?;
        if cursor_col > 0 {
            write!(stderr, "\x1b[{}C", cursor_col)?;
        }
        stderr.flush()?;
        self.last_rows = new_rows;
        self.last_cursor_row = cursor_row;
        self.last_cursor_col = cursor_col;
        Ok(())
    }

    pub fn scripted_accept(&mut self, buf: &EditBuffer) -> io::Result<()> {
        if !self.silent {
            return Ok(());
        }
        let mut stderr = io::stderr().lock();
        stderr.write_all(self.prompt.as_bytes())?;
        stderr.write_all(buf.contents().as_bytes())?;
        stderr.flush()
    }

    pub fn scripted_eof(&mut self) -> io::Result<()> {
        if !self.silent {
            return Ok(());
        }
        let mut stderr = io::stderr().lock();
        stderr.write_all(self.prompt.as_bytes())?;
        stderr.write_all(b"exit\n")?;
        stderr.flush()
    }
}

fn term_columns() -> u16 {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 {
        return ws.ws_col;
    }
    if let Ok(cols) = std::env::var("COLUMNS").unwrap_or_default().parse::<u16>() {
        if cols > 0 {
            return cols;
        }
    }
    80
}

/// Strip ANSI CSI escapes so we can compute prompt width.
fn strip_csi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x01' {
            for n in chars.by_ref() {
                if n == '\x02' {
                    break;
                }
            }
            continue;
        }
        if c == '\x1b' {
            if let Some(&n) = chars.peek() {
                if n == '[' {
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d.is_ascii_alphabetic() {
                            break;
                        }
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}
