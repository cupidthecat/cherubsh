//! Emacs-style circular kill ring.

pub struct KillRing {
    entries: Vec<String>,
    head: usize,
    capacity: usize,
}

impl KillRing {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            head: 0,
            capacity: 8,
        }
    }

    pub fn push(&mut self, s: String) {
        if s.is_empty() {
            return;
        }
        if self.entries.len() < self.capacity {
            self.entries.push(s);
            self.head = self.entries.len() - 1;
        } else {
            self.head = (self.head + 1) % self.capacity;
            self.entries[self.head] = s;
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.entries.get(self.head).map(|s| s.as_str())
    }

    pub fn rotate(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if self.head == 0 {
            self.head = self.entries.len() - 1;
        } else {
            self.head -= 1;
        }
    }
}

impl Default for KillRing {
    fn default() -> Self {
        Self::new()
    }
}
