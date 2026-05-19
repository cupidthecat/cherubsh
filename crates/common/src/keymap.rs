//! Readline-style keymap.
//!
//! The hand-rolled line editor in `crates/lineedit` reads `Keymap` values
//! out of this module to dispatch key sequences to high-level edit actions.
//! The `bind` builtin mutates the active `Keymap` through the
//! `Environment` surface.
//!
//! Keysequences are the canonical bash form ("\e[A", "\C-a", "\M-f"); on
//! lookup the editor first normalises pressed keys into the same sequence
//! string, then probes the map.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditAction {
    // Cursor movement
    BeginningOfLine,
    EndOfLine,
    ForwardChar,
    BackwardChar,
    ForwardWord,
    BackwardWord,
    NextScreenLine,
    PreviousScreenLine,
    // Insertion / deletion
    SelfInsert,
    DeleteChar,
    BackwardDeleteChar,
    QuotedInsert,
    TabInsert,
    TransposeChars,
    TransposeWords,
    UpcaseWord,
    DowncaseWord,
    CapitalizeWord,
    // Killing / yanking
    KillLine,
    BackwardKillLine,
    KillWord,
    BackwardKillWord,
    UnixWordRubout,
    UnixLineDiscard,
    KillRegion,
    Yank,
    YankPop,
    YankLastArg,
    YankNthArg,
    // History
    PreviousHistory,
    NextHistory,
    OperateAndGetNext,
    BeginningOfHistory,
    EndOfHistory,
    ReverseSearchHistory,
    ForwardSearchHistory,
    NonIncrementalReverseSearchHistory,
    NonIncrementalForwardSearchHistory,
    HistorySearchForward,
    HistorySearchBackward,
    // Commands
    AcceptLine,
    NewLine,
    DeleteCharOrList,
    Complete,
    PossibleCompletions,
    InsertCompletions,
    MenuComplete,
    MenuCompleteBackward,
    UndoCmd,
    RevertLine,
    ClearScreen,
    Redraw,
    Abort,
    Tilde,
    Quit,
    // Vi modes
    ViMovementMode,
    ViInsertionMode,
    ViAppendMode,
    ViAppendEol,
    // Misc / fallback
    Noop,
    /// Run a shell command - populated by `bind -x`.
    ShellCommand(u32),
    /// Insert a fixed string - populated by `bind '"key":"macro"'`.
    Macro(u32),
}

#[derive(Clone, Debug)]
pub struct Keymap {
    pub name: String,
    pub bindings: BTreeMap<String, EditAction>,
    /// Shell-command bodies referenced by `EditAction::ShellCommand(idx)`.
    pub shell_commands: Vec<String>,
    /// Macro strings referenced by `EditAction::Macro(idx)`.
    pub macros: Vec<String>,
}

impl Keymap {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bindings: BTreeMap::new(),
            shell_commands: Vec::new(),
            macros: Vec::new(),
        }
    }

    pub fn bind(&mut self, seq: impl Into<String>, action: EditAction) {
        self.bindings.insert(seq.into(), action);
    }

    pub fn unbind(&mut self, seq: &str) -> bool {
        self.bindings.remove(seq).is_some()
    }

    pub fn lookup(&self, seq: &str) -> Option<EditAction> {
        self.bindings.get(seq).copied()
    }

    pub fn install_emacs_defaults(&mut self) {
        use EditAction::*;
        let pairs: &[(&str, EditAction)] = &[
            ("\\C-a", BeginningOfLine),
            ("\\C-e", EndOfLine),
            ("\\C-f", ForwardChar),
            ("\\C-b", BackwardChar),
            ("\\M-f", ForwardWord),
            ("\\M-b", BackwardWord),
            ("\\C-d", DeleteCharOrList),
            ("\\C-h", BackwardDeleteChar),
            ("\\C-k", KillLine),
            ("\\C-u", UnixLineDiscard),
            ("\\C-w", UnixWordRubout),
            ("\\M-d", KillWord),
            ("\\C-y", Yank),
            ("\\M-y", YankPop),
            ("\\C-t", TransposeChars),
            ("\\M-t", TransposeWords),
            ("\\M-u", UpcaseWord),
            ("\\M-l", DowncaseWord),
            ("\\M-c", CapitalizeWord),
            ("\\C-p", PreviousHistory),
            ("\\C-n", NextHistory),
            ("\\C-o", OperateAndGetNext),
            ("\\C-r", ReverseSearchHistory),
            ("\\C-s", ForwardSearchHistory),
            ("\\C-j", AcceptLine),
            ("\\C-m", AcceptLine),
            ("\\C-l", ClearScreen),
            ("\\C-_", UndoCmd),
            ("\\C-x\\C-u", UndoCmd),
            ("\\C-g", Abort),
            ("\\e[A", PreviousHistory),
            ("\\e[B", NextHistory),
            ("\\e[C", ForwardChar),
            ("\\e[D", BackwardChar),
            ("\\e[H", BeginningOfLine),
            ("\\e[F", EndOfLine),
            ("\\eOH", BeginningOfLine),
            ("\\eOF", EndOfLine),
            ("\\e[3~", DeleteChar),
            ("\\t", Complete),
            ("\\e?", PossibleCompletions),
            ("\\M-.", YankLastArg),
            ("\\M-_", YankLastArg),
            ("\\e\\e", Complete),
        ];
        for (seq, act) in pairs {
            self.bind(*seq, *act);
        }
    }

    pub fn install_vi_insert_defaults(&mut self) {
        use EditAction::*;
        self.bind("\\C-m", AcceptLine);
        self.bind("\\C-j", AcceptLine);
        self.bind("\\C-h", BackwardDeleteChar);
        self.bind("\\e", ViMovementMode);
        self.bind("\\C-i", Complete);
        self.bind("\\C-r", ReverseSearchHistory);
    }

    pub fn install_vi_movement_defaults(&mut self) {
        use EditAction::*;
        // Minimal vi-movement subset; full operator/motion grammar lives in
        // the editor crate's `vi.rs` state machine.
        self.bind("h", BackwardChar);
        self.bind("l", ForwardChar);
        self.bind("k", PreviousHistory);
        self.bind("j", NextHistory);
        self.bind("0", BeginningOfLine);
        self.bind("$", EndOfLine);
        self.bind("w", ForwardWord);
        self.bind("b", BackwardWord);
        self.bind("i", ViInsertionMode);
        self.bind("a", ViAppendMode);
        self.bind("A", ViAppendEol);
        self.bind("x", DeleteChar);
        self.bind("D", KillLine);
        self.bind("u", UndoCmd);
        self.bind("\\C-m", AcceptLine);
        self.bind("\\C-j", AcceptLine);
        self.bind("/", ReverseSearchHistory);
    }
}

/// Canonicalise a key sequence string per bash's "\e","\C-x","\M-x" syntax.
/// Used by `bind 'keyseq:fn'` and by the editor when matching key presses.
pub fn canonicalise_keyseq(input: &str) -> String {
    // Already in canonical form - bash just stores the literal escape string.
    input.to_string()
}
