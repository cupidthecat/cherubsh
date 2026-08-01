macro_rules! environment_trap_actions {
    () => {
    fn trap_action(&self, kind: TrapKind) -> Option<TrapAction> {
        self.trap_actions.get(&kind).cloned().or_else(|| {
            let TrapKind::Numeric(sig) = kind else {
                return None;
            };
            if sig != libc::SIGPIPE && crate::signals::signal_ignored_at_start(sig) {
                Some(TrapAction::Ignore)
            } else {
                None
            }
        })
    }
    fn trap_set_action(&mut self, kind: TrapKind, action: TrapAction) {
        if kind == TrapKind::Exit {
            self.inherited_exit_trap_suppressed = false;
        }
        let label = match kind {
            TrapKind::Numeric(sig) => canonical_trap_signal(&sig.to_string()),
            _ => kind.as_str().to_string(),
        };
        if let Some(sig) = kind.as_signal() {
            if crate::signals::signal_ignored_at_start(sig) {
                self.traps.insert(label, String::new());
                self.trap_actions.insert(kind, TrapAction::Ignore);
                return;
            }
            crate::signals::configure_trap_signal(
                sig,
                Some(&action),
                self.interactive,
                self.job_control,
            );
        }
        match &action {
            TrapAction::Default => {
                self.traps.remove(&label);
                self.trap_actions.remove(&kind);
            }
            TrapAction::Ignore => {
                self.traps.insert(label.clone(), String::new());
                self.trap_actions.insert(kind, action);
            }
            TrapAction::Command(cmd) => {
                self.traps.insert(label.clone(), cmd.clone());
                self.trap_actions.insert(kind, action);
            }
        }
    }
    fn trap_clear(&mut self, kind: TrapKind) {
        if kind == TrapKind::Exit {
            self.inherited_exit_trap_suppressed = false;
        }
        let label = match kind {
            TrapKind::Numeric(sig) => canonical_trap_signal(&sig.to_string()),
            _ => kind.as_str().to_string(),
        };
        if let Some(sig) = kind.as_signal() {
            if crate::signals::signal_ignored_at_start(sig) {
                self.traps.insert(label, String::new());
                self.trap_actions.insert(kind, TrapAction::Ignore);
                return;
            }
            crate::signals::configure_trap_signal(sig, None, self.interactive, self.job_control);
        }
        self.traps.remove(&label);
        self.trap_actions.remove(&kind);
    }
    fn trap_all(&self) -> Vec<(TrapKind, TrapAction)> {
        let mut entries: Vec<(TrapKind, TrapAction)> = self
            .trap_actions
            .iter()
            .map(|(k, a)| (*k, a.clone()))
            .collect();
        for sig in crate::signals::startup_ignored_signals() {
            let kind = TrapKind::Numeric(sig);
            if !self.trap_actions.contains_key(&kind) {
                entries.push((kind, TrapAction::Ignore));
            }
        }
        entries
    }
    fn suppress_inherited_exit_trap(&mut self) {
        if self.trap_actions.contains_key(&TrapKind::Exit) {
            self.inherited_exit_trap_suppressed = true;
        }
    }
    fn inherited_exit_trap_suppressed(&self) -> bool {
        self.inherited_exit_trap_suppressed
    }
    fn trap_is_set(&self, kind: TrapKind) -> bool {
        self.trap_actions.contains_key(&kind)
    }
    };
}
