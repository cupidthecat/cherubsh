//! Incremental Ctrl-R history search.

use std::io::{self, Write};

use crate::buffer::EditBuffer;
use crate::input::read_key;
use crate::key::KeyEvent;
use crate::render::Renderer;
use crate::HistoryProvider;

pub enum SearchOutcome {
    ContinueEditing,
    AcceptLine(Option<usize>),
    OperateAndGetNext(Option<usize>),
}

/// Run interactive incremental search. Returns Ok when the user accepts a
/// match (placed into `buf`), or cancels (leaves `buf` untouched).
pub fn interactive_search(
    buf: &mut EditBuffer,
    history: &mut dyn HistoryProvider,
    reverse: bool,
    renderer: &mut Renderer,
) -> io::Result<SearchOutcome> {
    let original = buf.contents();
    let mut needle = String::new();
    let mut cursor: usize = if reverse { history.len() } else { 0 };
    let mut current_hit: Option<usize> = None;
    let mut stderr = io::stderr();
    loop {
        write!(
            stderr,
            "\r\x1b[K({}reverse-i-search)`{}': ",
            if reverse { "" } else { "i-" },
            needle
        )?;
        stderr.flush()?;
        let Some(key) = read_key()? else { break };
        match key {
            KeyEvent::Char(c) => needle.push(c),
            KeyEvent::Backspace => {
                needle.pop();
            }
            KeyEvent::Enter => {
                let _ = renderer.full_redraw(buf);
                return Ok(SearchOutcome::AcceptLine(current_hit));
            }
            KeyEvent::Ctrl('o') => {
                let _ = renderer.full_redraw(buf);
                return Ok(SearchOutcome::OperateAndGetNext(current_hit));
            }
            KeyEvent::Ctrl('g') | KeyEvent::Esc => {
                buf.replace_all(&original);
                let _ = renderer.full_redraw(buf);
                return Ok(SearchOutcome::ContinueEditing);
            }
            KeyEvent::Ctrl('r') => {
                if cursor > 0 {
                    cursor -= 1;
                }
            }
            KeyEvent::Ctrl('s') => {
                cursor += 1;
            }
            _ => {}
        }
        if let Some(hit) = find_match(history, &needle, cursor, reverse) {
            cursor = hit;
            current_hit = Some(hit);
            if let Some(line) = history.get(hit) {
                buf.replace_all(&line);
            }
        }
    }
    Ok(SearchOutcome::ContinueEditing)
}

fn find_match(
    history: &dyn HistoryProvider,
    needle: &str,
    start: usize,
    reverse: bool,
) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let len = history.len();
    if reverse {
        let mut idx = start.min(len.saturating_sub(1));
        loop {
            if let Some(line) = history.get(idx) {
                if line.contains(needle) {
                    return Some(idx);
                }
            }
            if idx == 0 {
                return None;
            }
            idx -= 1;
        }
    } else {
        for idx in start..len {
            if let Some(line) = history.get(idx) {
                if line.contains(needle) {
                    return Some(idx);
                }
            }
        }
        None
    }
}
