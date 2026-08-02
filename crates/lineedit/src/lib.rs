//! UTF-8 line editor used by interactive CherubSH sessions.
//!
//! It supports Emacs and Vi keymaps, history search, completion, raw terminal
//! input, and multi-line redisplay.

mod buffer;
mod history_search;
mod input;
mod key;
mod killring;
mod raw_mode;
mod render;
mod termcap;
mod vi;

use buffer::CompletionApplication;
pub use buffer::EditBuffer;
pub use input::{set_input_deadline, InputDecoder};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub matches: Vec<String>,
    pub replace_start: usize,
    pub suppress_append: bool,
    pub append_character: Option<char>,
    pub filenames: bool,
}

/// Controls when ambiguous completion candidates are displayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletionDisplayPolicy {
    pub show_all_if_ambiguous: bool,
    pub show_all_if_unmodified: bool,
}

/// Supplies candidates when the user presses Tab.
pub trait CompletionProvider {
    fn complete(&mut self, line: &str, point: usize) -> Completion;

    fn completion_display_policy(&self) -> CompletionDisplayPolicy {
        CompletionDisplayPolicy::default()
    }

    fn run_shell_command(
        &mut self,
        _command: &str,
        _line: &str,
        _point: usize,
    ) -> Option<(String, usize)> {
        None
    }
}

/// Provided by the shell. Used for Ctrl-P / Ctrl-N / Ctrl-R navigation.
pub trait HistoryProvider {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
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
    self_insert_unbound: bool,
    vi_command_keymap: Keymap,
    vi_state: vi::ViState,
    mark: Option<usize>,
    last_yank: Option<(usize, usize)>,
    menu: Option<MenuState>,
    original_line: String,
    pending_history_index: Option<usize>,
    pending_history_entries: Option<Vec<String>>,
    last_completion_changed: bool,
}

#[derive(Clone)]
struct MenuState {
    completion: Completion,
    index: usize,
}

impl LineEditor {
    pub fn new(keymap: Keymap) -> Self {
        let mut vi_command_keymap = Keymap::new("vi-command");
        vi_command_keymap.install_vi_movement_defaults();
        Self {
            kill_ring: KillRing::new(),
            keymap,
            last_action: None,
            vi_mode: false,
            self_insert_unbound: true,
            vi_command_keymap,
            vi_state: vi::ViState::new(),
            mark: None,
            last_yank: None,
            menu: None,
            original_line: String::new(),
            pending_history_index: None,
            pending_history_entries: None,
            last_completion_changed: false,
        }
    }

    pub fn set_vi_command_keymap(&mut self, keymap: Keymap) {
        self.vi_command_keymap = keymap;
    }

    pub fn set_self_insert_unbound(&mut self, enabled: bool) {
        self.self_insert_unbound = enabled;
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
        self.original_line = buf.contents();
        self.mark = Some(buf.char_point());
        self.last_yank = None;
        self.menu = None;
        self.vi_state.reset();
        self.vi_mode = self.keymap.name == "vi-command";
        let raw = if raw_mode {
            Some(RawMode::enter()?)
        } else {
            None
        };
        let mut renderer = if raw_mode {
            render::Renderer::new(prompt)
        } else {
            render::Renderer::silent(prompt)
        };
        renderer.full_redraw(&buf)?;
        let result = loop {
            let key =
                match input::read_key_mode(self.keymap.name.starts_with("vi-") && !self.vi_mode)? {
                    Some(k) => k,
                    None if raw_mode => continue,
                    None if buf.is_empty() => {
                        if !raw_mode {
                            renderer.scripted_eof()?;
                        }
                        break Err(EditError::Eof);
                    }
                    None => {
                        if !raw_mode {
                            renderer.scripted_accept(&buf)?;
                        }
                        break Ok(buf.contents());
                    }
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
                LoopOutcome::Accept => {
                    if !raw_mode {
                        renderer.scripted_accept(&buf)?;
                    }
                    break Ok(buf.contents());
                }
                LoopOutcome::Interrupted => break Err(EditError::Interrupted),
                LoopOutcome::Eof => break Err(EditError::Eof),
            }
        };
        drop(raw);
        // Readline leaves the next program output at the start of a fresh line.
        if matches!(&result, Ok(_) | Err(EditError::Eof)) {
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
        if key == KeyEvent::Ctrl('c') {
            self.clear_operate_and_get_next();
            self.vi_state.reset();
            return Ok(LoopOutcome::Interrupted);
        }
        if let KeyEvent::Paste(text) = key {
            buf.break_undo_group();
            buf.insert_str(&text);
            self.last_action = Some(EditAction::SelfInsert);
            self.menu = None;
            renderer.full_redraw(buf)?;
            return Ok(LoopOutcome::Continue);
        }
        if self.vi_mode && self.handle_vi_command(&key, buf, history, history_index, saved_current)
        {
            self.menu = None;
            renderer.full_redraw(buf)?;
            return Ok(LoopOutcome::Continue);
        }

        let previous_action = self.last_action;
        let action = self.resolve_action(&key)?.unwrap_or({
            if !self.vi_mode && self.self_insert_unbound && matches!(key, KeyEvent::Char(_)) {
                EditAction::SelfInsert
            } else {
                EditAction::Noop
            }
        });
        self.last_action = Some(action);
        if action != EditAction::SelfInsert {
            buf.break_undo_group();
        }
        if !matches!(
            action,
            EditAction::MenuComplete | EditAction::MenuCompleteBackward
        ) {
            self.menu = None;
        }
        match action {
            EditAction::SelfInsert => {
                if let KeyEvent::Char(c) = key {
                    buf.insert(c);
                }
            }
            EditAction::QuotedInsert => {
                if let Some(ch) = input::read_literal_char()? {
                    buf.insert(ch);
                }
            }
            EditAction::TabInsert => buf.insert('\t'),
            EditAction::BackwardDeleteChar => buf.backward_delete(),
            EditAction::DeleteChar => buf.delete_forward(),
            EditAction::DeleteCharOrList => {
                if buf.is_empty() {
                    return Ok(LoopOutcome::Eof);
                }
                if buf.char_point() == buf.len() {
                    if display_completions(
                        &completion.complete(&buf.as_str(), buf.point()),
                        renderer.columns(),
                    )? {
                        renderer.invalidate();
                    }
                } else {
                    buf.delete_forward();
                }
            }
            EditAction::ForwardChar => buf.move_right(),
            EditAction::BackwardChar => buf.move_left(),
            EditAction::ForwardWord => buf.move_word_right(),
            EditAction::BackwardWord => buf.move_word_left(),
            EditAction::BeginningOfLine => buf.move_home(),
            EditAction::EndOfLine => buf.move_end(),
            EditAction::NextScreenLine => buf.move_visual_line(renderer.columns(), 1),
            EditAction::PreviousScreenLine => buf.move_visual_line(renderer.columns(), -1),
            EditAction::KillLine => {
                let killed = buf.kill_to_end();
                self.record_kill(killed, false, previous_action);
            }
            EditAction::BackwardKillLine | EditAction::UnixLineDiscard => {
                let killed = buf.kill_to_beginning();
                self.record_kill(killed, true, previous_action);
            }
            EditAction::BackwardKillWord | EditAction::UnixWordRubout => {
                let killed = buf.backward_kill_word();
                self.record_kill(killed, true, previous_action);
            }
            EditAction::KillWord => {
                let killed = buf.forward_kill_word();
                self.record_kill(killed, false, previous_action);
            }
            EditAction::KillRegion => {
                let killed = buf.kill_range(self.mark.unwrap_or(0), buf.char_point());
                self.record_kill(killed, false, previous_action);
            }
            EditAction::Yank => {
                if let Some(text) = self.kill_ring.current().map(str::to_string) {
                    let start = buf.char_point();
                    buf.insert_str(&text);
                    self.last_yank = Some((start, buf.char_point()));
                }
            }
            EditAction::YankPop => {
                if matches!(
                    previous_action,
                    Some(EditAction::Yank | EditAction::YankPop)
                ) && self.kill_ring.len() > 1
                {
                    if let Some((start, end)) = self.last_yank {
                        self.kill_ring.rotate();
                        if let Some(text) = self.kill_ring.current().map(str::to_string) {
                            buf.replace_char_range(start, end, &text);
                            self.last_yank = Some((start, buf.char_point()));
                        }
                    }
                }
            }
            EditAction::YankLastArg => yank_history_argument(history, buf, None),
            EditAction::YankNthArg => yank_history_argument(history, buf, Some(1)),
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
            EditAction::BeginningOfHistory => {
                if !history.is_empty() {
                    if history_index.is_none() {
                        *saved_current = Some(buf.contents());
                    }
                    *history_index = Some(0);
                    buf.replace_all(&history.get(0).unwrap_or_default());
                }
            }
            EditAction::EndOfHistory => {
                *history_index = Some(history.len());
                buf.replace_all(saved_current.as_deref().unwrap_or_default());
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
                let mode = completion_mode(
                    previous_action,
                    self.last_completion_changed,
                    completion.completion_display_policy(),
                );
                let line = buf.as_str();
                let result = completion.complete(&line, buf.point());
                self.last_completion_changed = if mode == CompletionDisplayMode::List {
                    if display_completions(&result, renderer.columns())? {
                        renderer.invalidate();
                    }
                    false
                } else {
                    match buf.apply_completion_result(&result) {
                        CompletionApplication::NoMatches => {
                            ring_bell();
                            false
                        }
                        CompletionApplication::Unique { changed } => changed,
                        CompletionApplication::Ambiguous { changed } => {
                            match mode {
                                CompletionDisplayMode::Normal => ring_bell(),
                                CompletionDisplayMode::ShowAll => {
                                    if display_completions(&result, renderer.columns())? {
                                        renderer.invalidate();
                                    }
                                }
                                CompletionDisplayMode::ShowUnmodified if !changed => {
                                    if display_completions(&result, renderer.columns())? {
                                        renderer.invalidate();
                                    }
                                }
                                CompletionDisplayMode::ShowUnmodified
                                | CompletionDisplayMode::List => {}
                            }
                            changed
                        }
                    }
                };
            }
            EditAction::PossibleCompletions => {
                if display_completions(
                    &completion.complete(&buf.as_str(), buf.point()),
                    renderer.columns(),
                )? {
                    renderer.invalidate();
                }
            }
            EditAction::InsertCompletions => {
                let result = completion.complete(&buf.as_str(), buf.point());
                if !result.matches.is_empty() {
                    let joined = result.matches.join(" ");
                    buf.replace_completion(
                        &Completion {
                            suppress_append: true,
                            ..result
                        },
                        &joined,
                    );
                }
            }
            EditAction::MenuComplete => {
                self.menu_complete(buf, completion, false);
            }
            EditAction::MenuCompleteBackward => {
                self.menu_complete(buf, completion, true);
            }
            EditAction::ClearScreen => {
                let _ = io::stderr().write_all(termcap::CLEAR_SCREEN.as_bytes());
                renderer.full_redraw(buf)?;
                return Ok(LoopOutcome::Continue);
            }
            EditAction::Redraw => {
                renderer.full_redraw(buf)?;
                return Ok(LoopOutcome::Continue);
            }
            EditAction::TransposeChars => buf.transpose_chars(),
            EditAction::TransposeWords => buf.transpose_words(),
            EditAction::UpcaseWord => buf.upcase_word(),
            EditAction::DowncaseWord => buf.downcase_word(),
            EditAction::CapitalizeWord => buf.capitalize_word(),
            EditAction::ReverseSearchHistory | EditAction::NonIncrementalReverseSearchHistory => {
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
            EditAction::ForwardSearchHistory | EditAction::NonIncrementalForwardSearchHistory => {
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
            EditAction::HistorySearchBackward => {
                navigate_history_prefix(history, history_index, saved_current, buf, -1);
            }
            EditAction::HistorySearchForward => {
                navigate_history_prefix(history, history_index, saved_current, buf, 1);
            }
            EditAction::ViMovementMode => {
                self.vi_mode = true;
                self.vi_state.reset();
            }
            EditAction::ViInsertionMode => {
                self.vi_mode = false;
                self.vi_state.reset();
            }
            EditAction::ViAppendMode => {
                self.vi_mode = false;
                buf.move_right();
                self.vi_state.reset();
            }
            EditAction::ViAppendEol => {
                self.vi_mode = false;
                buf.move_end();
                self.vi_state.reset();
            }
            EditAction::UndoCmd => buf.undo(),
            EditAction::RevertLine => {
                buf.replace_all(&self.original_line);
            }
            EditAction::Tilde => expand_tilde_at_point(buf),
            EditAction::Quit => {
                if buf.is_empty() {
                    return Ok(LoopOutcome::Eof);
                }
                ring_bell();
            }
            EditAction::Noop => {}
            EditAction::ShellCommand(index) => {
                if let Some(command) = self
                    .active_keymap()
                    .shell_commands
                    .get(index as usize)
                    .cloned()
                {
                    if let Some((line, point)) =
                        completion.run_shell_command(&command, &buf.as_str(), buf.point())
                    {
                        buf.replace_all_at_byte(&line, point);
                    }
                }
            }
            EditAction::Macro(index) => {
                if let Some(text) = self.active_keymap().macros.get(index as usize).cloned() {
                    buf.insert_str(&text);
                }
            }
        }
        renderer.full_redraw(buf)?;
        Ok(LoopOutcome::Continue)
    }

    fn active_keymap(&self) -> &Keymap {
        if self.vi_mode {
            &self.vi_command_keymap
        } else {
            &self.keymap
        }
    }

    fn resolve_action(&self, key: &KeyEvent) -> io::Result<Option<EditAction>> {
        let keymap = self.active_keymap();
        let mut sequence = key.to_sequence();
        if sequence.is_empty() {
            return Ok(None);
        }
        if matches!(key, KeyEvent::Char('\\')) && keymap.lookup(&sequence).is_none() {
            return Ok(None);
        }
        loop {
            let exact = keymap.lookup(&sequence);
            let has_longer = keymap.bindings.keys().any(|candidate| {
                candidate.starts_with(&sequence) && candidate.len() > sequence.len()
            });
            if !has_longer {
                return Ok(exact);
            }
            let Some(next) = input::read_key()? else {
                return Ok(exact);
            };
            sequence.push_str(&next.to_sequence());
            let still_a_prefix = keymap
                .bindings
                .keys()
                .any(|candidate| candidate.starts_with(&sequence));
            if !still_a_prefix {
                ring_bell();
                return Ok(exact);
            }
        }
    }

    fn record_kill(&mut self, killed: String, backward: bool, previous_action: Option<EditAction>) {
        if is_kill_action(previous_action) {
            if backward {
                self.kill_ring.prepend(&killed);
            } else {
                self.kill_ring.append(&killed);
            }
        } else {
            self.kill_ring.push(killed);
        }
    }

    fn menu_complete(
        &mut self,
        buf: &mut EditBuffer,
        provider: &mut dyn CompletionProvider,
        backward: bool,
    ) {
        let continuing = self.menu.is_some();
        let mut state = self.menu.take().unwrap_or_else(|| MenuState {
            completion: provider.complete(&buf.as_str(), buf.point()),
            index: if backward { usize::MAX } else { 0 },
        });
        let len = state.completion.matches.len();
        if len == 0 {
            ring_bell();
            return;
        }
        state.index = if state.index == usize::MAX {
            len - 1
        } else if backward {
            state.index.checked_sub(1).unwrap_or(len - 1)
        } else if !continuing && state.index == 0 {
            0
        } else {
            (state.index + 1) % len
        };
        let choice = state.completion.matches[state.index].clone();
        let mut replacement = state.completion.clone();
        replacement.suppress_append = true;
        buf.replace_completion(&replacement, &choice);
        self.menu = Some(state);
    }

    fn handle_vi_command(
        &mut self,
        key: &KeyEvent,
        buf: &mut EditBuffer,
        history: &mut dyn HistoryProvider,
        history_index: &mut Option<usize>,
        saved_current: &mut Option<String>,
    ) -> bool {
        if matches!(key, KeyEvent::Esc) {
            self.vi_state.reset();
            return true;
        }
        let KeyEvent::Char(ch) = *key else {
            return false;
        };

        if let Some(pending) = self.vi_state.pending.take() {
            match pending {
                vi::Pending::Replace => {
                    let count = self.vi_state.take_count();
                    let start = buf.char_point();
                    let end = start.saturating_add(count).min(buf.len());
                    if start < end {
                        let replacement: String = std::iter::repeat_n(ch, end - start).collect();
                        buf.replace_char_range(start, end, &replacement);
                        buf.set_char_point(end.saturating_sub(1));
                    }
                    return true;
                }
                vi::Pending::Find { backward, till } => {
                    let count = self.vi_state.take_count();
                    for _ in 0..count {
                        let found = if backward {
                            buf.find_backward(ch, !till)
                        } else {
                            buf.find_forward(ch, !till)
                        };
                        if let Some(point) = found {
                            buf.set_char_point(point);
                        } else {
                            ring_bell();
                            break;
                        }
                    }
                    return true;
                }
                vi::Pending::Operator(op) => {
                    let count = self.vi_state.take_count();
                    if matches!(
                        (op, ch),
                        (vi::Op::Delete, 'd') | (vi::Op::Change, 'c') | (vi::Op::Yank, 'y')
                    ) {
                        self.apply_vi_line_operator(op, buf);
                        return true;
                    }
                    if let Some((start, end)) = vi_motion_range(buf, ch, count) {
                        self.apply_vi_operator(op, buf, start, end);
                    } else {
                        ring_bell();
                    }
                    return true;
                }
            }
        }

        if ch.is_ascii_digit() && (ch != '0' || self.vi_state.count.is_some()) {
            self.vi_state
                .push_digit(ch.to_digit(10).unwrap_or(0) as usize);
            return true;
        }
        match ch {
            'd' => {
                self.vi_state.pending = Some(vi::Pending::Operator(vi::Op::Delete));
                return true;
            }
            'c' => {
                self.vi_state.pending = Some(vi::Pending::Operator(vi::Op::Change));
                return true;
            }
            'y' => {
                self.vi_state.pending = Some(vi::Pending::Operator(vi::Op::Yank));
                return true;
            }
            'f' | 'F' | 't' | 'T' => {
                self.vi_state.pending = Some(vi::Pending::Find {
                    backward: matches!(ch, 'F' | 'T'),
                    till: matches!(ch, 't' | 'T'),
                });
                return true;
            }
            'r' => {
                self.vi_state.pending = Some(vi::Pending::Replace);
                return true;
            }
            _ => {}
        }
        let count = self.vi_state.take_count();
        match ch {
            'h' => {
                buf.set_char_point(buf.char_point().saturating_sub(count));
                true
            }
            'l' | ' ' => {
                buf.set_char_point(buf.char_point().saturating_add(count).min(buf.len()));
                true
            }
            '0' => {
                buf.move_home();
                true
            }
            '$' => {
                buf.move_end();
                true
            }
            'w' => {
                for _ in 0..count {
                    buf.move_word_right();
                }
                true
            }
            'b' => {
                for _ in 0..count {
                    buf.move_word_left();
                }
                true
            }
            'e' => {
                for _ in 0..count {
                    buf.move_word_right();
                }
                buf.move_left();
                true
            }
            'k' => {
                for _ in 0..count {
                    navigate_history(history, history_index, saved_current, buf, -1);
                }
                true
            }
            'j' => {
                for _ in 0..count {
                    navigate_history(history, history_index, saved_current, buf, 1);
                }
                true
            }
            'i' => {
                self.vi_mode = false;
                true
            }
            'I' => {
                buf.move_home();
                self.vi_mode = false;
                true
            }
            'a' => {
                buf.move_right();
                self.vi_mode = false;
                true
            }
            'A' => {
                buf.move_end();
                self.vi_mode = false;
                true
            }
            'x' => {
                let start = buf.char_point();
                let killed = buf.kill_range(start, start.saturating_add(count));
                self.kill_ring.push(killed);
                true
            }
            'X' => {
                let end = buf.char_point();
                let killed = buf.kill_range(end.saturating_sub(count), end);
                self.kill_ring.push(killed);
                true
            }
            'D' => {
                let killed = buf.kill_to_end();
                self.kill_ring.push(killed);
                true
            }
            'C' => {
                let killed = buf.kill_to_end();
                self.kill_ring.push(killed);
                self.vi_mode = false;
                true
            }
            'S' => {
                let killed = buf.kill_range(0, buf.len());
                self.kill_ring.push(killed);
                self.vi_mode = false;
                true
            }
            's' => {
                let start = buf.char_point();
                let killed = buf.kill_range(start, start.saturating_add(count));
                self.kill_ring.push(killed);
                self.vi_mode = false;
                true
            }
            'p' | 'P' => {
                if let Some(text) = self.kill_ring.current().map(str::to_string) {
                    if ch == 'p' {
                        buf.move_right();
                    }
                    for _ in 0..count {
                        buf.insert_str(&text);
                    }
                }
                true
            }
            '~' => {
                for _ in 0..count {
                    let point = buf.char_point();
                    let Some(current) = buf.char_at(point) else {
                        break;
                    };
                    let replacement = if current.is_lowercase() {
                        current.to_uppercase().collect::<String>()
                    } else {
                        current.to_lowercase().collect::<String>()
                    };
                    buf.replace_char_range(point, point + 1, &replacement);
                }
                true
            }
            'u' => {
                buf.undo();
                true
            }
            _ => false,
        }
    }

    fn apply_vi_line_operator(&mut self, op: vi::Op, buf: &mut EditBuffer) {
        let text = buf.slice(0, buf.len());
        match op {
            vi::Op::Yank => self.kill_ring.push(text),
            vi::Op::Delete | vi::Op::Change => {
                self.kill_ring.push(text);
                buf.clear();
                if op == vi::Op::Change {
                    self.vi_mode = false;
                }
            }
        }
    }

    fn apply_vi_operator(&mut self, op: vi::Op, buf: &mut EditBuffer, start: usize, end: usize) {
        let text = buf.slice(start, end);
        match op {
            vi::Op::Yank => self.kill_ring.push(text),
            vi::Op::Delete | vi::Op::Change => {
                self.kill_ring.push(buf.kill_range(start, end));
                if op == vi::Op::Change {
                    self.vi_mode = false;
                }
            }
        }
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

fn navigate_history_prefix(
    history: &mut dyn HistoryProvider,
    history_index: &mut Option<usize>,
    saved_current: &mut Option<String>,
    buf: &mut EditBuffer,
    direction: i32,
) {
    if history.is_empty() {
        return;
    }
    if saved_current.is_none() {
        *saved_current = Some(buf.contents());
    }
    let prefix = saved_current.as_deref().unwrap_or_default();
    let start = history_index.unwrap_or(history.len());
    if direction < 0 {
        for index in (0..start.min(history.len())).rev() {
            if let Some(line) = history.get(index) {
                if line.starts_with(prefix) {
                    *history_index = Some(index);
                    buf.replace_all(&line);
                    return;
                }
            }
        }
    } else {
        for index in start.saturating_add(1)..history.len() {
            if let Some(line) = history.get(index) {
                if line.starts_with(prefix) {
                    *history_index = Some(index);
                    buf.replace_all(&line);
                    return;
                }
            }
        }
        *history_index = Some(history.len());
        buf.replace_all(prefix);
    }
    ring_bell();
}

fn is_kill_action(action: Option<EditAction>) -> bool {
    matches!(
        action,
        Some(
            EditAction::KillLine
                | EditAction::BackwardKillLine
                | EditAction::KillWord
                | EditAction::BackwardKillWord
                | EditAction::UnixWordRubout
                | EditAction::UnixLineDiscard
                | EditAction::KillRegion
        )
    )
}

fn vi_motion_range(buf: &EditBuffer, motion: char, count: usize) -> Option<(usize, usize)> {
    let origin = buf.char_point();
    let mut probe = buf.clone();
    match motion {
        'h' => probe.set_char_point(origin.saturating_sub(count)),
        'l' | ' ' => probe.set_char_point(origin.saturating_add(count).min(buf.len())),
        '0' => probe.move_home(),
        '$' => probe.move_end(),
        'w' | 'e' => {
            for _ in 0..count {
                probe.move_word_right();
            }
        }
        'b' => {
            for _ in 0..count {
                probe.move_word_left();
            }
        }
        _ => return None,
    }
    let target = probe.char_point();
    Some(if origin <= target {
        (origin, target)
    } else {
        (target, origin)
    })
}

fn display_completions(completion: &Completion, terminal_columns: usize) -> io::Result<bool> {
    if completion.matches.is_empty() {
        ring_bell();
        return Ok(false);
    }
    let mut stderr = io::stderr().lock();
    stderr.write_all(b"\n")?;
    stderr.write_all(format_completion_rows(&completion.matches, terminal_columns).as_bytes())?;
    stderr.flush()?;
    Ok(true)
}

fn format_completion_rows(matches: &[String], terminal_columns: usize) -> String {
    let widths: Vec<usize> = matches
        .iter()
        .map(|candidate| candidate.chars().map(render::char_width).sum())
        .collect();
    let column_width = widths.iter().copied().max().unwrap_or(0).saturating_add(2);
    let mut columns = terminal_columns / column_width.max(1);
    if columns != 1 && columns.saturating_mul(column_width) == terminal_columns {
        columns = columns.saturating_sub(1);
    }
    columns = columns.max(1);
    let rows = matches.len().div_ceil(columns);
    let mut output = String::new();
    for row in 0..rows {
        for column in 0..columns {
            let index = row + column * rows;
            let Some(candidate) = matches.get(index) else {
                break;
            };
            output.push_str(candidate);
            if column + 1 < columns {
                output.push_str(&" ".repeat(column_width.saturating_sub(widths[index])));
            }
        }
        output.push('\n');
    }
    output
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionDisplayMode {
    Normal,
    List,
    ShowAll,
    ShowUnmodified,
}

fn completion_mode(
    previous_action: Option<EditAction>,
    previous_changed: bool,
    policy: CompletionDisplayPolicy,
) -> CompletionDisplayMode {
    if previous_action == Some(EditAction::Complete) && !previous_changed {
        CompletionDisplayMode::List
    } else if policy.show_all_if_ambiguous {
        CompletionDisplayMode::ShowAll
    } else if policy.show_all_if_unmodified {
        CompletionDisplayMode::ShowUnmodified
    } else {
        CompletionDisplayMode::Normal
    }
}

fn ring_bell() {
    let _ = io::stderr().write_all(&[termcap::BEL]);
    let _ = io::stderr().flush();
}

fn yank_history_argument(
    history: &dyn HistoryProvider,
    buf: &mut EditBuffer,
    index: Option<usize>,
) {
    let Some(line) = history.len().checked_sub(1).and_then(|i| history.get(i)) else {
        ring_bell();
        return;
    };
    let words = shell_words(&line);
    let selected = index
        .and_then(|i| words.get(i))
        .or_else(|| words.last())
        .cloned();
    if let Some(word) = selected {
        buf.insert_str(&word);
    } else {
        ring_bell();
    }
}

fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for ch in line.trim_end_matches(['\r', '\n']).chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' && !single {
            escaped = true;
        } else if ch == '\'' && !double {
            single = !single;
        } else if ch == '"' && !single {
            double = !double;
        } else if ch.is_whitespace() && !single && !double {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn expand_tilde_at_point(buf: &mut EditBuffer) {
    let point = buf.char_point();
    let line = buf.contents();
    let chars: Vec<char> = line.chars().collect();
    let start = chars[..point]
        .iter()
        .rposition(|ch| ch.is_whitespace() || matches!(ch, ':' | '='))
        .map_or(0, |index| index + 1);
    let word: String = chars[start..point].iter().collect();
    if word == "~" || word.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let replacement = format!("{}{}", home.to_string_lossy(), &word[1..]);
            buf.replace_char_range(start, point, &replacement);
            return;
        }
    }
    ring_bell();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{
        completion_mode, format_completion_rows, render, shell_words, vi_motion_range, Completion,
        CompletionDisplayMode, CompletionDisplayPolicy, CompletionProvider, EditAction, EditBuffer,
        HistoryProvider, KeyEvent, Keymap, LineEditor,
    };

    struct EmptyHistory;

    impl HistoryProvider for EmptyHistory {
        fn len(&self) -> usize {
            0
        }

        fn get(&self, _idx: usize) -> Option<String> {
            None
        }
    }

    struct MutatingPolicyProvider {
        calls: RefCell<Vec<&'static str>>,
        show_all: bool,
    }

    impl CompletionProvider for MutatingPolicyProvider {
        fn complete(&mut self, _line: &str, _point: usize) -> Completion {
            self.calls.borrow_mut().push("complete");
            self.show_all = true;
            Completion {
                matches: vec!["source".to_string(), "sort".to_string()],
                ..Completion::default()
            }
        }

        fn completion_display_policy(&self) -> CompletionDisplayPolicy {
            self.calls.borrow_mut().push("policy");
            CompletionDisplayPolicy {
                show_all_if_ambiguous: self.show_all,
                show_all_if_unmodified: false,
            }
        }
    }

    #[test]
    fn completion_policy_is_sampled_before_candidates_are_generated() {
        let mut keymap = Keymap::new("emacs");
        keymap.install_emacs_defaults();
        let mut editor = LineEditor::new(keymap);
        let mut buffer = EditBuffer::new();
        buffer.insert_str("so");
        let mut history = EmptyHistory;
        let mut history_index = None;
        let mut saved_current = None;
        let mut provider = MutatingPolicyProvider {
            calls: RefCell::new(Vec::new()),
            show_all: false,
        };
        let mut renderer = render::Renderer::silent("");

        editor
            .dispatch(
                KeyEvent::Tab,
                &mut buffer,
                &mut history,
                &mut history_index,
                &mut saved_current,
                &mut provider,
                &mut renderer,
            )
            .unwrap();

        assert_eq!(*provider.calls.borrow(), ["policy", "complete"]);
    }

    #[test]
    fn completion_grid_lists_candidates_down_columns() {
        let matches = ["a", "b", "c", "d", "e"].map(str::to_string);

        assert_eq!(format_completion_rows(&matches, 9), "a  d\nb  e\nc  \n");
    }

    #[test]
    fn completion_mode_repeats_only_after_an_unchanged_completion() {
        let policy = CompletionDisplayPolicy::default();

        assert_eq!(
            completion_mode(None, false, policy),
            CompletionDisplayMode::Normal
        );
        assert_eq!(
            completion_mode(Some(EditAction::Complete), true, policy),
            CompletionDisplayMode::Normal
        );
        assert_eq!(
            completion_mode(Some(EditAction::Complete), false, policy),
            CompletionDisplayMode::List
        );
        assert_eq!(
            completion_mode(Some(EditAction::BackwardChar), false, policy),
            CompletionDisplayMode::Normal
        );
    }

    #[test]
    fn completion_mode_honors_readline_display_policies() {
        assert_eq!(
            completion_mode(
                None,
                false,
                CompletionDisplayPolicy {
                    show_all_if_ambiguous: true,
                    show_all_if_unmodified: false,
                },
            ),
            CompletionDisplayMode::ShowAll
        );
        assert_eq!(
            completion_mode(
                None,
                false,
                CompletionDisplayPolicy {
                    show_all_if_ambiguous: false,
                    show_all_if_unmodified: true,
                },
            ),
            CompletionDisplayMode::ShowUnmodified
        );
        assert_eq!(
            completion_mode(
                Some(EditAction::Complete),
                false,
                CompletionDisplayPolicy {
                    show_all_if_ambiguous: true,
                    show_all_if_unmodified: false,
                },
            ),
            CompletionDisplayMode::List
        );
    }

    #[test]
    fn history_arguments_follow_shell_quotes() {
        assert_eq!(
            shell_words("printf '%s %s' one\\ two\n"),
            ["printf", "%s %s", "one two"]
        );
    }

    #[test]
    fn vi_word_motion_produces_an_operator_range() {
        let mut buffer = EditBuffer::new();
        buffer.insert_str("one two");
        buffer.move_home();
        assert_eq!(vi_motion_range(&buffer, 'w', 1), Some((0, 3)));
    }
}
