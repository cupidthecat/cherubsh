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

    pub fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(entry) = self.entries.get_mut(self.head) {
            entry.push_str(text);
        } else {
            self.push(text.to_string());
        }
    }

    pub fn prepend(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(entry) = self.entries.get_mut(self.head) {
            entry.insert_str(0, text);
        } else {
            self.push(text.to_string());
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for KillRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::KillRing;

    #[test]
    fn consecutive_kills_can_join_in_either_direction() {
        let mut ring = KillRing::new();
        ring.push("middle".to_string());
        ring.append(" tail");
        ring.prepend("head ");
        assert_eq!(ring.current(), Some("head middle tail"));
    }

    #[test]
    fn rotation_visits_older_entries() {
        let mut ring = KillRing::new();
        ring.push("first".to_string());
        ring.push("second".to_string());
        ring.rotate();
        assert_eq!(ring.current(), Some("first"));
    }
}
