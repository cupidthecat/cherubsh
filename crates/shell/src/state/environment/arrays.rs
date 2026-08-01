macro_rules! environment_arrays {
    () => {
        fn set_array(&mut self, name: &str, values: Vec<String>) {
            if name == "GROUPS" && self.groups_dynamic {
                return;
            }
            if name == "DIRSTACK" {
                self.dirs_stack = values.into_iter().skip(1).map(PathBuf::from).collect();
                return;
            }
            let prior_attrs = self
                .variables
                .get(name)
                .map(|e| e.attrs)
                .unwrap_or_default();
            let values = values
                .into_iter()
                .map(|value| apply_case_attrs(value, prior_attrs))
                .collect::<Vec<_>>();
            let scalar = values.first().cloned().unwrap_or_default();
            let has_value = !values.is_empty();
            self.variables.insert(
                name.to_string(),
                VariableEntry {
                    value: scalar,
                    has_value,
                    exported: false,
                    readonly: false,
                    attrs: VarAttrs::ARRAY,
                },
            );
            self.indexed_arrays
                .insert(name.to_string(), IndexedArray::from_values(values));
        }

        fn get_array(&self, name: &str) -> Option<Vec<String>> {
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME"
            ) {
                return self
                    .special_iter_indexed_array(name)
                    .flatten()
                    .map(|values| values.into_iter().map(|(_, value)| value).collect());
            }
            if name == "DIRSTACK" {
                return Some(
                    self.dirs_iter()
                        .into_iter()
                        .map(|p| p.display().to_string())
                        .collect(),
                );
            }
            self.indexed_arrays.get(name).map(IndexedArray::values)
        }

        fn set_array_indexed(&mut self, name: &str, index: i64, value: String) {
            if name == "GROUPS" && self.groups_dynamic {
                return;
            }
            if name == "DIRSTACK" {
                if index <= 0 {
                    return;
                }
                let idx = (index - 1) as usize;
                if idx >= self.dirs_stack.len() {
                    self.dirs_stack.resize(idx + 1, PathBuf::new());
                }
                self.dirs_stack[idx] = PathBuf::from(value);
                return;
            }
            if index > 0 {
                let case_attrs = VarAttrs::UPPERCASE | VarAttrs::LOWERCASE | VarAttrs::CAPCASE;
                if let (Some(entry), Some(bt)) =
                    (self.variables.get(name), self.indexed_arrays.get_mut(name))
                {
                    if entry.attrs.contains(VarAttrs::ARRAY) && !entry.attrs.intersects(case_attrs)
                    {
                        bt.insert(index, value);
                        return;
                    }
                }
            }
            let indexed_exists = self.indexed_arrays.contains_key(name);
            let (prior_attrs, exported, has_array_attr, seed_scalar) =
                match self.variables.get(name) {
                    Some(entry) => (
                        entry.attrs,
                        entry.exported,
                        entry.attrs.contains(VarAttrs::ARRAY),
                        (!indexed_exists
                            && entry.has_value
                            && !entry.attrs.intersects(VarAttrs::ARRAY | VarAttrs::ASSOC))
                        .then(|| entry.value.clone()),
                    ),
                    None => (VarAttrs::empty(), false, false, None),
                };
            let value = apply_case_attrs(value, prior_attrs);
            let bt = self.indexed_arrays.entry(name.to_string()).or_default();
            if let Some(value) = seed_scalar {
                if bt.get(0).is_none() {
                    bt.insert(0, value);
                }
            }
            bt.insert(index, value.clone());
            let update_entry = index == 0 || !has_array_attr;
            if update_entry {
                let attrs = prior_attrs | VarAttrs::ARRAY;
                let scalar = bt.get(0).unwrap_or_default().to_string();
                let has_value = bt.get(0).is_some();
                self.variables.insert(
                    name.to_string(),
                    VariableEntry {
                        value: scalar,
                        has_value,
                        exported,
                        readonly: false,
                        attrs,
                    },
                );
            }
        }

        fn get_array_indexed(&self, name: &str, index: i64) -> Option<String> {
            if name == "DIRSTACK" {
                if index < 0 {
                    return None;
                }
                return self
                    .dirs_iter()
                    .get(index as usize)
                    .map(|p| p.display().to_string());
            }
            if name == "FUNCNAME" {
                if index < 0 {
                    return None;
                }
                return self
                    .funcname_values()
                    .into_iter()
                    .find(|(idx, _)| *idx == index)
                    .map(|(_, value)| value);
            }
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE"
            ) {
                if index < 0 {
                    return None;
                }
                return self
                    .special_iter_indexed_array(name)
                    .and_then(|values| values)
                    .and_then(|values| {
                        values
                            .into_iter()
                            .find(|(idx, _)| *idx == index)
                            .map(|(_, value)| value)
                    });
            }
            self.indexed_arrays
                .get(name)
                .and_then(|bt| bt.get(index).map(str::to_string))
        }

        fn get_array_indexed_cow<'a>(&'a self, name: &str, index: i64) -> Option<Cow<'a, str>> {
            if name == "DIRSTACK" {
                return self.get_array_indexed(name, index).map(Cow::Owned);
            }
            if name == "FUNCNAME" {
                if index < 0 {
                    return None;
                }
                return self.get_array_indexed(name, index).map(Cow::Owned);
            }
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE"
            ) {
                return self.get_array_indexed(name, index).map(Cow::Owned);
            }
            self.indexed_arrays
                .get(name)
                .and_then(|bt| bt.get(index).map(Cow::Borrowed))
        }

        fn get_array_all(&self, name: &str) -> Option<Vec<(i64, String)>> {
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME"
            ) {
                return self.special_iter_indexed_array(name).flatten();
            }
            if name == "DIRSTACK" {
                return Some(
                    self.dirs_iter()
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| (i as i64, p.display().to_string()))
                        .collect(),
                );
            }
            self.indexed_arrays.get(name).map(IndexedArray::all)
        }

        fn array_keys(&self, name: &str) -> Option<Vec<i64>> {
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME"
            ) {
                return self
                    .special_iter_indexed_array(name)
                    .flatten()
                    .map(|values| values.into_iter().map(|(idx, _)| idx).collect());
            }
            if name == "DIRSTACK" {
                return Some((0..self.dirs_iter().len() as i64).collect());
            }
            self.indexed_arrays.get(name).map(IndexedArray::keys)
        }

        fn array_len(&self, name: &str) -> usize {
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME"
            ) {
                return self
                    .special_iter_indexed_array(name)
                    .flatten()
                    .map(|values| values.len())
                    .unwrap_or(0);
            }
            if name == "DIRSTACK" {
                return self.dirs_iter().len();
            }
            self.indexed_arrays
                .get(name)
                .map(IndexedArray::len)
                .unwrap_or(0)
        }

        fn array_max_index(&self, name: &str) -> Option<i64> {
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME"
            ) {
                return self
                    .special_iter_indexed_array(name)
                    .flatten()
                    .and_then(|values| values.into_iter().map(|(idx, _)| idx).max());
            }
            if name == "DIRSTACK" {
                return (!self.dirs_iter().is_empty()).then(|| self.dirs_iter().len() as i64 - 1);
            }
            self.indexed_arrays
                .get(name)
                .and_then(IndexedArray::max_index)
        }

        fn set_array_assoc(&mut self, name: &str, key: &str, value: String) {
            if name == "BASH_ALIASES" {
                if state_valid_alias_name(key) {
                    self.aliases.insert(key.to_string(), value);
                } else if let (Some(source), Some(line)) =
                    (self.diagnostic_source_name(), self.diagnostic_line())
                {
                    eprintln!("{source}: line {line}: `{key}': invalid alias name");
                } else {
                    eprintln!("cherubsh: `{key}': invalid alias name");
                }
                return;
            }
            if name == "BASH_CMDS" {
                self.command_hash
                    .insert(key.to_string(), PathBuf::from(value));
                self.command_hash_hits.insert(key.to_string(), 0);
                return;
            }
            if key != "0" && !self.assoc_print_orders.contains_key(name) {
                let case_attrs = VarAttrs::UPPERCASE | VarAttrs::LOWERCASE | VarAttrs::CAPCASE;
                if let (Some(entry), Some(bt)) =
                    (self.variables.get(name), self.assoc_arrays.get_mut(name))
                {
                    if entry.attrs.contains(VarAttrs::ASSOC) && !entry.attrs.intersects(case_attrs)
                    {
                        bt.insert(key.to_string(), value);
                        return;
                    }
                }
            }
            let (prior_attrs, exported, has_assoc_attr) = self
                .variables
                .get(name)
                .map(|entry| {
                    (
                        entry.attrs,
                        entry.exported,
                        entry.attrs.contains(VarAttrs::ASSOC),
                    )
                })
                .unwrap_or((VarAttrs::empty(), false, false));
            let value = apply_case_attrs(value, prior_attrs);
            let zero_value = {
                let bt = self.assoc_arrays.entry(name.to_string()).or_default();
                bt.insert(key.to_string(), value);
                if let Some(order) = self.assoc_print_orders.get_mut(name) {
                    let key = key.to_string();
                    if !order.contains(&key) {
                        order.push(key);
                    }
                }
                bt.get("0").cloned()
            };
            let update_entry = key == "0" || !has_assoc_attr;
            if update_entry {
                let attrs = prior_attrs | VarAttrs::ASSOC;
                self.variables.insert(
                    name.to_string(),
                    VariableEntry {
                        value: zero_value.clone().unwrap_or_default(),
                        has_value: zero_value.is_some(),
                        exported,
                        readonly: false,
                        attrs,
                    },
                );
            }
        }

        fn get_array_assoc(&self, name: &str, key: &str) -> Option<String> {
            if name == "BASH_ALIASES" {
                return self.aliases.get(key).cloned();
            }
            if name == "BASH_CMDS" {
                return self
                    .command_hash
                    .get(key)
                    .map(|path| path.display().to_string());
            }
            self.assoc_arrays
                .get(name)
                .and_then(|bt| bt.get(key).cloned())
        }

        fn get_array_assoc_cow<'a>(&'a self, name: &str, key: &str) -> Option<Cow<'a, str>> {
            if name == "BASH_ALIASES" {
                return self
                    .aliases
                    .get(key)
                    .map(|value| Cow::Borrowed(value.as_str()));
            }
            if name == "BASH_CMDS" {
                return self.get_array_assoc(name, key).map(Cow::Owned);
            }
            self.assoc_arrays
                .get(name)
                .and_then(|bt| bt.get(key).map(|value| Cow::Borrowed(value.as_str())))
        }

        fn assoc_all(&self, name: &str) -> Option<Vec<(String, String)>> {
            if name == "BASH_ALIASES" {
                let mut entries = self
                    .aliases
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>();
                entries.sort_by_key(|(k, _)| bash_alias_bucket(k));
                return Some(entries);
            }
            if name == "BASH_CMDS" {
                let mut entries = self
                    .command_hash
                    .iter()
                    .map(|(k, v)| (k.clone(), v.display().to_string()))
                    .collect::<Vec<_>>();
                entries.sort_by_key(|(k, _)| bash_command_bucket(k));
                return Some(entries);
            }
            self.assoc_arrays.get(name).map(|bt| {
                let mut entries = bt
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>();
                entries.sort_by_key(|(k, _)| bash_assoc_bucket(k));
                entries
            })
        }

        fn assoc_keys(&self, name: &str) -> Option<Vec<String>> {
            if name == "BASH_ALIASES" {
                let mut keys = self.aliases.keys().cloned().collect::<Vec<_>>();
                keys.sort_by_key(|k| bash_alias_bucket(k));
                return Some(keys);
            }
            if name == "BASH_CMDS" {
                let mut keys = self.command_hash.keys().cloned().collect::<Vec<_>>();
                keys.sort_by_key(|k| bash_command_bucket(k));
                return Some(keys);
            }
            self.assoc_arrays.get(name).map(|bt| {
                let mut keys = bt.keys().cloned().collect::<Vec<_>>();
                keys.sort_by_key(|k| bash_assoc_bucket(k));
                keys
            })
        }

        fn assoc_len(&self, name: &str) -> usize {
            if name == "BASH_ALIASES" {
                return self.aliases.len();
            }
            if name == "BASH_CMDS" {
                return self.command_hash.len();
            }
            self.assoc_arrays.get(name).map(|bt| bt.len()).unwrap_or(0)
        }

        fn kind(&self, name: &str) -> VarKind {
            if self.nameref_targets.contains_key(name) {
                return VarKind::Nameref;
            }
            if matches!(name, "BASH_ALIASES" | "BASH_CMDS") {
                return VarKind::Assoc;
            }
            if name == "DIRSTACK" {
                return VarKind::Indexed;
            }
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "FUNCNAME"
            ) {
                return VarKind::Indexed;
            }
            if self.iter_special_scalar_names().contains(&name) {
                return VarKind::Scalar;
            }
            let attrs = self.attrs(name);
            if attrs.contains(VarAttrs::ASSOC) {
                return VarKind::Assoc;
            }
            if attrs.contains(VarAttrs::ARRAY) {
                return VarKind::Indexed;
            }
            if self.assoc_arrays.contains_key(name) {
                return VarKind::Assoc;
            }
            if self.indexed_arrays.contains_key(name) {
                return VarKind::Indexed;
            }
            if self.variables.contains_key(name) {
                return VarKind::Scalar;
            }
            if std::env::var(name).is_ok() {
                return VarKind::Scalar;
            }
            VarKind::Unset
        }

        fn attrs(&self, name: &str) -> VarAttrs {
            if matches!(
                name,
                "BASH_ARGC" | "BASH_ARGV" | "BASH_LINENO" | "BASH_SOURCE" | "DIRSTACK" | "FUNCNAME"
            ) {
                return VarAttrs::ARRAY;
            }
            if matches!(name, "BASH_ALIASES" | "BASH_CMDS") {
                return VarAttrs::ASSOC;
            }
            if let Some(attrs) = Self::special_scalar_attrs(name) {
                return attrs;
            }
            self.variables
                .get(name)
                .map(|e| e.attrs)
                .unwrap_or_default()
        }

        fn set_attr(&mut self, name: &str, attr: VarAttrs, on: bool) {
            if attr.is_empty() {
                self.variables.entry(name.to_string()).or_default();
                return;
            }
            let existed = self.variables.contains_key(name);
            let had_indexed = self.indexed_arrays.contains_key(name);
            let had_assoc = self.assoc_arrays.contains_key(name);
            let entry = self.variables.entry(name.to_string()).or_default();
            let was_array = entry.attrs.contains(VarAttrs::ARRAY);
            let was_assoc = entry.attrs.contains(VarAttrs::ASSOC);
            if on {
                entry.attrs.insert(attr);
            } else {
                entry.attrs.remove(attr);
            }
            if attr.contains(VarAttrs::ARRAY) && on {
                if existed && !had_indexed && !was_array && entry.has_value {
                    let scalar = entry.value.clone();
                    let bt = self.indexed_arrays.entry(name.to_string()).or_default();
                    if bt.get(0).is_none() {
                        bt.insert(0, scalar);
                    }
                }
                self.assoc_arrays.remove(name);
                self.assoc_print_orders.remove(name);
                entry.attrs.remove(VarAttrs::ASSOC);
            }
            if attr.contains(VarAttrs::ASSOC) && on {
                let seed_scalar = if existed && !had_assoc && !was_assoc && entry.has_value {
                    Some(entry.value.clone())
                } else {
                    None
                };
                self.indexed_arrays.remove(name);
                self.assoc_print_orders.remove(name);
                entry.attrs.remove(VarAttrs::ARRAY);
                if let Some(scalar) = seed_scalar {
                    let bt = self.assoc_arrays.entry(name.to_string()).or_default();
                    bt.entry("0".to_string()).or_insert(scalar);
                }
            }
            if attr.contains(VarAttrs::EXPORT) {
                entry.exported = on;
            }
            if attr.contains(VarAttrs::READONLY) {
                entry.readonly = on;
            }
            if attr.contains(VarAttrs::NAMEREF) {
                if on {
                    if !self.nameref_targets.contains_key(name) && entry.has_value {
                        self.nameref_targets
                            .insert(name.to_string(), entry.value.clone());
                    }
                } else {
                    self.nameref_targets.remove(name);
                }
            }
            if attr.contains(VarAttrs::EXPORT) {
                self.sync_export_env(name);
            }
        }

        fn global_kind(&self, name: &str) -> VarKind {
            self.global_saved_snapshot(name)
                .map(|saved| self.kind_from_snapshot(name, saved))
                .unwrap_or_else(|| self.kind(name))
        }

        fn global_attrs(&self, name: &str) -> VarAttrs {
            self.global_saved_snapshot(name)
                .and_then(|saved| saved.entry.as_ref().map(|entry| entry.attrs))
                .unwrap_or_else(|| self.attrs(name))
        }

        fn global_exported(&self, name: &str) -> bool {
            self.global_saved_snapshot(name)
                .and_then(|saved| saved.entry.as_ref().map(|entry| entry.exported))
                .unwrap_or_else(|| self.exported(name))
        }

        fn global_get(&self, name: &str) -> Option<String> {
            if let Some(saved) = self.global_saved_snapshot(name) {
                return saved
                    .entry
                    .as_ref()
                    .and_then(|entry| entry.has_value.then(|| entry.value.clone()));
            }
            self.get(name)
        }

        fn preserve_assoc_order_for_next_assignment(&mut self, name: &str) {
            self.preserve_assoc_order_once.insert(name.to_string());
        }

        fn take_preserve_assoc_order_for_next_assignment(&mut self, name: &str) -> bool {
            self.preserve_assoc_order_once.remove(name)
        }

        fn set_assoc_print_order(&mut self, name: &str, keys: Option<Vec<String>>) {
            if let Some(keys) = keys {
                self.assoc_print_orders.insert(name.to_string(), keys);
            } else {
                self.assoc_print_orders.remove(name);
            }
        }

        fn global_is_readonly(&self, name: &str) -> bool {
            self.global_saved_snapshot(name)
                .and_then(|saved| saved.entry.as_ref().map(|entry| entry.readonly))
                .unwrap_or_else(|| self.is_readonly(name))
        }

        fn global_array_keys(&self, name: &str) -> Option<Vec<i64>> {
            if let Some(saved) = self.global_saved_snapshot(name) {
                if let Some(indexed) = saved.indexed.as_ref() {
                    return Some(indexed.keys());
                }
                return None;
            }
            self.array_keys(name)
        }

        fn set_global_attr(&mut self, name: &str, attr: VarAttrs, on: bool) {
            self.with_global_var(name, |state| state.set_attr(name, attr, on));
        }

        fn assign_global(&mut self, name: &str, value: String) -> Result<(), AssignError> {
            self.with_global_var(name, |state| state.assign(name, value))
        }

        fn unset_global(&mut self, name: &str) {
            self.with_global_var(name, |state| state.unset(name));
        }

        fn export_global(&mut self, name: &str) {
            self.with_global_var(name, |state| state.export(name));
        }

        fn set_global_array(&mut self, name: &str, values: Vec<String>) {
            self.with_global_var(name, |state| state.set_array(name, values));
        }

        fn set_global_array_indexed(&mut self, name: &str, index: i64, value: String) {
            self.with_global_var(name, |state| state.set_array_indexed(name, index, value));
        }

        fn set_global_array_assoc(&mut self, name: &str, key: &str, value: String) {
            self.with_global_var(name, |state| state.set_array_assoc(name, key, value));
        }

        fn unset_global_array_elem(&mut self, name: &str, key: &str) {
            self.with_global_var(name, |state| state.unset_array_elem(name, key));
        }
    };
}
