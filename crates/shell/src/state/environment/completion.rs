macro_rules! environment_completion {
    () => {
    fn compspec_set(&mut self, slot: CompSlot, key: Option<&str>, spec: CompSpec) {
        self.compspec_initialized = true;
        match slot {
            CompSlot::Command => {
                if let Some(k) = key {
                    if !self.compspec_order.contains_key(k) {
                        self.compspec_order
                            .insert(k.to_string(), self.compspec_next_order);
                        self.compspec_next_order = self.compspec_next_order.saturating_add(1);
                    }
                    self.compspecs.insert(k.to_string(), spec);
                }
            }
            CompSlot::Default => self.default_compspec = Some(spec),
            CompSlot::Initial => self.initial_compspec = Some(spec),
            CompSlot::Empty => self.empty_compspec = Some(spec),
        }
    }
    fn compspec_get(&self, slot: CompSlot, key: Option<&str>) -> Option<CompSpec> {
        match slot {
            CompSlot::Command => key.and_then(|k| self.compspecs.get(k).cloned()),
            CompSlot::Default => self.default_compspec.clone(),
            CompSlot::Initial => self.initial_compspec.clone(),
            CompSlot::Empty => self.empty_compspec.clone(),
        }
    }
    fn compspec_remove(&mut self, slot: CompSlot, key: Option<&str>) -> bool {
        if !self.compspec_initialized {
            return true;
        }
        match slot {
            CompSlot::Command => key
                .map(|k| {
                    self.compspec_order.remove(k);
                    self.compspecs.remove(k).is_some()
                })
                .unwrap_or(false),
            CompSlot::Default => self.default_compspec.take().is_some(),
            CompSlot::Initial => self.initial_compspec.take().is_some(),
            CompSlot::Empty => self.empty_compspec.take().is_some(),
        }
    }
    fn compspec_iter(&self) -> Vec<(CompSlot, Option<String>, CompSpec)> {
        let mut commands: Vec<(String, CompSpec, u64)> = self
            .compspecs
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.clone(),
                    self.compspec_order.get(k).copied().unwrap_or(0),
                )
            })
            .collect();
        commands.sort_by(|(left_key, _, left_order), (right_key, _, right_order)| {
            bash_progcomp_bucket(left_key)
                .cmp(&bash_progcomp_bucket(right_key))
                .then_with(|| right_order.cmp(left_order))
                .then_with(|| left_key.cmp(right_key))
        });
        let mut out: Vec<(CompSlot, Option<String>, CompSpec)> = commands
            .into_iter()
            .map(|(k, v, _)| (CompSlot::Command, Some(k), v))
            .collect();
        if let Some(d) = &self.default_compspec {
            out.push((CompSlot::Default, None, d.clone()));
        }
        if let Some(i) = &self.initial_compspec {
            out.push((CompSlot::Initial, None, i.clone()));
        }
        if let Some(e) = &self.empty_compspec {
            out.push((CompSlot::Empty, None, e.clone()));
        }
        out
    }

    fn completion_options_update(
        &mut self,
        set: cherubsh_common::completion::CompOpts,
        clear: cherubsh_common::completion::CompOpts,
    ) -> bool {
        let Some(options) = self.active_completion_options.as_mut() else {
            return false;
        };
        *options |= set;
        *options &= !clear;
        true
    }

    fn completion_options_current(
        &self,
    ) -> Option<(String, cherubsh_common::completion::CompOpts)> {
        Some((
            self.active_completion_command.clone()?,
            self.active_completion_options?,
        ))
    }

    fn keymap_active(&self) -> &str {
        &self.active_keymap
    }
    fn keymap_set_active(&mut self, name: &str) {
        if self.keymaps.contains_key(name) || default_keymap(name).is_some() {
            self.active_keymap = name.to_string();
        }
    }
    fn keymap_get(&self, name: &str) -> Option<Keymap> {
        self.keymaps
            .get(name)
            .cloned()
            .or_else(|| default_keymap(name))
    }
    fn keymap_bind(&mut self, name: &str, seq: &str, action: EditAction) {
        let entry = self
            .keymaps
            .entry(name.to_string())
            .or_insert_with(|| default_keymap(name).unwrap_or_else(|| Keymap::new(name)));
        entry.bind(seq, action);
    }
    fn keymap_bind_macro(&mut self, name: &str, seq: &str, text: &str) {
        let entry = self
            .keymaps
            .entry(name.to_string())
            .or_insert_with(|| default_keymap(name).unwrap_or_else(|| Keymap::new(name)));
        let idx = entry.macros.len() as u32;
        entry.macros.push(text.to_string());
        entry.bind(seq, EditAction::Macro(idx));
    }
    fn keymap_bind_shell_command(&mut self, name: &str, seq: &str, command: &str) {
        let entry = self
            .keymaps
            .entry(name.to_string())
            .or_insert_with(|| default_keymap(name).unwrap_or_else(|| Keymap::new(name)));
        let idx = entry.shell_commands.len() as u32;
        entry.shell_commands.push(command.to_string());
        entry.bind(seq, EditAction::ShellCommand(idx));
    }
    fn keymap_unbind(&mut self, name: &str, seq: &str) -> bool {
        if !self.keymaps.contains_key(name) {
            if let Some(default) = default_keymap(name) {
                self.keymaps.insert(name.to_string(), default);
            }
        }
        self.keymaps.get_mut(name).is_some_and(|k| k.unbind(seq))
    }
    fn keymap_list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.keymaps.keys().cloned().collect();
        for name in ["emacs", "vi-command", "vi-insert"] {
            if !v.iter().any(|existing| existing == name) {
                v.push(name.to_string());
            }
        }
        v.sort();
        v
    }
    };
}
