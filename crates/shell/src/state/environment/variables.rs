macro_rules! environment_variable_accessors {
    () => {
        fn get(&self, name: &str) -> Option<String> {
            self.get_cow(name).map(Cow::into_owned)
        }

        fn get_cow<'a>(&'a self, name: &str) -> Option<Cow<'a, str>> {
            if let Ok(index) = name.parse::<usize>() {
                return self.positional_cow(index);
            }
            if name == "FUNCNAME" {
                return self.get_array_indexed_cow("FUNCNAME", 0);
            }
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE"
            ) {
                return self.get_array_indexed_cow(name, 0);
            }
            match name {
                "?" => return Some(Cow::Owned(self.last_command_exit_value.to_string())),
                "#" => {
                    return Some(Cow::Owned(
                        self.dollar_vars.len().saturating_sub(1).to_string(),
                    ))
                }
                "$" => {
                    let pid = self
                        .shell_pid_value
                        .unwrap_or_else(|| unsafe { libc::getpid() });
                    return Some(Cow::Owned(pid.to_string()));
                }
                "!" => {
                    return Some(Cow::Owned(
                        self.last_async_pid
                            .map(|p| p.to_string())
                            .unwrap_or_default(),
                    ))
                }
                "-" => return Some(Cow::Owned(self.option_letters())),
                "SHELLOPTS" => return Some(Cow::Owned(self.shellopts_value())),
                "BASHOPTS" => return Some(Cow::Owned(self.bashopts_value())),
                "BASHPID" => {
                    let pid = self
                        .bashpid_cache
                        .unwrap_or_else(|| unsafe { libc::getpid() });
                    return Some(Cow::Owned(pid.to_string()));
                }
                "BASH_ARGV0" if self.bash_argv0_dynamic => {
                    return self
                        .dollar_vars
                        .first()
                        .map(|value| Cow::Borrowed(value.as_str()));
                }
                "BASH_COMMAND" => {
                    if let Some(command) = self.current_command_string.as_ref() {
                        return Some(Cow::Borrowed(command.trim_end_matches('\n')));
                    }
                }
                "BASH_SUBSHELL" => return Some(Cow::Owned(self.subshell_level.to_string())),
                "BASH_TRAPSIG" => {
                    return Some(Cow::Owned(
                        self.running_trap_sig
                            .filter(|sig| *sig > 0)
                            .map(|sig| sig.to_string())
                            .unwrap_or_default(),
                    ));
                }
                "LINENO" => {
                    let line = self
                        .diagnostic_line_stack
                        .last()
                        .copied()
                        .or_else(|| {
                            (self.current_command_line_count > 0)
                                .then_some(self.current_command_line_count)
                        })
                        .unwrap_or(0);
                    return Some(Cow::Owned(line.to_string()));
                }
                "HISTCMD" => {
                    let value = if self.interactive_shell || self.option("history") {
                        self.history_table.base() + self.history_table.len()
                    } else {
                        0
                    };
                    return Some(Cow::Owned(value.to_string()));
                }
                "SECONDS" if self.seconds_dynamic => {
                    let elapsed = self.seconds_start.elapsed().as_secs() as i64;
                    return Some(Cow::Owned(
                        self.seconds_offset.saturating_add(elapsed).to_string(),
                    ));
                }
                "EPOCHSECONDS" if self.epochseconds_dynamic => {
                    return Some(Cow::Owned(current_epoch_seconds().to_string()));
                }
                "EPOCHREALTIME" if self.epochrealtime_dynamic => {
                    let (secs, micros) = current_epoch_realtime();
                    return Some(Cow::Owned(format!("{secs}.{micros:06}")));
                }
                "BASH_MONOSECONDS" if self.bash_monoseconds_dynamic => {
                    return Some(Cow::Owned(current_mono_seconds().to_string()));
                }
                "RANDOM" => loop {
                    let (next_seed, value) = bash_random_from_seed(self.random_seed.get());
                    self.random_seed.set(next_seed);
                    if value != self.last_random_value.get() {
                        self.last_random_value.set(value);
                        return Some(Cow::Owned(value.to_string()));
                    }
                },
                "SRANDOM" => {
                    let (next_seed, value) = bash_srandom_from_seed(self.srandom_seed.get());
                    self.srandom_seed.set(next_seed);
                    return Some(Cow::Owned(value.to_string()));
                }
                _ => {}
            }
            if let Some(entry) = self.variables.get(name) {
                if entry.attrs.contains(VarAttrs::NAMEREF)
                    && self.local_self_nameref_target(name).is_some()
                {
                    self.report_circular_name_reference(name);
                    return self
                        .global_saved_snapshot(name)
                        .and_then(Self::saved_scalar_value)
                        .map(Cow::Owned);
                }
                if entry.attrs.contains(VarAttrs::NAMEREF)
                    && self
                        .nameref_targets
                        .get(name)
                        .is_some_and(|target| target == name)
                {
                    self.report_circular_name_reference(name);
                    return None;
                }
                return entry
                    .has_value
                    .then_some(Cow::Borrowed(entry.value.as_str()));
            }
            std::env::var(name).ok().map(Cow::Owned)
        }

        fn set(&mut self, name: &str, value: String) {
            let _ = self.assign(name, value);
        }
    };
}

macro_rules! environment_variable_assignment {
    () => {
        fn assign(&mut self, name: &str, mut value: String) -> Result<(), AssignError> {
            match name {
                "RANDOM" => {
                    if let Ok(seed) = value.trim().parse::<i64>() {
                        self.random_seed.set(seed as u32);
                        self.last_random_value.set(0);
                        self.variables.insert(
                            name.to_string(),
                            VariableEntry {
                                value: seed.to_string(),
                                has_value: true,
                                exported: false,
                                readonly: false,
                                attrs: VarAttrs::INTEGER,
                            },
                        );
                    }
                    return Ok(());
                }
                "BASH_ARGV0" if self.bash_argv0_dynamic => {
                    if self.dollar_vars.is_empty() {
                        self.dollar_vars.push(value);
                    } else {
                        self.dollar_vars[0] = value;
                    }
                    return Ok(());
                }
                "SECONDS" if self.seconds_dynamic => {
                    self.seconds_offset = value.trim().parse::<i64>().unwrap_or(0);
                    self.seconds_start = Instant::now();
                    return Ok(());
                }
                "EPOCHSECONDS" if self.epochseconds_dynamic => return Ok(()),
                "EPOCHREALTIME" if self.epochrealtime_dynamic => return Ok(()),
                "BASH_MONOSECONDS" if self.bash_monoseconds_dynamic => return Ok(()),
                "FUNCNAME" => return Ok(()),
                "GROUPS" if self.groups_dynamic => return Ok(()),
                "BASHOPTS" | "SHELLOPTS" => return Err(AssignError::ReadOnly(name.to_string())),
                _ => {}
            }
            if name == "BASH_XTRACEFD" && !value.is_empty() && !xtrace_fd_value_is_valid(&value) {
                self.report_invalid_xtracefd(&value);
                self.variables.remove(name);
                std::env::remove_var(name);
                return Ok(());
            }
            if self.restricted && matches!(name, "PATH" | "SHELL") {
                return Err(AssignError::ReadOnly(name.to_string()));
            }
            if let Some(existing) = self.variables.get(name) {
                if existing.attrs.contains(VarAttrs::READONLY) || existing.readonly {
                    return Err(AssignError::ReadOnly(name.to_string()));
                }
            }
            let attrs = self
                .variables
                .get(name)
                .map(|e| e.attrs)
                .unwrap_or_default();
            if attrs.contains(VarAttrs::NAMEREF) {
                if self.nameref_targets.contains_key(name) {
                    if self.local_self_nameref_target(name).is_some() {
                        self.report_max_nameref_depth(name);
                        return self.with_global_var(name, |state| {
                            state.assign_nameref_target(name, name, value)
                        });
                    }
                    let Some(target) = self.resolve_nameref(name) else {
                        self.report_circular_name_reference(name);
                        return Err(AssignError::CircularNameReference(name.to_string()));
                    };
                    if target != name {
                        return self.assign_nameref_target(name, &target, value);
                    }
                } else {
                    if !state_nameref_target_is_valid(&value) {
                        return Err(AssignError::InvalidName(value));
                    }
                    self.nameref_targets.insert(name.to_string(), value.clone());
                    let (exported, prev_attrs) = self
                        .variables
                        .get(name)
                        .map(|e| (e.exported, e.attrs))
                        .unwrap_or((false, VarAttrs::empty()));
                    self.variables.insert(
                        name.to_string(),
                        VariableEntry {
                            value,
                            has_value: true,
                            exported,
                            readonly: false,
                            attrs: prev_attrs | VarAttrs::NAMEREF,
                        },
                    );
                    return Ok(());
                }
            }
            if attrs.contains(VarAttrs::ASSOC) {
                self.set_array_assoc(name, "0", value);
                return Ok(());
            }
            if attrs.contains(VarAttrs::ARRAY) {
                self.set_array_indexed(name, 0, value);
                return Ok(());
            }
            if attrs.contains(VarAttrs::INTEGER) {
                value = match value.trim().parse::<i64>() {
                    Ok(n) => n.to_string(),
                    Err(_) => {
                        // Fall back to 0 on non-numeric just like bash with strict
                        // checking off; full arith eval lives in expander::arith.
                        "0".to_string()
                    }
                };
            }
            value = apply_case_attrs(value, attrs);
            let (exported, prev_attrs) = self
                .variables
                .get(name)
                .map(|e| (e.exported, e.attrs))
                .unwrap_or((false, VarAttrs::empty()));
            let readonly = self
                .variables
                .get(name)
                .map(|e| e.readonly)
                .unwrap_or(false);
            self.variables.insert(
                name.to_string(),
                VariableEntry {
                    value: value.clone(),
                    has_value: true,
                    exported,
                    readonly,
                    attrs: prev_attrs,
                },
            );
            if name == "BASH_COMPAT" {
                let level = compatibility_level(&value);
                for option in COMPAT_SHOPT_OPTIONS {
                    self.shopt_options.insert((*option).to_string(), false);
                }
                if let Some(option) = level.and_then(compatibility_option) {
                    self.shopt_options.insert(option.to_string(), true);
                }
            }
            self.apply_history_special_var(name, &value);
            if name == "IGNOREEOF" {
                self.shopt_options.insert("ignoreeof".to_string(), true);
            }
            if exported {
                std::env::set_var(name, &value);
            }
            Ok(())
        }

        fn is_readonly(&self, name: &str) -> bool {
            if matches!(name, "BASHOPTS" | "SHELLOPTS") {
                return true;
            }
            if self.restricted && matches!(name, "PATH" | "SHELL") {
                return true;
            }
            self.variables
                .get(name)
                .map(|e| e.readonly || e.attrs.contains(VarAttrs::READONLY))
                .unwrap_or(false)
        }

        fn unset(&mut self, name: &str) {
            match name {
                "BASH_ARGV0" => self.bash_argv0_dynamic = false,
                "SECONDS" => self.seconds_dynamic = false,
                "EPOCHSECONDS" => self.epochseconds_dynamic = false,
                "EPOCHREALTIME" => self.epochrealtime_dynamic = false,
                "BASH_MONOSECONDS" => self.bash_monoseconds_dynamic = false,
                "GROUPS" => self.groups_dynamic = false,
                "FUNCNAME" => return,
                "HISTFILE" => {
                    self.histfile_explicit = true;
                    self.histfile = None;
                }
                "IGNOREEOF" => {
                    self.shopt_options.insert("ignoreeof".to_string(), false);
                }
                "BASHOPTS" | "SHELLOPTS" => return,
                _ => {}
            }
            let current_scope_has_name = self
                .local_scopes
                .last()
                .is_some_and(|scope| scope.contains_key(name));
            if !current_scope_has_name {
                if let Some(scope_idx) = self
                    .local_scopes
                    .iter()
                    .rposition(|scope| scope.contains_key(name))
                {
                    if self.option("localvar_unset") {
                        self.variables
                            .insert(name.to_string(), VariableEntry::default());
                        self.indexed_arrays.remove(name);
                        self.assoc_arrays.remove(name);
                        self.assoc_print_orders.remove(name);
                        self.nameref_targets.remove(name);
                        self.sync_export_env(name);
                        return;
                    }
                    if let Some(saved) = self.local_scopes[scope_idx].remove(name) {
                        self.restore_var_snapshot(name, saved);
                        self.sync_export_env(name);
                        return;
                    }
                }
            } else {
                let inherit_export = self
                    .local_scopes
                    .last()
                    .and_then(|scope| scope.get(name))
                    .and_then(|saved| saved.entry.as_ref())
                    .is_some_and(|entry| entry.exported);
                let mut entry = VariableEntry::default();
                if inherit_export {
                    entry.exported = true;
                    entry.attrs.insert(VarAttrs::EXPORT);
                }
                self.variables.insert(name.to_string(), entry);
                self.indexed_arrays.remove(name);
                self.assoc_arrays.remove(name);
                self.assoc_print_orders.remove(name);
                self.nameref_targets.remove(name);
                self.sync_export_env(name);
                return;
            }
            self.variables.remove(name);
            self.indexed_arrays.remove(name);
            self.assoc_arrays.remove(name);
            self.assoc_print_orders.remove(name);
            self.nameref_targets.remove(name);
            std::env::remove_var(name);
        }

        fn exported(&self, name: &str) -> bool {
            self.variables
                .get(name)
                .map(|entry| entry.exported)
                .unwrap_or(false)
        }

        fn export(&mut self, name: &str) {
            let value = self.get(name);
            let attrs = self
                .variables
                .get(name)
                .map(|e| e.attrs)
                .unwrap_or_default()
                | VarAttrs::EXPORT;
            let readonly = self
                .variables
                .get(name)
                .map(|e| e.readonly)
                .unwrap_or(false);
            self.variables.insert(
                name.to_string(),
                VariableEntry {
                    value: value.clone().unwrap_or_default(),
                    has_value: value.is_some(),
                    exported: true,
                    readonly,
                    attrs,
                },
            );
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    };
}

macro_rules! environment_positionals {
    () => {
        fn positional(&self, index: usize) -> Option<String> {
            self.positional_cow(index).map(Cow::into_owned)
        }

        fn positional_cow<'a>(&'a self, index: usize) -> Option<Cow<'a, str>> {
            self.dollar_vars
                .get(index)
                .map(|value| Cow::Borrowed(value.as_str()))
        }

        fn positional_count(&self) -> usize {
            self.dollar_vars.len().saturating_sub(1)
        }

        fn set_positionals(&mut self, params: Vec<String>) {
            self.dollar_vars = params;
        }

        fn push_function_positionals(&mut self, args: &[String]) -> Vec<String> {
            let saved = std::mem::take(&mut self.dollar_vars);
            let zero = saved
                .first()
                .cloned()
                .unwrap_or_else(|| "cherubsh".to_string());
            self.dollar_vars = Vec::with_capacity(args.len() + 1);
            self.dollar_vars.push(zero);
            self.dollar_vars.extend(args.iter().cloned());
            saved
        }

        fn pop_function_positionals(&mut self, mut saved: Vec<String>) {
            if let Some(current_zero) = self.dollar_vars.first().cloned() {
                if saved.is_empty() {
                    saved.push(current_zero);
                } else {
                    saved[0] = current_zero;
                }
            }
            self.dollar_vars = saved;
        }
    };
}
