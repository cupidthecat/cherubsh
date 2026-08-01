impl ShellState {
    fn special_scalar_attrs(name: &str) -> Option<VarAttrs> {
        match name {
            "HISTCMD" | "RANDOM" | "SRANDOM" => Some(VarAttrs::INTEGER),
            "BASHOPTS" | "SHELLOPTS" => Some(VarAttrs::READONLY),
            _ if Self::is_special_scalar_name(name) => Some(VarAttrs::empty()),
            _ => None,
        }
    }

    fn is_special_scalar_name(name: &str) -> bool {
        matches!(
            name,
            "BASH_ARGV0"
                | "BASH_COMMAND"
                | "BASH_MONOSECONDS"
                | "BASH_SUBSHELL"
                | "BASH_TRAPSIG"
                | "EPOCHREALTIME"
                | "EPOCHSECONDS"
                | "HISTCMD"
                | "LINENO"
                | "RANDOM"
                | "SECONDS"
                | "BASHOPTS"
                | "SHELLOPTS"
                | "SRANDOM"
        )
    }

    fn special_scalar_value_for_snapshot(&self, name: &str) -> Option<String> {
        self.get_cow(name).map(Cow::into_owned)
    }

    fn iter_special_scalar_names(&self) -> Vec<&'static str> {
        let mut names = vec![
            "BASH_COMMAND",
            "BASH_SUBSHELL",
            "BASHOPTS",
            "HISTCMD",
            "LINENO",
            "RANDOM",
            "SHELLOPTS",
            "SRANDOM",
        ];
        if self.bash_argv0_dynamic {
            names.push("BASH_ARGV0");
        }
        if self.seconds_dynamic {
            names.push("SECONDS");
        }
        if self.epochseconds_dynamic {
            names.push("EPOCHSECONDS");
        }
        if self.epochrealtime_dynamic {
            names.push("EPOCHREALTIME");
        }
        if self.bash_monoseconds_dynamic {
            names.push("BASH_MONOSECONDS");
        }
        names
    }

    fn saved_scalar_value(saved: &SavedVar) -> Option<String> {
        if let Some(indexed) = saved.indexed.as_ref() {
            return indexed.get(0).map(str::to_string);
        }
        saved.entry.as_ref().and_then(|entry| {
            entry
                .has_value
                .then(|| saved.nameref.clone().unwrap_or_else(|| entry.value.clone()))
        })
    }

    fn local_self_nameref_target(&self, name: &str) -> Option<String> {
        let target = self.nameref_targets.get(name)?;
        if target == name && self.global_scope_index(name).is_some() {
            return Some(target.clone());
        }
        None
    }

    fn report_circular_name_reference(&self, name: &str) {
        if let (Some(source), Some(line)) = (self.diagnostic_source_name(), self.diagnostic_line())
        {
            eprintln!("{source}: line {line}: warning: {name}: circular name reference");
        } else {
            eprintln!("cherubsh: warning: {name}: circular name reference");
        }
    }

    fn report_max_nameref_depth(&self, name: &str) {
        if let (Some(source), Some(line)) = (self.diagnostic_source_name(), self.diagnostic_line())
        {
            eprintln!(
                "{source}: line {line}: warning: {name}: maximum nameref depth ({NAMEREF_MAX_DEPTH}) exceeded"
            );
        } else {
            eprintln!(
                "cherubsh: warning: {name}: maximum nameref depth ({NAMEREF_MAX_DEPTH}) exceeded"
            );
        }
    }

    fn report_removing_nameref_attribute(&self, name: &str) {
        if let (Some(source), Some(line)) = (self.diagnostic_source_name(), self.diagnostic_line())
        {
            eprintln!("{source}: line {line}: warning: {name}: removing nameref attribute");
        } else {
            eprintln!("cherubsh: warning: {name}: removing nameref attribute");
        }
    }

    fn report_invalid_xtracefd(&self, value: &str) {
        if let (Some(source), Some(line)) = (self.diagnostic_source_name(), self.diagnostic_line())
        {
            eprintln!(
                "{source}: line {line}: BASH_XTRACEFD: {value}: invalid value for trace file descriptor"
            );
        } else {
            eprintln!("BASH_XTRACEFD: {value}: invalid value for trace file descriptor");
        }
    }

    fn assign_nameref_target(
        &mut self,
        source: &str,
        target: &str,
        value: String,
    ) -> Result<(), AssignError> {
        if let Some((name, subscript)) = state_array_reference(target) {
            if self.is_readonly(name) {
                return Err(AssignError::ReadOnly(name.to_string()));
            }
            if source == name
                && self
                    .nameref_targets
                    .get(source)
                    .is_some_and(|t| t == target)
            {
                return Err(AssignError::InvalidName(target.to_string()));
            }
            if self.attrs(name).contains(VarAttrs::NAMEREF) {
                self.report_removing_nameref_attribute(name);
                self.set_attr(name, VarAttrs::NAMEREF, false);
                self.set_array(name, Vec::new());
            }
            if self.kind(name) == VarKind::Assoc {
                self.set_array_assoc(name, subscript, value);
                return Ok(());
            }
            let trimmed = subscript.trim();
            if trimmed.is_empty() || matches!(trimmed, "*" | "@") {
                return Err(AssignError::BadArraySubscript(target.to_string()));
            }
            let index = trimmed
                .parse::<i64>()
                .map_err(|_| AssignError::BadArraySubscript(target.to_string()))?;
            self.set_array_indexed(name, index, value);
            return Ok(());
        }
        if !state_valid_name(target) {
            return Err(AssignError::InvalidName(target.to_string()));
        }
        self.assign(target, value)
    }

    fn snapshot_var(&self, name: &str) -> SavedVar {
        SavedVar {
            entry: self.variables.get(name).cloned(),
            indexed: self.indexed_arrays.get(name).cloned(),
            assoc: self.assoc_arrays.get(name).cloned(),
            nameref: self.nameref_targets.get(name).cloned(),
        }
    }

    fn restore_var_snapshot(&mut self, name: &str, saved: SavedVar) {
        match saved.entry {
            Some(entry) => {
                self.variables.insert(name.to_string(), entry);
            }
            None => {
                self.variables.remove(name);
            }
        }
        match saved.indexed {
            Some(values) => {
                self.indexed_arrays.insert(name.to_string(), values);
            }
            None => {
                self.indexed_arrays.remove(name);
            }
        }
        match saved.assoc {
            Some(values) => {
                self.assoc_arrays.insert(name.to_string(), values);
                self.assoc_print_orders.remove(name);
            }
            None => {
                self.assoc_arrays.remove(name);
                self.assoc_print_orders.remove(name);
            }
        }
        match saved.nameref {
            Some(target) => {
                self.nameref_targets.insert(name.to_string(), target);
            }
            None => {
                self.nameref_targets.remove(name);
            }
        }
    }

    fn saved_var_from_snapshot(snapshot: Option<VarSnapshot>) -> SavedVar {
        let Some(snapshot) = snapshot else {
            return SavedVar {
                entry: None,
                indexed: None,
                assoc: None,
                nameref: None,
            };
        };
        let indexed = snapshot.indexed.map(IndexedArray::from_pairs);
        let assoc = snapshot
            .assoc
            .map(|values| values.into_iter().collect::<HashMap<_, _>>());
        let value = snapshot
            .scalar
            .clone()
            .or_else(|| {
                indexed
                    .as_ref()
                    .and_then(|values| values.get(0).map(str::to_string))
            })
            .unwrap_or_default();
        let has_value = snapshot.scalar.is_some()
            || indexed
                .as_ref()
                .is_some_and(|values| values.get(0).is_some());
        SavedVar {
            entry: Some(VariableEntry {
                value,
                has_value,
                exported: snapshot.attrs.contains(VarAttrs::EXPORT),
                readonly: snapshot.attrs.contains(VarAttrs::READONLY),
                attrs: snapshot.attrs,
            }),
            indexed,
            assoc,
            nameref: snapshot.nameref_target,
        }
    }

    fn global_scope_index(&self, name: &str) -> Option<usize> {
        self.local_scopes
            .iter()
            .position(|scope| scope.contains_key(name))
    }

    fn global_saved_snapshot(&self, name: &str) -> Option<&SavedVar> {
        self.global_scope_index(name)
            .and_then(|idx| self.local_scopes.get(idx))
            .and_then(|scope| scope.get(name))
    }

    fn with_global_var<R>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let Some(scope_idx) = self.global_scope_index(name) else {
            return f(self);
        };
        let visible = self.snapshot_var(name);
        let global = self.local_scopes[scope_idx]
            .get(name)
            .cloned()
            .unwrap_or_else(|| self.snapshot_var(name));

        self.restore_var_snapshot(name, global);
        let result = f(self);
        let updated_global = self.snapshot_var(name);
        self.restore_var_snapshot(name, visible);
        if let Some(scope) = self.local_scopes.get_mut(scope_idx) {
            scope.insert(name.to_string(), updated_global);
        }
        self.sync_export_env(name);
        result
    }

    fn kind_from_snapshot(&self, name: &str, saved: &SavedVar) -> VarKind {
        if saved.nameref.is_some() {
            return VarKind::Nameref;
        }
        let attrs = saved.entry.as_ref().map(|e| e.attrs).unwrap_or_default();
        if attrs.contains(VarAttrs::ASSOC) || saved.assoc.is_some() {
            return VarKind::Assoc;
        }
        if attrs.contains(VarAttrs::ARRAY) || saved.indexed.is_some() {
            return VarKind::Indexed;
        }
        if saved.entry.is_some() {
            return VarKind::Scalar;
        }
        if std::env::var(name).is_ok() {
            return VarKind::Scalar;
        }
        VarKind::Unset
    }

    fn apply_history_special_var(&mut self, name: &str, value: &str) {
        match name {
            "HISTSIZE" => {
                if let Ok(sz) = value.trim().parse::<usize>() {
                    if sz > 0 {
                        self.histsize = sz;
                        self.history_table.set_max(sz);
                    }
                }
            }
            "HISTFILESIZE" => {
                if let Ok(sz) = value.trim().parse::<usize>() {
                    self.histfilesize = sz;
                }
            }
            "HISTCONTROL" => {
                self.histcontrol_flags = HistControl::parse(value);
            }
            "HISTFILE" => {
                self.histfile_explicit = true;
                self.histfile = if value.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                };
            }
            _ => {}
        }
    }

    fn can_assign_simple_local_direct(&self, name: &str) -> bool {
        !(matches!(
            name,
            "RANDOM"
                | "BASH_ARGV0"
                | "BASH_MONOSECONDS"
                | "SECONDS"
                | "EPOCHSECONDS"
                | "EPOCHREALTIME"
                | "FUNCNAME"
                | "BASHOPTS"
                | "SHELLOPTS"
                | "HISTSIZE"
                | "HISTFILESIZE"
                | "HISTCONTROL"
                | "HISTFILE"
                | "IGNOREEOF"
        ) || self.restricted && matches!(name, "PATH" | "SHELL"))
    }

    pub fn configure_history_from_vars(&mut self, load_file: bool, default_histfile: bool) {
        if let Ok(sz) = self.get("HISTSIZE").unwrap_or_default().parse::<usize>() {
            if sz > 0 {
                self.histsize = sz;
                self.history_table.set_max(sz);
            }
        }
        if let Ok(fsz) = self
            .get("HISTFILESIZE")
            .unwrap_or_default()
            .parse::<usize>()
        {
            self.histfilesize = fsz;
        }
        if let Some(raw) = self.get("HISTCONTROL") {
            self.histcontrol_flags = HistControl::parse(&raw);
        }

        self.histfile_explicit = self.variables.contains_key("HISTFILE");
        let path = match self.get("HISTFILE") {
            Some(value) if value.is_empty() => None,
            Some(value) => Some(PathBuf::from(value)),
            None if default_histfile => std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".bash_history");
                p
            }),
            None => None,
        };
        self.histfile = path.clone();
        if load_file {
            if let Some(p) = path {
                if self.history_table.load_from(&p).is_err() {
                    self.histfile = None;
                }
            }
        }
    }

    pub fn push_input(&mut self, input: BashInput) {
        let previous = std::mem::replace(&mut self.input, input);
        if !matches!(previous, BashInput::None) {
            self.input_stack.push(previous);
        }
        self.input_line_count_stack
            .push(self.current_command_line_count);
        self.current_command_line_count = 0;
        self.eof_reached = false;
    }

    pub fn pop_input(&mut self) -> bool {
        let previous_line_count = self.input_line_count_stack.pop().unwrap_or(0);
        match self.input_stack.pop() {
            Some(previous) => {
                self.input = previous;
                self.current_command_line_count = previous_line_count;
                self.eof_reached = false;
                true
            }
            None => {
                self.input = BashInput::None;
                self.current_command_line_count = previous_line_count;
                self.eof_reached = true;
                false
            }
        }
    }

    /// Seed environment-derived variables from the process environment.
    /// Called once during shell_initialize.
    pub fn import_process_env(&mut self) {
        for (key, value) in std::env::vars() {
            self.variables.insert(
                key,
                VariableEntry {
                    value,
                    has_value: true,
                    exported: true,
                    readonly: false,
                    attrs: VarAttrs::EXPORT,
                },
            );
        }
    }

    fn script_source_name(&self) -> Option<String> {
        if self.startup_state == StartupMode::DashC {
            return None;
        }
        match &self.input {
            BashInput::Stream { name, .. } if !name.is_empty() => Some(self.source_name(name)),
            _ => None,
        }
    }

    pub fn update_window_size(&mut self) {
        if !self.option("checkwinsize") {
            return;
        }
        let fd = self.tty_fd_value.unwrap_or(libc::STDIN_FILENO);
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } != 0 {
            return;
        }
        if size.ws_col > 0 {
            let _ = self.assign("COLUMNS", size.ws_col.to_string());
        }
        if size.ws_row > 0 {
            let _ = self.assign("LINES", size.ws_row.to_string());
        }
    }

    pub fn check_mail(&mut self) {
        use std::os::unix::fs::MetadataExt;

        let interval = self
            .get("MAILCHECK")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(60);
        if interval == 0
            || self
                .mail_last_check
                .is_some_and(|last| last.elapsed().as_secs() < interval)
        {
            return;
        }
        self.mail_last_check = Some(Instant::now());

        let entries = self
            .get("MAILPATH")
            .filter(|value| !value.is_empty())
            .map(|value| value.split(':').map(str::to_string).collect::<Vec<_>>())
            .or_else(|| self.get("MAIL").map(|value| vec![value]))
            .unwrap_or_default();
        for entry in entries {
            let (path, message) = entry
                .split_once('?')
                .or_else(|| entry.split_once('%'))
                .map(|(path, message)| (path, Some(message)))
                .unwrap_or((entry.as_str(), None));
            if path.is_empty() {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(path) else {
                self.mail_status.remove(Path::new(path));
                continue;
            };
            let current = MailStatus {
                modified: metadata.mtime(),
                accessed: metadata.atime(),
                size: metadata.len(),
            };
            let previous = self.mail_status.insert(PathBuf::from(path), current);
            let new_mail = current.size > 0
                && current.modified >= current.accessed
                && previous
                    .is_none_or(|old| current.modified > old.modified || current.size > old.size);
            if new_mail {
                if let Some(message) = message {
                    eprintln!("{}", message.replace("$_", path));
                } else {
                    eprintln!("You have new mail in {path}");
                }
            } else if self.option("mailwarn")
                && previous.is_some_and(|old| current.accessed > old.accessed)
            {
                eprintln!("The mail in {path} has been read");
            }
        }
    }

    fn source_name(&self, source: &str) -> String {
        if !self.option("bash_source_fullpath") || source == "main" {
            return source.to_string();
        }
        std::fs::canonicalize(source)
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.to_string())
    }

    fn main_source_name(&self) -> String {
        self.script_source_name()
            .unwrap_or_else(|| "main".to_string())
    }

    fn include_synthetic_main_frame(&self) -> bool {
        (self.functrace || self.errtrace || self.option("extdebug"))
            && self.script_source_name().is_some()
    }

    fn current_source_name(&self) -> String {
        self.funcname_source_stack
            .last()
            .cloned()
            .unwrap_or_else(|| self.main_source_name())
    }

    fn current_call_line(&self) -> u32 {
        self.diagnostic_line_stack
            .last()
            .copied()
            .or_else(|| {
                (self.current_command_line_count > 0).then_some(self.current_command_line_count)
            })
            .unwrap_or(0)
    }

    fn funcname_values(&self) -> Vec<(i64, String)> {
        if self.funcname_stack.is_empty() {
            return Vec::new();
        }
        if self
            .funcname_source_frame_stack
            .iter()
            .all(|is_source_frame| *is_source_frame)
        {
            return Vec::new();
        }
        let mut values = self
            .funcname_stack
            .iter()
            .rev()
            .enumerate()
            .map(|(i, value)| (i as i64, value.clone()))
            .collect::<Vec<_>>();
        if self.include_synthetic_main_frame() {
            values.push((values.len() as i64, "main".to_string()));
        }
        values
    }

    fn bash_source_values(&self) -> Vec<(i64, String)> {
        if self.funcname_source_stack.is_empty() {
            return self
                .script_source_name()
                .map(|source| vec![(0, source)])
                .unwrap_or_default();
        }
        let mut values = self
            .funcname_source_stack
            .iter()
            .rev()
            .enumerate()
            .map(|(i, value)| (i as i64, value.clone()))
            .collect::<Vec<_>>();
        if self.include_synthetic_main_frame() {
            values.push((values.len() as i64, self.main_source_name()));
        }
        values
    }

    fn bash_lineno_values(&self) -> Vec<(i64, String)> {
        if self.funcname_lineno_stack.is_empty() {
            return self
                .script_source_name()
                .map(|_| vec![(0, "0".to_string())])
                .unwrap_or_default();
        }
        let mut values = self
            .funcname_lineno_stack
            .iter()
            .rev()
            .enumerate()
            .map(|(i, value)| (i as i64, value.to_string()))
            .collect::<Vec<_>>();
        if self.include_synthetic_main_frame() {
            values.push((values.len() as i64, "0".to_string()));
        }
        values
    }

    fn bash_argc_values(&self) -> Vec<(i64, String)> {
        let mut values = self
            .bash_argc_stack
            .iter()
            .rev()
            .enumerate()
            .map(|(i, value)| (i as i64, value.to_string()))
            .collect::<Vec<_>>();
        if self.option("extdebug") {
            values.push((values.len() as i64, "0".to_string()));
        }
        values
    }

    fn bash_argv_values(&self) -> Vec<(i64, String)> {
        self.bash_argv_stack
            .iter()
            .enumerate()
            .map(|(i, value)| (i as i64, value.clone()))
            .collect()
    }

    fn special_iter_indexed_array(&self, name: &str) -> Option<Option<Vec<(i64, String)>>> {
        match name {
            "BASH_ARGC" => Some(Some(self.bash_argc_values())),
            "BASH_ARGV" => Some(Some(self.bash_argv_values())),
            "DIRSTACK" => Some(Some(Vec::new())),
            "BASH_SOURCE" => Some(Some(self.bash_source_values())),
            "BASH_LINENO" => Some(Some(self.bash_lineno_values())),
            "FUNCNAME" => {
                let values = self.funcname_values();
                Some((!values.is_empty()).then_some(values))
            }
            _ => None,
        }
    }
}
