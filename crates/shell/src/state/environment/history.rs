macro_rules! environment_history {
    () => {
    fn history(&self) -> Option<&HistoryTable> {
        Some(&self.history_table)
    }
    fn history_mut(&mut self) -> Option<&mut HistoryTable> {
        Some(&mut self.history_table)
    }
    fn history_last_line_added(&self) -> bool {
        self.history_last_line_added
    }
    fn histcontrol(&self) -> HistControl {
        self.histcontrol_flags
    }
    fn histfile(&self) -> Option<PathBuf> {
        if matches!(std::env::var_os("HISTFILE"), Some(value) if value.is_empty()) {
            return None;
        }
        if self.histfile_explicit {
            return self.histfile.clone();
        }
        self.histfile.clone()
    }
    };
}
