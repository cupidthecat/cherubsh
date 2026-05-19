//! Hand-rolled readline-equivalent line editor.
//!
//! Strict-parity-max implementation: termcap detection, raw-mode termios
//! enter/leave, ESC-sequence keyboard input, emacs + vi modes, kill-ring,
//! history search, programmable completion callback, multi-line redraw.
//! Bash links to GNU Readline; we re-implement the surface that matters
//! for shell interaction.

mod buffer;
mod history_search;
mod input;
mod key;
mod killring;
mod raw_mode;
mod render;

pub use buffer::EditBuffer;
pub use key::KeyEvent;
pub use killring::KillRing;
pub use raw_mode::RawMode;

use std::io::{self, Write};

use cherubsh_common::keymap::{EditAction, Keymap};

/// Outcome of `LineEditor::readline`.
#[derive(Debug)]
pub enum EditError {
    /// Ctrl-C / SIGINT mid-edit. The current buffer is discarded.
    Interrupted,
    /// Ctrl-D on empty line (EOF).
    Eof,
    /// Underlying I/O error.
    Io(io::Error),
}

impl From<io::Error> for EditError {
    fn from(e: io::Error) -> Self {
        EditError::Io(e)
    }
}

/// Provided by the shell. Invoked when the user presses Tab.
pub trait CompletionProvider {
    /// Given the current input + cursor position, return the candidate
    /// matches for the word under the cursor.
    fn complete(&mut self, line: &str, point: usize) -> Vec<String>;
}

/// Provided by the shell. Used for Ctrl-P / Ctrl-N / Ctrl-R navigation.
pub trait HistoryProvider {
    fn len(&self) -> usize;
    fn get(&self, idx: usize) -> Option<String>;
    /// Add the just-accepted line (only meaningful if not already in
    /// history via the shell's HISTCONTROL handling).
    fn add(&mut self, _line: &str) {}
}

/// State carried across `readline()` calls - kill-ring, last yank, etc.
pub struct LineEditor {
    pub kill_ring: KillRing,
    pub keymap: Keymap,
    pub last_action: Option<EditAction>,
    pub vi_mode: bool,
    pending_history_index: Option<usize>,
    pending_history_entries: Option<Vec<String>>,
}

impl LineEditor {
    pub fn new(keymap: Keymap) -> Self {
        Self {
            kill_ring: KillRing::new(),
            keymap,
            last_action: None,
            vi_mode: false,
            pending_history_index: None,
            pending_history_entries: None,
        }
    }

    /// Read one line with editing. `history` and `completion` are
    /// borrowed for the duration of the call.
    pub fn readline(
        &mut self,
        prompt: &str,
        history: &mut dyn HistoryProvider,
        completion: &mut dyn CompletionProvider,
    ) -> Result<String, EditError> {
        self.readline_inner(prompt, history, completion, true)
    }

    /// Read from a non-tty interactive input stream. Bash still lets
    /// Readline consume control bytes in `bash -i < pipe`; upstream tests use
    /// that path for scripted history navigation.
    pub fn readline_scripted(
        &mut self,
        prompt: &str,
        history: &mut dyn HistoryProvider,
        completion: &mut dyn CompletionProvider,
    ) -> Result<String, EditError> {
        self.readline_inner(prompt, history, completion, false)
    }

    fn readline_inner(
        &mut self,
        prompt: &str,
        history: &mut dyn HistoryProvider,
        completion: &mut dyn CompletionProvider,
        raw_mode: bool,
    ) -> Result<String, EditError> {
        let mut buf = EditBuffer::new();
        let mut history_index: Option<usize> = None;
        let mut saved_current: Option<String> = None;
        if let Some(idx) = self.pending_history_index.take() {
            let pending_entries = self.pending_history_entries.as_ref();
            let len = pending_entries
                .map(|entries| entries.len())
                .unwrap_or(history.len());
            let idx = idx.min(len);
            history_index = Some(idx);
            if idx < len {
                if let Some(line) = pending_entries
                    .and_then(|entries| entries.get(idx).cloned())
                    .or_else(|| history.get(idx))
                {
                    buf.replace_all(&line);
                }
            }
        }
        let raw = if raw_mode {
            Some(RawMode::enter()?)
        } else {
            None
        };
        let mut renderer = render::Renderer::new(prompt);
        renderer.full_redraw(&buf)?;
        let result = loop {
            let key = match input::read_key()? {
                Some(k) => k,
                None if raw_mode => continue,
                None if buf.is_empty() => break Err(EditError::Eof),
                None => break Ok(buf.contents()),
            };
            match self.dispatch(
                key,
                &mut buf,
                history,
                &mut history_index,
                &mut saved_current,
                completion,
                &mut renderer,
            )? {
                LoopOutcome::Continue => {}
                LoopOutcome::Accept => break Ok(buf.contents()),
                LoopOutcome::Interrupted => break Err(EditError::Interrupted),
                LoopOutcome::Eof => break Err(EditError::Eof),
            }
        };
        drop(raw);
        // Newline terminator after accept; renderer left cursor at end of line.
        if matches!(&result, Ok(_)) {
            let _ = io::stderr().write_all(b"\n");
            let _ = io::stderr().flush();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        key: KeyEvent,
        buf: &mut EditBuffer,
        history: &mut dyn HistoryProvider,
        history_index: &mut Option<usize>,
        saved_current: &mut Option<String>,
        completion: &mut dyn CompletionProvider,
        renderer: &mut render::Renderer,
    ) -> Result<LoopOutcome, EditError> {
        let action = match key.lookup_in(&self.keymap) {
            Some(a) => a,
            None => match &key {
                KeyEvent::Char(_) => EditAction::SelfInsert,
                _ => EditAction::Noop,
            },
        };
        self.last_action = Some(action);
        match action {
            EditAction::SelfInsert => {
                if let KeyEvent::Char(c) = key {
                    buf.insert(c);
                }
            }
            EditAction::BackwardDeleteChar => {
                buf.backward_delete();
            }
            EditAction::DeleteChar | EditAction::DeleteCharOrList => {
                if buf.is_empty() {
                    return Ok(LoopOutcome::Eof);
                }
                buf.delete_forward();
            }
            EditAction::ForwardChar => buf.move_right(),
            EditAction::BackwardChar => buf.move_left(),
            EditAction::ForwardWord => buf.move_word_right(),
            EditAction::BackwardWord => buf.move_word_left(),
            EditAction::BeginningOfLine => buf.move_home(),
            EditAction::EndOfLine => buf.move_end(),
            EditAction::KillLine => {
                let killed = buf.kill_to_end();
                self.kill_ring.push(killed);
            }
            EditAction::UnixLineDiscard => {
                let killed = buf.kill_to_beginning();
                self.kill_ring.push(killed);
            }
            EditAction::UnixWordRubout => {
                let killed = buf.backward_kill_word();
                self.kill_ring.push(killed);
            }
            EditAction::KillWord => {
                let killed = buf.forward_kill_word();
                self.kill_ring.push(killed);
            }
            EditAction::Yank => {
                if let Some(text) = self.kill_ring.current() {
                    buf.insert_str(text);
                }
            }
            EditAction::YankPop => {
                self.kill_ring.rotate();
            }
            EditAction::PreviousHistory => {
                navigate_history(history, history_index, saved_current, buf, -1);
            }
            EditAction::NextHistory => {
                navigate_history(history, history_index, saved_current, buf, 1);
            }
            EditAction::OperateAndGetNext => {
                self.queue_operate_and_get_next(history, *history_index);
                renderer.full_redraw(buf)?;
                return Ok(LoopOutcome::Accept);
            }
            EditAction::AcceptLine | EditAction::NewLine => {
                self.clear_operate_and_get_next();
                renderer.full_redraw(buf)?;
                return Ok(LoopOutcome::Accept);
            }
            EditAction::Abort => {
                self.clear_operate_and_get_next();
                buf.clear();
                return Ok(LoopOutcome::Interrupted);
            }
            EditAction::Complete => {
                let line = buf.as_str();
                let matches = completion.complete(&line, buf.point());
                buf.apply_completion(&matches);
            }
            EditAction::ClearScreen => {
                let _ = io::stderr().write_all(b"\x1b[2J\x1b[H");
                renderer.full_redraw(buf)?;
                return Ok(LoopOutcome::Continue);
            }
            EditAction::TransposeChars => buf.transpose_chars(),
            EditAction::UpcaseWord => buf.upcase_word(),
            EditAction::DowncaseWord => buf.downcase_word(),
            EditAction::CapitalizeWord => buf.capitalize_word(),
            EditAction::ReverseSearchHistory => {
                match history_search::interactive_search(buf, history, true, renderer)? {
                    history_search::SearchOutcome::ContinueEditing => {}
                    history_search::SearchOutcome::AcceptLine(hit) => {
                        *history_index = hit;
                        self.clear_operate_and_get_next();
                        return Ok(LoopOutcome::Accept);
                    }
                    history_search::SearchOutcome::OperateAndGetNext(hit) => {
                        *history_index = hit;
                        self.queue_operate_and_get_next(history, *history_index);
                        return Ok(LoopOutcome::Accept);
                    }
                }
            }
            EditAction::ForwardSearchHistory => {
                match history_search::interactive_search(buf, history, false, renderer)? {
                    history_search::SearchOutcome::ContinueEditing => {}
                    history_search::SearchOutcome::AcceptLine(hit) => {
                        *history_index = hit;
                        self.clear_operate_and_get_next();
                        return Ok(LoopOutcome::Accept);
                    }
                    history_search::SearchOutcome::OperateAndGetNext(hit) => {
                        *history_index = hit;
                        self.queue_operate_and_get_next(history, *history_index);
                        return Ok(LoopOutcome::Accept);
                    }
                }
            }
            EditAction::ViMovementMode => {
                self.vi_mode = true;
            }
            EditAction::ViInsertionMode => {
                self.vi_mode = false;
            }
            EditAction::ViAppendMode => {
                self.vi_mode = false;
                buf.move_right();
            }
            EditAction::ViAppendEol => {
                self.vi_mode = false;
                buf.move_end();
            }
            EditAction::UndoCmd | EditAction::RevertLine => {
                buf.undo();
            }
            EditAction::Noop => {}
            _ => {
                // Unimplemented action - bell.
                let _ = io::stderr().write_all(b"\x07");
            }
        }
        renderer.full_redraw(buf)?;
        Ok(LoopOutcome::Continue)
    }

    fn queue_operate_and_get_next(
        &mut self,
        history: &dyn HistoryProvider,
        history_index: Option<usize>,
    ) {
        if self.pending_history_entries.is_none() {
            self.pending_history_entries = Some(
                (0..history.len())
                    .filter_map(|idx| history.get(idx))
                    .collect(),
            );
        }
        let len = self
            .pending_history_entries
            .as_ref()
            .map(|entries| entries.len())
            .unwrap_or_else(|| history.len());
        let next = history_index.map(|idx| idx + 1).unwrap_or(len);
        self.pending_history_index = Some(next.min(len));
    }

    fn clear_operate_and_get_next(&mut self) {
        self.pending_history_index = None;
        self.pending_history_entries = None;
    }
}

enum LoopOutcome {
    Continue,
    Accept,
    Interrupted,
    Eof,
}

fn navigate_history(
    history: &mut dyn HistoryProvider,
    history_index: &mut Option<usize>,
    saved_current: &mut Option<String>,
    buf: &mut EditBuffer,
    delta: i32,
) {
    let len = history.len();
    if len == 0 {
        return;
    }
    let cur_idx = history_index.unwrap_or(len);
    let new_idx = if delta < 0 {
        if cur_idx == 0 {
            return;
        }
        cur_idx - 1
    } else {
        if cur_idx >= len {
            return;
        }
        cur_idx + 1
    };
    if history_index.is_none() {
        *saved_current = Some(buf.contents());
    }
    *history_index = Some(new_idx);
    let text = if new_idx == len {
        saved_current.clone().unwrap_or_default()
    } else {
        history.get(new_idx).unwrap_or_default()
    };
    buf.replace_all(&text);
}
