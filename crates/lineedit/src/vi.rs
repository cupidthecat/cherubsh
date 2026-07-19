//! State used while the vi command keymap is active.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    Operator(Op),
    Find { backward: bool, till: bool },
    Replace,
}

#[derive(Default, Debug)]
pub struct ViState {
    pub pending: Option<Pending>,
    pub count: Option<usize>,
}

impl ViState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.pending = None;
        self.count = None;
    }

    pub fn push_digit(&mut self, digit: usize) {
        self.count = Some(self.count.unwrap_or(0).saturating_mul(10) + digit);
    }

    pub fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::ViState;

    #[test]
    fn counts_accumulate_and_reset_after_use() {
        let mut state = ViState::new();
        state.push_digit(1);
        state.push_digit(2);
        assert_eq!(state.take_count(), 12);
        assert_eq!(state.take_count(), 1);
    }
}
