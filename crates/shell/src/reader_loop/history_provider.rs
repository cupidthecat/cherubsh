struct HistorySnapshot {
    entries: Vec<String>,
}

impl HistorySnapshot {
    fn from_state(state: &ShellState) -> Self {
        Self {
            entries: state
                .history_table
                .iter()
                .map(|entry| entry.line.clone())
                .collect(),
        }
    }
}

impl HistoryProvider for HistorySnapshot {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, idx: usize) -> Option<String> {
        self.entries.get(idx).cloned()
    }
}
