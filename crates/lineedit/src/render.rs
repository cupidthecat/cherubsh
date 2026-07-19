//! Prompt + buffer rendering with minimal-diff updates.

use std::io::{self, Write};

use crate::buffer::EditBuffer;

pub struct Renderer {
    prompt: String,
    visible_prompt: String,
    last_rows: u16,
    last_cursor_row: u16,
    last_cursor_col: u16,
    last_line: String,
    last_point: usize,
    drawn: bool,
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
        let visible_prompt = strip_csi(prompt);
        Self {
            prompt: prompt.to_string(),
            visible_prompt,
            last_rows: 0,
            last_cursor_row: 0,
            last_cursor_col: 0,
            last_line: String::new(),
            last_point: 0,
            drawn: false,
            silent,
        }
    }

    pub fn full_redraw(&mut self, buf: &EditBuffer) -> io::Result<()> {
        if self.silent {
            return Ok(());
        }
        let term_cols = term_columns().max(1);
        let mut stderr = io::stderr().lock();
        let line = buf.contents();
        let point = buf.char_point();
        let prefix: String = line.chars().take(point).collect();
        let (cursor_row, cursor_col) = screen_position(
            &format!("{}{}", self.visible_prompt, prefix),
            term_cols as usize,
        );
        let (new_rows, end_col) = screen_position(
            &format!("{}{}", self.visible_prompt, line),
            term_cols as usize,
        );

        if !self.drawn {
            stderr.write_all(prompt_for_output(&self.prompt).as_bytes())?;
            stderr.write_all(line.as_bytes())?;
            move_from_line_end(&mut stderr, new_rows, end_col, cursor_row, cursor_col)?;
            stderr.flush()?;
            self.remember(line, point, new_rows, cursor_row, cursor_col);
            return Ok(());
        }

        let old_at_end = self.last_point == self.last_line.chars().count();
        let new_at_end = point == line.chars().count();
        if old_at_end && new_at_end && line.starts_with(&self.last_line) {
            stderr.write_all(&line.as_bytes()[self.last_line.len()..])?;
            stderr.flush()?;
            self.remember(line, point, new_rows, cursor_row, cursor_col);
            return Ok(());
        }

        if line == self.last_line {
            move_cursor(
                &mut stderr,
                self.last_cursor_row,
                self.last_cursor_col,
                cursor_row,
                cursor_col,
            )?;
            stderr.flush()?;
            self.remember(line, point, new_rows, cursor_row, cursor_col);
            return Ok(());
        }

        // Move cursor to start: up by last_cursor_row, then \r.
        if self.last_cursor_row > 0 {
            write!(stderr, "\x1b[{}A", self.last_cursor_row)?;
        }
        write!(stderr, "\r\x1b[J")?;
        stderr.write_all(prompt_for_output(&self.prompt).as_bytes())?;
        stderr.write_all(line.as_bytes())?;
        move_from_line_end(&mut stderr, new_rows, end_col, cursor_row, cursor_col)?;
        stderr.flush()?;
        self.remember(line, point, new_rows, cursor_row, cursor_col);
        Ok(())
    }

    fn remember(&mut self, line: String, point: usize, rows: u16, row: u16, column: u16) {
        self.last_line = line;
        self.last_point = point;
        self.last_rows = rows;
        self.last_cursor_row = row;
        self.last_cursor_col = column;
        self.drawn = true;
    }

    pub fn columns(&self) -> usize {
        term_columns().max(1) as usize
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
        stderr.write_all(b"exit")?;
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
                    for d in chars.by_ref() {
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

fn move_from_line_end(
    output: &mut impl Write,
    end_row: u16,
    end_col: u16,
    cursor_row: u16,
    cursor_col: u16,
) -> io::Result<()> {
    if end_row == cursor_row && end_col == cursor_col {
        return Ok(());
    }
    if end_row > cursor_row {
        write!(output, "\x1b[{}A", end_row - cursor_row)?;
    }
    write!(output, "\r")?;
    if cursor_col > 0 {
        write!(output, "\x1b[{}C", cursor_col)?;
    }
    Ok(())
}

fn move_cursor(
    output: &mut impl Write,
    old_row: u16,
    old_col: u16,
    new_row: u16,
    new_col: u16,
) -> io::Result<()> {
    if old_row > new_row {
        write!(output, "\x1b[{}A", old_row - new_row)?;
    } else if new_row > old_row {
        write!(output, "\x1b[{}B", new_row - old_row)?;
    }
    if old_col != new_col || old_row != new_row {
        write!(output, "\r")?;
        if new_col > 0 {
            write!(output, "\x1b[{}C", new_col)?;
        }
    }
    Ok(())
}

fn prompt_for_output(prompt: &str) -> String {
    prompt
        .chars()
        .filter(|ch| !matches!(ch, '\x01' | '\x02'))
        .collect()
}

fn screen_position(text: &str, columns: usize) -> (u16, u16) {
    let columns = columns.max(1);
    let mut row = 0usize;
    let mut column = 0usize;
    for ch in text.chars() {
        match ch {
            '\n' => {
                row += 1;
                column = 0;
            }
            '\r' => column = 0,
            '\t' => {
                let width = 8 - (column % 8);
                column += width;
            }
            _ => column += char_width(ch),
        }
        if column >= columns {
            row += column / columns;
            column %= columns;
        }
    }
    (row.min(u16::MAX as usize) as u16, column as u16)
}

fn char_width(ch: char) -> usize {
    if ch == '\0' || ch.is_control() || is_combining(ch) {
        return 0;
    }
    if is_wide(ch) {
        2
    } else {
        1
    }
}

fn is_combining(ch: char) -> bool {
    matches!(ch as u32,
        0x0300..=0x036f | 0x0483..=0x0489 | 0x0591..=0x05bd |
        0x05bf | 0x05c1..=0x05c2 | 0x05c4..=0x05c5 | 0x0610..=0x061a |
        0x064b..=0x065f | 0x0670 | 0x06d6..=0x06ed | 0x0711 |
        0x0730..=0x074a | 0x07a6..=0x07b0 | 0x07eb..=0x07f3 |
        0x0816..=0x082d | 0x0859..=0x085b | 0x08d3..=0x0902 |
        0x093a | 0x093c | 0x0941..=0x0948 | 0x094d | 0x0951..=0x0957 |
        0x0962..=0x0963 | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff |
        0x20d0..=0x20ff | 0xfe00..=0xfe0f | 0xfe20..=0xfe2f |
        0xe0100..=0xe01ef)
}

fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115f | 0x231a..=0x231b | 0x2329..=0x232a |
        0x23e9..=0x23ec | 0x23f0 | 0x23f3 | 0x25fd..=0x25fe |
        0x2614..=0x2615 | 0x2648..=0x2653 | 0x267f | 0x2693 |
        0x26a1 | 0x26aa..=0x26ab | 0x26bd..=0x26be | 0x26c4..=0x26c5 |
        0x26ce | 0x26d4 | 0x26ea | 0x26f2..=0x26f3 | 0x26f5 |
        0x26fa | 0x26fd | 0x2705 | 0x270a..=0x270b | 0x2728 |
        0x274c | 0x274e | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 |
        0x27b0 | 0x27bf | 0x2b1b..=0x2b1c | 0x2b50 | 0x2b55 |
        0x2e80..=0x303e | 0x3040..=0xa4cf | 0xac00..=0xd7a3 |
        0xf900..=0xfaff | 0xfe10..=0xfe19 | 0xfe30..=0xfe6f |
        0xff00..=0xff60 | 0xffe0..=0xffe6 | 0x16fe0..=0x16fe4 |
        0x16ff0..=0x16ff1 | 0x17000..=0x187f7 | 0x18800..=0x18cd5 |
        0x18d00..=0x18d08 | 0x1b000..=0x1b2ff | 0x1f000..=0x1faff |
        0x20000..=0x3fffd)
}

#[cfg(test)]
mod tests {
    use super::{prompt_for_output, screen_position, strip_csi};

    #[test]
    fn prompt_markers_and_csi_are_not_counted() {
        assert_eq!(strip_csi("\u{1}\x1b[31m\u{2}> "), "> ");
        assert_eq!(prompt_for_output("\u{1}\x1b[31m\u{2}> "), "\x1b[31m> ");
    }

    #[test]
    fn wide_and_combining_characters_use_terminal_columns() {
        assert_eq!(screen_position("界e\u{301}", 80), (0, 3));
        assert_eq!(screen_position("123界", 5), (1, 0));
    }
}
