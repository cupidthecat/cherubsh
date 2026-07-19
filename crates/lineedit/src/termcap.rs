//! ECMA-48 control sequences used by the terminal renderer.

pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
pub const BEL: u8 = 0x07;
