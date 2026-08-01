macro_rules! environment_traps {
    () => {
        fn trap_set(&mut self, signal: &str, action: Option<String>) {
            let key = canonical_trap_signal(signal);
            match action {
                Some(act) => {
                    self.traps.insert(key, act);
                }
                None => {
                    self.traps.remove(&key);
                }
            }
        }
        fn trap_get(&self, signal: &str) -> Option<String> {
            let key = canonical_trap_signal(signal);
            self.traps
                .get(&key)
                .cloned()
                .or_else(|| startup_ignored_trap_action(&key))
        }
        fn trap_iter(&self) -> Vec<TrapEntry> {
            let mut entries: Vec<TrapEntry> = self
                .traps
                .iter()
                .map(|(k, v)| TrapEntry {
                    signal: k.clone(),
                    action: v.clone(),
                })
                .collect();
            for sig in crate::signals::startup_ignored_signals() {
                if let Some(short) = signal_short_name(sig) {
                    if !self.traps.contains_key(short) {
                        entries.push(TrapEntry {
                            signal: short.to_string(),
                            action: String::new(),
                        });
                    }
                }
            }
            entries
        }
    };
}
