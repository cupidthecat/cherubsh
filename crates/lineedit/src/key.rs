//! Decoded keyboard input.

use cherubsh_common::keymap::{EditAction, Keymap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Char(char),
    Ctrl(char),
    Meta(char),
    Function(u8),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    Backspace,
    Tab,
    Enter,
    Esc,
    Paste(String),
    Raw(String),
}

impl KeyEvent {
    /// Convert to the canonical bash key-sequence string used in `bind`
    /// (e.g. "\\C-a", "\\e[A"). The keymap is keyed on these strings.
    pub fn to_sequence(&self) -> String {
        match self {
            KeyEvent::Char(c) => c.to_string(),
            KeyEvent::Ctrl(c) => format!("\\C-{}", c.to_ascii_lowercase()),
            KeyEvent::Meta(c) => format!("\\M-{}", c),
            KeyEvent::Function(n) => format!("\\eO{}", (b'P' + (n - 1)) as char),
            KeyEvent::Up => "\\e[A".to_string(),
            KeyEvent::Down => "\\e[B".to_string(),
            KeyEvent::Right => "\\e[C".to_string(),
            KeyEvent::Left => "\\e[D".to_string(),
            KeyEvent::Home => "\\e[H".to_string(),
            KeyEvent::End => "\\e[F".to_string(),
            KeyEvent::PageUp => "\\e[5~".to_string(),
            KeyEvent::PageDown => "\\e[6~".to_string(),
            KeyEvent::Delete => "\\e[3~".to_string(),
            KeyEvent::Insert => "\\e[2~".to_string(),
            KeyEvent::Backspace => "\\C-h".to_string(),
            KeyEvent::Tab => "\\t".to_string(),
            KeyEvent::Enter => "\\C-m".to_string(),
            KeyEvent::Esc => "\\e".to_string(),
            KeyEvent::Paste(_) => String::new(),
            KeyEvent::Raw(s) => s.clone(),
        }
    }

    pub fn lookup_in(&self, keymap: &Keymap) -> Option<EditAction> {
        keymap.lookup(&self.to_sequence())
    }
}
