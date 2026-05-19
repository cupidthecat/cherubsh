//! Stub termcap module - we emit ANSI escapes directly in render.rs.
//!
//! A full terminfo/termcap loader (link `ncurses` and call `tigetstr`) would
//! live here. For the purpose of running inside any reasonably modern
//! terminal emulator the ANSI escape constants below are sufficient.

pub const CLEAR_LINE: &str = "\x1b[2K";
pub const CLEAR_TO_EOL: &str = "\x1b[K";
pub const CLEAR_TO_EOS: &str = "\x1b[J";
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
pub const CURSOR_UP: &str = "\x1b[A";
pub const CURSOR_DOWN: &str = "\x1b[B";
pub const CURSOR_RIGHT: &str = "\x1b[C";
pub const CURSOR_LEFT: &str = "\x1b[D";
pub const SAVE_CURSOR: &str = "\x1b[s";
pub const RESTORE_CURSOR: &str = "\x1b[u";
pub const BEL: u8 = 0x07;
