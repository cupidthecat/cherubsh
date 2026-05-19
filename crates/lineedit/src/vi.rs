//! Vi-mode operator/motion state machine.
//!
//! Lives here as a focused submodule - the main dispatch loop in `lib.rs`
//! handles emacs-style commands directly via the keymap, and switches into
//! vi-movement mode when the editor receives `EditAction::ViMovementMode`.
//!
//! The full vi grammar (operators × motions × text-objects × counts) is
//! large; this skeleton implements the common subset (`h`/`l`/`0`/`$`/`w`/`b`,
//! `x`, `dd`/`D`, `i`/`I`/`a`/`A`, `u`, `cw`/`dw`, `f<c>`/`F<c>`/`t<c>`/`T<c>`).

#[derive(Default, Debug)]
pub struct ViState {
    pub pending_op: Option<Op>,
    pub pending_motion_arg: Option<char>,
    pub count: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub enum Op {
    Delete,
    Change,
    Yank,
}

impl ViState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.pending_op = None;
        self.pending_motion_arg = None;
        self.count = None;
    }

    pub fn pump_digit(&mut self, d: u32) {
        self.count = Some(self.count.unwrap_or(0) * 10 + d);
    }

    pub fn effective_count(&self) -> u32 {
        self.count.unwrap_or(1)
    }
}
