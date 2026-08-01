macro_rules! environment_locals {
    () => {
    fn resolve_nameref(&self, name: &str) -> Option<String> {
        let mut cur = name.to_string();
        let mut seen = HashSet::default();
        seen.insert(cur.clone());
        for _ in 0..NAMEREF_MAX_DEPTH {
            match self.nameref_targets.get(&cur) {
                Some(target) if target != &cur => {
                    if !seen.insert(target.clone()) {
                        return None;
                    }
                    cur = target.clone();
                    if !self.nameref_targets.contains_key(&cur) {
                        return Some(cur);
                    }
                }
                _ => return Some(cur),
            }
        }
        None
    }

    fn push_local_scope(&mut self) {
        self.local_scopes.push(HashMap::default());
        self.local_option_scopes.push(None);
    }

    fn pop_local_scope(&mut self) {
        if let Some(saved) = self.local_option_scopes.pop().flatten() {
            self.restore_options(saved);
        }
        let Some(scope) = self.local_scopes.pop() else {
            return;
        };
        for (name, saved) in scope {
            match saved.entry {
                Some(e) => {
                    self.variables.insert(name.clone(), e);
                }
                None => {
                    self.variables.remove(&name);
                }
            }
            match saved.indexed {
                Some(bt) => {
                    self.indexed_arrays.insert(name.clone(), bt);
                }
                None => {
                    self.indexed_arrays.remove(&name);
                }
            }
            match saved.assoc {
                Some(bt) => {
                    self.assoc_arrays.insert(name.clone(), bt);
                    self.assoc_print_orders.remove(&name);
                }
                None => {
                    self.assoc_arrays.remove(&name);
                    self.assoc_print_orders.remove(&name);
                }
            }
            match saved.nameref {
                Some(t) => {
                    self.nameref_targets.insert(name.clone(), t);
                }
                None => {
                    self.nameref_targets.remove(&name);
                }
            }
            self.sync_export_env(&name);
        }
    }

    fn set_local(&mut self, name: &str, value: String) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.entry(name.to_string()).or_insert_with(|| SavedVar {
                entry: self.variables.get(name).cloned(),
                indexed: self.indexed_arrays.get(name).cloned(),
                assoc: self.assoc_arrays.get(name).cloned(),
                nameref: self.nameref_targets.get(name).cloned(),
            });
        }
        self.set(name, value);
    }

    fn make_local(&mut self, name: &str) -> Result<(), AssignError> {
        if self.local_scopes.is_empty() {
            return Ok(());
        }
        if self
            .local_scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name))
        {
            return Ok(());
        }
        let shadows_outer_local = self
            .local_scopes
            .iter()
            .any(|scope| scope.contains_key(name));
        if !shadows_outer_local && self.variables.get(name).is_some_and(|entry| entry.readonly) {
            return Err(AssignError::ReadOnly(name.to_string()));
        }
        let saved = SavedVar {
            entry: self.variables.get(name).cloned(),
            indexed: self.indexed_arrays.get(name).cloned(),
            assoc: self.assoc_arrays.get(name).cloned(),
            nameref: self.nameref_targets.get(name).cloned(),
        };
        let inherit_export = saved.entry.as_ref().is_some_and(|entry| entry.exported);
        self.local_scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), saved);
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
        Ok(())
    }

    fn make_local_with_value(
        &mut self,
        name: &str,
        value: Option<String>,
    ) -> Result<(), AssignError> {
        if self.local_scopes.is_empty() {
            if let Some(value) = value {
                self.assign(name, value)?;
            }
            return Ok(());
        }

        let already_local = self
            .local_scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name));
        let mut inherited_export = false;
        if !already_local {
            let shadows_outer_local = self
                .local_scopes
                .iter()
                .any(|scope| scope.contains_key(name));
            if !shadows_outer_local && self.variables.get(name).is_some_and(|entry| entry.readonly)
            {
                return Err(AssignError::ReadOnly(name.to_string()));
            }
            let saved = SavedVar {
                entry: self.variables.get(name).cloned(),
                indexed: self.indexed_arrays.get(name).cloned(),
                assoc: self.assoc_arrays.get(name).cloned(),
                nameref: self.nameref_targets.get(name).cloned(),
            };
            inherited_export = saved.entry.as_ref().is_some_and(|entry| entry.exported);
            self.local_scopes
                .last_mut()
                .unwrap()
                .insert(name.to_string(), saved);
            self.variables.remove(name);
            self.indexed_arrays.remove(name);
            self.assoc_arrays.remove(name);
            self.assoc_print_orders.remove(name);
            self.nameref_targets.remove(name);
            if value.is_none() || !self.can_assign_simple_local_direct(name) {
                let mut entry = VariableEntry::default();
                if inherited_export {
                    entry.exported = true;
                    entry.attrs.insert(VarAttrs::EXPORT);
                }
                self.variables.insert(name.to_string(), entry);
                self.sync_export_env(name);
            }
        }

        let Some(value) = value else {
            return Ok(());
        };
        if !self.can_assign_simple_local_direct(name) {
            self.assign(name, value)?;
            return Ok(());
        }
        let (exported, attrs) = self
            .variables
            .get(name)
            .map(|entry| (entry.exported, entry.attrs))
            .unwrap_or_else(|| {
                let attrs = if inherited_export {
                    VarAttrs::EXPORT
                } else {
                    VarAttrs::empty()
                };
                (inherited_export, attrs)
            });
        let value = apply_case_attrs(value, attrs);
        self.variables.insert(
            name.to_string(),
            VariableEntry {
                value: value.clone(),
                has_value: true,
                exported,
                readonly: false,
                attrs,
            },
        );
        if exported {
            std::env::set_var(name, &value);
        }
        Ok(())
    }

    fn make_local_inherit(&mut self, name: &str) -> Result<(), AssignError> {
        if self.local_scopes.is_empty() {
            return Ok(());
        }
        if self
            .local_scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name))
        {
            return Ok(());
        }
        let shadows_outer_local = self
            .local_scopes
            .iter()
            .any(|scope| scope.contains_key(name));
        if !shadows_outer_local && self.variables.get(name).is_some_and(|entry| entry.readonly) {
            return Err(AssignError::ReadOnly(name.to_string()));
        }
        self.local_scopes.last_mut().unwrap().insert(
            name.to_string(),
            SavedVar {
                entry: self.variables.get(name).cloned(),
                indexed: self.indexed_arrays.get(name).cloned(),
                assoc: self.assoc_arrays.get(name).cloned(),
                nameref: self.nameref_targets.get(name).cloned(),
            },
        );
        let exists = self.variables.contains_key(name)
            || self.indexed_arrays.contains_key(name)
            || self.assoc_arrays.contains_key(name)
            || self.nameref_targets.contains_key(name);
        if !exists {
            self.variables
                .insert(name.to_string(), VariableEntry::default());
        }
        if let Some(entry) = self.variables.get_mut(name) {
            entry.attrs.remove(VarAttrs::NAMEREF);
        }
        self.nameref_targets.remove(name);
        self.sync_export_env(name);
        Ok(())
    }

    fn set_local_restore_snapshot(&mut self, name: &str, snapshot: Option<VarSnapshot>) {
        let Some(scope) = self.local_scopes.last_mut() else {
            return;
        };
        if scope.contains_key(name) {
            scope.insert(name.to_string(), Self::saved_var_from_snapshot(snapshot));
        }
    }

    fn unset_array_elem(&mut self, name: &str, key: &str) {
        if name == "DIRSTACK" {
            if let Ok(idx) = key.parse::<usize>() {
                if idx > 0 && idx - 1 < self.dirs_stack.len() {
                    self.dirs_stack.remove(idx - 1);
                }
            }
            return;
        }
        if name == "BASH_ALIASES" {
            self.aliases.remove(key);
            return;
        }
        if name == "BASH_CMDS" {
            self.command_hash.remove(key);
            self.command_hash_hits.remove(key);
            return;
        }
        if let Some(bt) = self.assoc_arrays.get_mut(name) {
            bt.remove(key);
            if let Some(order) = self.assoc_print_orders.get_mut(name) {
                order.retain(|ordered| ordered != key);
            }
            if key == "0" {
                if let Some(entry) = self.variables.get_mut(name) {
                    entry.value = bt.get("0").cloned().unwrap_or_default();
                    entry.has_value = bt.contains_key("0");
                    entry.attrs.insert(VarAttrs::ASSOC);
                }
            }
            return;
        }
        if let Some(bt) = self.indexed_arrays.get_mut(name) {
            if let Ok(idx) = key.parse::<i64>() {
                bt.remove(idx);
                if let Some(entry) = self.variables.get_mut(name) {
                    entry.value = bt.get(0).unwrap_or_default().to_string();
                    entry.has_value = bt.get(0).is_some();
                    entry.attrs.insert(VarAttrs::ARRAY);
                }
            }
        }
    }

    fn all_var_names_with_prefix(&self, prefix: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .variables
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        for k in self.indexed_arrays.keys() {
            if k.starts_with(prefix) && !out.contains(k) {
                out.push(k.clone());
            }
        }
        for k in self.assoc_arrays.keys() {
            if k.starts_with(prefix) && !out.contains(k) {
                out.push(k.clone());
            }
        }
        out.sort();
        out
    }

    fn iter_vars(&self) -> Vec<VarSnapshot> {
        let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for k in self.variables.keys() {
            keys.insert(k.clone());
        }
        for k in self.indexed_arrays.keys() {
            keys.insert(k.clone());
        }
        for k in self.assoc_arrays.keys() {
            keys.insert(k.clone());
        }
        for k in self.nameref_targets.keys() {
            keys.insert(k.clone());
        }
        for name in [
            "BASH_ARGC",
            "BASH_ARGV",
            "BASH_LINENO",
            "BASH_ALIASES",
            "BASH_CMDS",
            "BASH_SOURCE",
            "DIRSTACK",
            "FUNCNAME",
            "SHELLOPTS",
        ] {
            keys.insert(name.to_string());
        }
        for name in self.iter_special_scalar_names() {
            keys.insert(name.to_string());
        }
        let mut out = Vec::with_capacity(keys.len());
        for name in keys {
            let special_indexed = self.special_iter_indexed_array(&name);
            let kind = if special_indexed.is_some() {
                VarKind::Indexed
            } else {
                self.kind(&name)
            };
            let attrs = if special_indexed.is_some() {
                VarAttrs::ARRAY
            } else if let Some(attrs) = Self::special_scalar_attrs(&name) {
                attrs
            } else {
                self.attrs(&name)
            };
            let scalar = if special_indexed.is_some() {
                None
            } else if Self::is_special_scalar_name(&name) {
                self.special_scalar_value_for_snapshot(&name)
            } else if name == "SHELLOPTS" {
                Some(self.shellopts_value())
            } else {
                self.variables
                    .get(&name)
                    .and_then(|e| e.has_value.then(|| e.value.clone()))
            };
            let indexed = special_indexed
                .flatten()
                .or_else(|| self.indexed_arrays.get(&name).map(IndexedArray::all));
            let assoc = if name == "BASH_ALIASES" || name == "BASH_CMDS" {
                self.assoc_all(&name)
            } else {
                self.assoc_arrays.get(&name).map(|bt| {
                    let mut entries = bt
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>();
                    if let Some(order) = self.assoc_print_orders.get(&name) {
                        entries.sort_by_key(|(key, _)| {
                            order
                                .iter()
                                .position(|ordered| ordered == key)
                                .unwrap_or(usize::MAX)
                        });
                    } else {
                        entries.sort_by_key(|(key, _)| bash_assoc_bucket(key));
                    }
                    entries
                })
            };
            let nameref_target = self.nameref_targets.get(&name).cloned();
            out.push(VarSnapshot {
                name,
                kind,
                attrs,
                scalar,
                indexed,
                assoc,
                nameref_target,
            });
        }
        out
    }

    fn var_snapshot(&self, name: &str) -> Option<VarSnapshot> {
        let special_name = matches!(
            name,
            "BASH_ARGC"
                | "BASH_ARGV"
                | "BASH_LINENO"
                | "BASH_ALIASES"
                | "BASH_CMDS"
                | "BASH_SOURCE"
                | "DIRSTACK"
                | "FUNCNAME"
                | "BASHOPTS"
                | "SHELLOPTS"
                | "BASH_ARGV0"
                | "BASH_COMMAND"
                | "BASH_MONOSECONDS"
                | "BASH_SUBSHELL"
                | "EPOCHREALTIME"
                | "EPOCHSECONDS"
                | "HISTCMD"
                | "LINENO"
                | "RANDOM"
                | "SECONDS"
                | "SRANDOM"
        );
        if !special_name
            && !self.variables.contains_key(name)
            && !self.indexed_arrays.contains_key(name)
            && !self.assoc_arrays.contains_key(name)
            && !self.nameref_targets.contains_key(name)
        {
            return None;
        }

        let special_indexed = self.special_iter_indexed_array(name);
        let kind = if special_indexed.is_some() {
            VarKind::Indexed
        } else {
            self.kind(name)
        };
        let attrs = if special_indexed.is_some() {
            VarAttrs::ARRAY
        } else if let Some(attrs) = Self::special_scalar_attrs(name) {
            attrs
        } else {
            self.attrs(name)
        };
        let scalar = if special_indexed.is_some() {
            None
        } else if Self::is_special_scalar_name(name) {
            self.special_scalar_value_for_snapshot(name)
        } else if name == "SHELLOPTS" {
            Some(self.shellopts_value())
        } else {
            self.variables
                .get(name)
                .and_then(|e| e.has_value.then(|| e.value.clone()))
        };
        let indexed = special_indexed
            .flatten()
            .or_else(|| self.indexed_arrays.get(name).map(IndexedArray::all));
        let assoc = if name == "BASH_ALIASES" || name == "BASH_CMDS" {
            self.assoc_all(name)
        } else {
            self.assoc_arrays.get(name).map(|bt| {
                let mut entries = bt
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>();
                if let Some(order) = self.assoc_print_orders.get(name) {
                    entries.sort_by_key(|(key, _)| {
                        order
                            .iter()
                            .position(|ordered| ordered == key)
                            .unwrap_or(usize::MAX)
                    });
                } else {
                    entries.sort_by_key(|(key, _)| bash_assoc_bucket(key));
                }
                entries
            })
        };
        Some(VarSnapshot {
            name: name.to_string(),
            kind,
            attrs,
            scalar,
            indexed,
            assoc,
            nameref_target: self.nameref_targets.get(name).cloned(),
        })
    }

    fn nameref_target(&self, name: &str) -> Option<String> {
        self.nameref_targets.get(name).cloned()
    }

    fn iter_local_vars(&self) -> Vec<VarSnapshot> {
        let Some(scope) = self.local_scopes.last() else {
            return Vec::new();
        };
        let names = scope.keys().collect::<std::collections::BTreeSet<_>>();
        names
            .into_iter()
            .filter_map(|name| self.var_snapshot(name))
            .collect()
    }

    fn local_options_active(&self) -> bool {
        self.local_option_scopes
            .last()
            .is_some_and(|slot| slot.is_some())
    }

    fn make_options_local(&mut self) {
        let snapshot = self.snapshot_options();
        if let Some(slot) = self.local_option_scopes.last_mut() {
            slot.get_or_insert(snapshot);
        }
    }

    fn logical_pwd(&self) -> Option<String> {
        self.logical_pwd_value
            .clone()
            .or_else(|| self.get("PWD").filter(|pwd| !pwd.is_empty()))
    }

    fn set_logical_pwd(&mut self, value: String) {
        self.logical_pwd_value = Some(value.clone());
        self.set("PWD", value);
    }
    };
}
