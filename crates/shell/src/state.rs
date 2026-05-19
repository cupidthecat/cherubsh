use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use cherubsh_common::{
    completion::{CompSlot, CompSpec},
    history::{HistControl, HistoryTable},
    jobs::JobTable,
    keymap::{EditAction, Keymap},
    signals::{TrapAction, TrapKind},
    AssignError, Environment, FastHashMap as HashMap, FastHashSet as HashSet, TrapEntry, VarAttrs,
    VarKind, VarSnapshot,
};
use cherubsh_lineedit::LineEditor;

use crate::input::BashInput;

/// Default rc file path: bash's DEFAULT_BASHRC is "~/.bashrc".
fn default_bashrc() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut path = PathBuf::from(home);
        path.push(".bashrc");
        path
    } else {
        PathBuf::from(".bashrc")
    }
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_epoch_realtime() -> (u64, u32) {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let micros = duration.subsec_micros().max(100_000);
    (duration.as_secs(), micros)
}

fn bash_intrand32(last: u32) -> u32 {
    let seed = if last == 0 { 123_459_876 } else { last };
    let h = seed / 127_773;
    let l = seed - (127_773 * h);
    let t = 16_807_i64 * l as i64 - 2_836_i64 * h as i64;
    if t < 0 {
        (t + 0x7fff_ffff) as u32
    } else {
        t as u32
    }
}

fn bash_random_from_seed(seed: u32) -> (u32, i32) {
    let next_seed = bash_intrand32(seed);
    let mixed = ((next_seed >> 16) ^ (next_seed & 65_535)) & 32_767;
    (next_seed, mixed as i32)
}

fn bash_hash(value: &str) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in value.bytes() {
        let old = hash;
        hash = old
            .wrapping_add(old << 1)
            .wrapping_add(old << 4)
            .wrapping_add(old << 7)
            .wrapping_add(old << 8)
            .wrapping_add(old << 24);
        hash ^= u32::from(byte);
    }
    hash
}

fn bash_assoc_bucket(value: &str) -> u32 {
    bash_hash(value) & 1023
}

fn bash_alias_bucket(value: &str) -> u32 {
    bash_hash(value) & 63
}

fn bash_command_bucket(value: &str) -> u32 {
    bash_hash(value) & 255
}

fn apply_case_attrs(mut value: String, attrs: VarAttrs) -> String {
    if attrs.contains(VarAttrs::UPPERCASE) {
        value = value.to_uppercase();
    }
    if attrs.contains(VarAttrs::LOWERCASE) {
        value = value.to_lowercase();
    }
    if attrs.contains(VarAttrs::CAPCASE) {
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars();
        if let Some(first) = chars.next() {
            if first.is_alphabetic() {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
        }
        value = out;
    }
    value
}

fn bash_progcomp_bucket(value: &str) -> u32 {
    bash_hash(value) & 511
}

fn default_shopt_value(name: &str, interactive: bool, posix: bool) -> bool {
    match name {
        "expand_aliases" => interactive,
        "inherit_errexit" if posix => true,
        _ => cherubsh_builtins::shopt_table::lookup(name)
            .map(|option| option.default)
            .unwrap_or(false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMode {
    Script = 0,
    Interactive = 1,
    DashC = 2,
}

#[derive(Debug, Default, Clone)]
pub struct VariableEntry {
    pub value: String,
    pub has_value: bool,
    pub exported: bool,
    pub readonly: bool,
    pub attrs: VarAttrs,
}

#[derive(Debug, Default, Clone)]
pub struct IndexedArray {
    dense: Vec<Option<String>>,
    sparse: BTreeMap<i64, String>,
    len: usize,
}

impl IndexedArray {
    const MAX_DENSE_GAP: i64 = 1024;
    const MAX_DENSE_LEN: usize = 65_536;

    fn from_values(values: Vec<String>) -> Self {
        let len = values.len();
        Self {
            dense: values.into_iter().map(Some).collect(),
            sparse: BTreeMap::new(),
            len,
        }
    }

    fn from_pairs(values: impl IntoIterator<Item = (i64, String)>) -> Self {
        let mut array = Self::default();
        for (idx, value) in values {
            array.insert(idx, value);
        }
        array
    }

    fn should_store_dense(&self, index: i64) -> bool {
        if index < 0 {
            return false;
        }
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        if index < self.dense.len() {
            return true;
        }
        index < Self::MAX_DENSE_LEN && index <= self.dense.len() + Self::MAX_DENSE_GAP as usize
    }

    fn insert(&mut self, index: i64, value: String) {
        if let Ok(index_usize) = usize::try_from(index) {
            if index_usize == self.dense.len() && index_usize < Self::MAX_DENSE_LEN {
                self.dense.push(Some(value));
                self.len += 1;
                return;
            }
        }
        if self.should_store_dense(index) {
            let index = index as usize;
            if let Some(old) = self.sparse.remove(&(index as i64)) {
                let _ = old;
                self.len = self.len.saturating_sub(1);
            }
            if index >= self.dense.len() {
                self.dense.resize_with(index + 1, || None);
            }
            if self.dense[index].is_none() {
                self.len += 1;
            }
            self.dense[index] = Some(value);
            return;
        }
        if index >= 0 {
            let idx = index as usize;
            if idx < self.dense.len() {
                if self.dense[idx].take().is_some() {
                    self.len = self.len.saturating_sub(1);
                }
            }
        }
        if self.sparse.insert(index, value).is_none() {
            self.len += 1;
        }
        self.trim_dense_tail();
    }

    fn get(&self, index: i64) -> Option<&str> {
        if index >= 0 {
            let idx = index as usize;
            if idx < self.dense.len() {
                return self.dense[idx].as_deref();
            }
        }
        self.sparse.get(&index).map(String::as_str)
    }

    fn remove(&mut self, index: i64) -> Option<String> {
        if index >= 0 {
            let idx = index as usize;
            if idx < self.dense.len() {
                let old = self.dense[idx].take();
                if old.is_some() {
                    self.len = self.len.saturating_sub(1);
                    self.trim_dense_tail();
                    return old;
                }
            }
        }
        let old = self.sparse.remove(&index);
        if old.is_some() {
            self.len = self.len.saturating_sub(1);
        }
        old
    }

    fn trim_dense_tail(&mut self) {
        while self.dense.last().is_some_and(Option::is_none) {
            self.dense.pop();
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn max_index(&self) -> Option<i64> {
        let dense_max = self
            .dense
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, value)| value.as_ref().map(|_| idx as i64));
        match (dense_max, self.sparse.keys().next_back().copied()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn values(&self) -> Vec<String> {
        self.all().into_iter().map(|(_, value)| value).collect()
    }

    fn keys(&self) -> Vec<i64> {
        if self.sparse.is_empty() {
            return self
                .dense
                .iter()
                .enumerate()
                .filter_map(|(idx, value)| value.as_ref().map(|_| idx as i64))
                .collect();
        }
        self.all().into_iter().map(|(idx, _)| idx).collect()
    }

    fn all(&self) -> Vec<(i64, String)> {
        let mut entries = Vec::with_capacity(self.len);
        for (idx, value) in self.dense.iter().enumerate() {
            if let Some(value) = value {
                entries.push((idx as i64, value.clone()));
            }
        }
        entries.extend(self.sparse.iter().map(|(idx, value)| (*idx, value.clone())));
        if !self.sparse.is_empty() {
            entries.sort_by_key(|(idx, _)| *idx);
        }
        entries
    }
}

/// Saved value of one variable for restoration when a local scope is popped.
#[derive(Debug, Clone)]
pub struct SavedVar {
    pub entry: Option<VariableEntry>,
    pub indexed: Option<IndexedArray>,
    pub assoc: Option<HashMap<String, String>>,
    pub nameref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SavedOptions {
    pub allexport: bool,
    pub errexit: bool,
    pub nounset: bool,
    pub noglob: bool,
    pub hashall: bool,
    pub keyword: bool,
    pub pipefail: bool,
    pub lastpipe: bool,
    pub noclobber: bool,
    pub xtrace: bool,
    pub braceexpand: bool,
    pub errtrace: bool,
    pub functrace: bool,
    pub physical: bool,
    pub notify: bool,
    pub monitor: bool,
    pub job_control: bool,
    pub noexec: bool,
    pub just_one_command: bool,
    pub privileged_mode: bool,
    pub verbose_flag: bool,
    pub restricted: bool,
    pub posixly_correct: bool,
    pub shopt_options: BTreeMap<String, bool>,
}

pub struct ShellState {
    pub interactive: bool,
    pub interactive_shell: bool,
    pub login_shell: i32,
    pub posixly_correct: bool,
    pub act_like_sh: bool,
    pub su_shell: bool,
    pub forced_interactive: bool,
    pub read_from_stdin: bool,
    pub want_pending_command: bool,
    pub just_one_command: bool,
    pub no_rc: bool,
    pub no_profile: bool,
    pub no_line_editing: bool,
    pub want_initial_help: bool,
    pub do_version: bool,
    pub make_login_shell: bool,
    pub privileged_mode: bool,
    pub restricted: bool,
    pub debugging: bool,
    pub noexec: bool,
    pub wordexp_only: bool,
    pub pretty_print_mode: bool,
    pub verbose_flag: bool,
    pub command_execution_string: Option<String>,
    pub shell_script_filename: Option<PathBuf>,
    pub shell_name: String,
    pub bashrc_file: PathBuf,
    pub dollar_vars: Vec<String>,
    pub last_command_exit_value: i32,
    pub indirection_level: i32,
    pub current_command_number: u64,
    pub executing: bool,
    pub eof_reached: bool,
    pub current_command_line_count: u32,
    pub diagnostic_line_stack: Vec<u32>,
    pub seconds_start: Instant,
    pub shell_start_epoch: i64,
    pub seconds_offset: i64,
    pub seconds_dynamic: bool,
    pub epochseconds_dynamic: bool,
    pub epochrealtime_dynamic: bool,
    pub bash_argv0_dynamic: bool,
    pub random_seed: Cell<u32>,
    pub last_random_value: Cell<i32>,
    pub startup_state: StartupMode,
    pub running_setuid: bool,
    pub shell_initialized: bool,
    pub input: BashInput,
    pub input_stack: Vec<BashInput>,
    pub input_line_count_stack: Vec<u32>,
    pub variables: HashMap<String, VariableEntry>,
    pub sourced_env: u32,
    pub allexport: bool,
    pub errexit: bool,
    pub nounset: bool,
    pub noglob: bool,
    pub hashall: bool,
    pub keyword: bool,
    pub pipefail: bool,
    pub lastpipe: bool,
    pub noclobber: bool,
    pub xtrace: bool,
    pub braceexpand: bool,
    pub errtrace: bool,
    pub functrace: bool,
    pub physical: bool,
    pub notify: bool,
    pub monitor: bool,
    pub subshell_environment: bool,
    pub need_here_doc: bool,
    pub last_async_pid: Option<i32>,
    pub pending_coproc_cleanups: Vec<(String, Option<String>)>,
    pub subshell_level: u32,
    pub bashpid_cache: Option<i32>,
    pub shell_pid_value: Option<i32>,
    pub funcname_stack: Vec<String>,
    pub funcname_source_stack: Vec<String>,
    pub funcname_lineno_stack: Vec<u32>,
    pub bash_argc_stack: Vec<usize>,
    pub bash_argv_stack: Vec<String>,
    pub indexed_arrays: HashMap<String, IndexedArray>,
    pub assoc_arrays: HashMap<String, HashMap<String, String>>,
    pub assoc_print_orders: HashMap<String, Vec<String>>,
    pub preserve_assoc_order_once: HashSet<String>,
    pub nameref_targets: HashMap<String, String>,
    pub local_scopes: Vec<HashMap<String, SavedVar>>,
    pub local_option_scopes: Vec<Option<SavedOptions>>,
    pub aliases: BTreeMap<String, String>,
    pub aliases_enabled: bool,
    pub traps: BTreeMap<String, String>,
    pub trap_actions: HashMap<TrapKind, TrapAction>,
    pub inherited_exit_trap_suppressed: bool,
    pub command_hash: BTreeMap<String, PathBuf>,
    pub command_hash_hits: BTreeMap<String, u64>,
    pub dirs_stack: Vec<PathBuf>,
    pub disabled_builtins: BTreeMap<String, bool>,
    pub function_readonly: std::collections::HashSet<String>,
    pub shopt_options: BTreeMap<String, bool>,

    pub jobs: JobTable,
    pub shell_pgrp_value: i32,
    pub original_pgrp_value: i32,
    pub tty_fd_value: Option<i32>,
    pub job_control: bool,
    pub running_trap_sig: Option<i32>,
    pub history_table: HistoryTable,
    pub history_last_line_added: bool,
    pub histfile: Option<PathBuf>,
    pub histfile_explicit: bool,
    pub histsize: usize,
    pub histfilesize: usize,
    pub histcontrol_flags: HistControl,
    pub compspecs: HashMap<String, CompSpec>,
    pub compspec_order: HashMap<String, u64>,
    pub compspec_next_order: u64,
    pub default_compspec: Option<CompSpec>,
    pub initial_compspec: Option<CompSpec>,
    pub empty_compspec: Option<CompSpec>,
    pub keymaps: HashMap<String, Keymap>,
    pub active_keymap: String,
    pub line_editor: Option<LineEditor>,
}

impl Default for ShellState {
    fn default() -> Self {
        let shell_pid = unsafe { libc::getpid() };
        let shell_pgrp = unsafe { libc::getpgrp() };
        Self {
            interactive: false,
            interactive_shell: false,
            login_shell: 0,
            posixly_correct: false,
            act_like_sh: false,
            su_shell: false,
            forced_interactive: false,
            read_from_stdin: false,
            want_pending_command: false,
            just_one_command: false,
            no_rc: false,
            no_profile: false,
            no_line_editing: false,
            want_initial_help: false,
            do_version: false,
            make_login_shell: false,
            privileged_mode: false,
            restricted: false,
            debugging: false,
            noexec: false,
            wordexp_only: false,
            pretty_print_mode: false,
            verbose_flag: false,
            command_execution_string: None,
            shell_script_filename: None,
            shell_name: String::from("cherubsh"),
            bashrc_file: default_bashrc(),
            dollar_vars: vec![String::from("cherubsh")],
            last_command_exit_value: 0,
            indirection_level: 0,
            current_command_number: 1,
            executing: false,
            eof_reached: false,
            current_command_line_count: 0,
            diagnostic_line_stack: Vec::new(),
            seconds_start: Instant::now(),
            shell_start_epoch: current_epoch_seconds() as i64,
            seconds_offset: 0,
            seconds_dynamic: true,
            epochseconds_dynamic: true,
            epochrealtime_dynamic: true,
            bash_argv0_dynamic: true,
            random_seed: Cell::new(1),
            last_random_value: Cell::new(0),
            startup_state: StartupMode::Script,
            running_setuid: false,
            shell_initialized: false,
            input: BashInput::None,
            input_stack: Vec::new(),
            input_line_count_stack: Vec::new(),
            variables: HashMap::default(),
            sourced_env: 0,
            allexport: false,
            errexit: false,
            nounset: false,
            noglob: false,
            hashall: true,
            keyword: false,
            pipefail: false,
            lastpipe: false,
            noclobber: false,
            xtrace: false,
            braceexpand: true,
            errtrace: false,
            functrace: false,
            physical: false,
            notify: false,
            monitor: false,
            subshell_environment: false,
            need_here_doc: false,
            last_async_pid: None,
            pending_coproc_cleanups: Vec::new(),
            subshell_level: 0,
            bashpid_cache: None,
            shell_pid_value: Some(shell_pid),
            funcname_stack: Vec::new(),
            funcname_source_stack: Vec::new(),
            funcname_lineno_stack: Vec::new(),
            bash_argc_stack: Vec::new(),
            bash_argv_stack: Vec::new(),
            indexed_arrays: HashMap::default(),
            assoc_arrays: HashMap::default(),
            assoc_print_orders: HashMap::default(),
            preserve_assoc_order_once: HashSet::default(),
            nameref_targets: HashMap::default(),
            local_scopes: Vec::new(),
            local_option_scopes: Vec::new(),
            aliases: BTreeMap::new(),
            aliases_enabled: false,
            traps: BTreeMap::new(),
            trap_actions: HashMap::default(),
            inherited_exit_trap_suppressed: false,
            command_hash: BTreeMap::new(),
            command_hash_hits: BTreeMap::new(),
            dirs_stack: Vec::new(),
            disabled_builtins: BTreeMap::new(),
            function_readonly: std::collections::HashSet::new(),
            shopt_options: BTreeMap::new(),
            jobs: JobTable::new(),
            shell_pgrp_value: shell_pgrp,
            original_pgrp_value: shell_pgrp,
            tty_fd_value: None,
            job_control: false,
            running_trap_sig: None,
            history_table: HistoryTable::new(500),
            history_last_line_added: false,
            histfile: None,
            histfile_explicit: false,
            histsize: 500,
            histfilesize: 500,
            histcontrol_flags: HistControl::empty(),
            compspecs: HashMap::default(),
            compspec_order: HashMap::default(),
            compspec_next_order: 0,
            default_compspec: None,
            initial_compspec: None,
            empty_compspec: None,
            keymaps: HashMap::default(),
            active_keymap: String::from("emacs"),
            line_editor: None,
        }
    }
}

fn default_keymap(name: &str) -> Option<Keymap> {
    let mut keymap = Keymap::new(name);
    match name {
        "emacs" => keymap.install_emacs_defaults(),
        "vi-insert" => keymap.install_vi_insert_defaults(),
        "vi-command" => keymap.install_vi_movement_defaults(),
        _ => return None,
    }
    Some(keymap)
}

impl ShellState {
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

    fn report_removing_nameref_attribute(&self, name: &str) {
        if let (Some(source), Some(line)) = (self.diagnostic_source_name(), self.diagnostic_line())
        {
            eprintln!("{source}: line {line}: warning: {name}: removing nameref attribute");
        } else {
            eprintln!("cherubsh: warning: {name}: removing nameref attribute");
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
                    if sz > 0 {
                        self.histfilesize = sz;
                    }
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
        !matches!(
            name,
            "RANDOM"
                | "BASH_ARGV0"
                | "SECONDS"
                | "EPOCHSECONDS"
                | "EPOCHREALTIME"
                | "FUNCNAME"
                | "SHELLOPTS"
                | "HISTSIZE"
                | "HISTFILESIZE"
                | "HISTCONTROL"
                | "HISTFILE"
                | "IGNOREEOF"
        ) && !(self.restricted && matches!(name, "PATH" | "SHELL"))
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
            if fsz > 0 {
                self.histfilesize = fsz;
            }
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
                let _ = self.history_table.load_from(&p);
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
        let name = self.input.name();
        (!name.is_empty()).then(|| name.to_string())
    }

    fn main_source_name(&self) -> String {
        self.script_source_name()
            .unwrap_or_else(|| "main".to_string())
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
        let mut values = self
            .funcname_stack
            .iter()
            .rev()
            .enumerate()
            .map(|(i, value)| (i as i64, value.clone()))
            .collect::<Vec<_>>();
        if !self.funcname_stack.is_empty() {
            values.push((self.funcname_stack.len() as i64, "main".to_string()));
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
        values.push((values.len() as i64, self.main_source_name()));
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
        values.push((values.len() as i64, "0".to_string()));
        values
    }

    fn bash_argc_values(&self) -> Vec<(i64, String)> {
        self.bash_argc_stack
            .iter()
            .rev()
            .enumerate()
            .map(|(i, value)| (i as i64, value.to_string()))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_matches_bash_seeded_sequence() {
        let mut state = ShellState::default();
        state.assign("RANDOM", "42".into()).unwrap();

        assert_eq!(state.get("RANDOM").as_deref(), Some("17772"));
        assert_eq!(state.get("RANDOM").as_deref(), Some("26794"));
        assert_eq!(state.get("RANDOM").as_deref(), Some("1435"));
        assert_eq!(state.get("RANDOM").as_deref(), Some("24388"));
    }

    #[test]
    fn declare_array_preserves_existing_scalar_as_element_zero() {
        let mut state = ShellState::default();
        state.assign("a", "abcde".into()).unwrap();

        state.set_attr("a", VarAttrs::ARRAY, true);

        assert_eq!(state.kind("a"), VarKind::Indexed);
        assert_eq!(state.get("a").as_deref(), Some("abcde"));
        assert_eq!(state.get_array_indexed("a", 0).as_deref(), Some("abcde"));
    }

    #[test]
    fn repeated_array_attr_does_not_seed_empty_element() {
        let mut state = ShellState::default();

        state.set_attr("a", VarAttrs::ARRAY, true);
        state.set_attr("a", VarAttrs::ARRAY, true);

        assert_eq!(state.kind("a"), VarKind::Indexed);
        assert_eq!(state.get_array_indexed("a", 0), None);
    }

    #[test]
    fn indexed_array_dense_sparse_and_unset_semantics() {
        let mut state = ShellState::default();

        state.set_array("a", vec!["zero".into(), "one".into()]);
        state.set_array_indexed("a", 10_000, "far".into());

        assert_eq!(state.array_len("a"), 3);
        assert_eq!(state.array_max_index("a"), Some(10_000));
        assert_eq!(state.array_keys("a"), Some(vec![0, 1, 10_000]));
        assert_eq!(
            state.get_array_all("a"),
            Some(vec![
                (0, "zero".into()),
                (1, "one".into()),
                (10_000, "far".into()),
            ])
        );

        state.unset_array_elem("a", "1");
        assert_eq!(state.array_len("a"), 2);
        assert_eq!(state.array_keys("a"), Some(vec![0, 10_000]));
        assert_eq!(state.get("a").as_deref(), Some("zero"));
    }

    #[test]
    fn indexed_array_negative_subscript_uses_max_index() {
        let mut state = ShellState::default();
        state.set_array("a", vec!["zero".into(), "one".into(), "two".into()]);

        let max = state.array_max_index("a").unwrap();
        let resolved = max + 1 - 1;

        assert_eq!(
            state.get_array_indexed("a", resolved).as_deref(),
            Some("two")
        );
    }

    #[test]
    fn assoc_len_is_direct() {
        let mut state = ShellState::default();
        state.set_array_assoc("m", "k1", "v1".into());
        state.set_array_assoc("m", "k2", "v2".into());

        assert_eq!(state.assoc_len("m"), 2);
    }

    #[test]
    fn assoc_key_zero_is_scalar_value() {
        let mut state = ShellState::default();
        state.set_attr("m", VarAttrs::ASSOC, true);
        state.set_array_assoc("m", "0", "zero".into());

        assert_eq!(state.get("m").as_deref(), Some("zero"));
        assert_eq!(state.get_array_assoc("m", "0").as_deref(), Some("zero"));
    }
}

impl Environment for ShellState {
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
                if let Some(command) = self.command_execution_string.as_ref() {
                    return Some(Cow::Borrowed(command.trim_end_matches('\n')));
                }
            }
            "BASH_SUBSHELL" => return Some(Cow::Owned(self.subshell_level.to_string())),
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
            "RANDOM" => loop {
                let (next_seed, value) = bash_random_from_seed(self.random_seed.get());
                self.random_seed.set(next_seed);
                if value != self.last_random_value.get() {
                    self.last_random_value.set(value);
                    return Some(Cow::Owned(value.to_string()));
                }
            },
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
            return entry.has_value.then(|| Cow::Borrowed(entry.value.as_str()));
        }
        std::env::var(name).ok().map(Cow::Owned)
    }

    fn set(&mut self, name: &str, value: String) {
        let _ = self.assign(name, value);
    }

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
            "FUNCNAME" => return Ok(()),
            "SHELLOPTS" => return Err(AssignError::ReadOnly(name.to_string())),
            _ => {}
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
                    self.report_circular_name_reference(name);
                    return self.with_global_var(name, |state| {
                        state.assign_nameref_target(name, name, value)
                    });
                }
                let Some(target) = self.resolve_nameref(name) else {
                    self.report_circular_name_reference(name);
                    return Ok(());
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
        if name == "SHELLOPTS" {
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
            "FUNCNAME" => return,
            "HISTFILE" => {
                self.histfile_explicit = true;
                self.histfile = None;
            }
            "IGNOREEOF" => {
                self.shopt_options.insert("ignoreeof".to_string(), false);
            }
            "SHELLOPTS" => return,
            _ => {}
        }
        let current_scope_has_name = self
            .local_scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name));
        if !current_scope_has_name {
            if let Some(scope_idx) = self.global_scope_index(name) {
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

    fn last_status(&self) -> i32 {
        self.last_command_exit_value
    }

    fn set_last_status(&mut self, status: i32) {
        self.last_command_exit_value = status;
    }

    fn diagnostic_source_name(&self) -> Option<String> {
        if let Some(source) = self.funcname_source_stack.last() {
            return Some(source.clone());
        }
        if self.startup_state == StartupMode::DashC {
            return Some(self.shell_name.clone());
        }
        let name = self.input.name();
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    }

    fn diagnostic_line(&self) -> Option<u32> {
        if let Some(line) = self.diagnostic_line_stack.last() {
            return Some(*line);
        }
        (self.current_command_line_count > 0).then_some(self.current_command_line_count)
    }

    fn push_diagnostic_line(&mut self, line: u32) {
        self.diagnostic_line_stack.push(line);
    }

    fn pop_diagnostic_line(&mut self) {
        self.diagnostic_line_stack.pop();
    }

    fn arithmetic_expansion_errors_exit_shell(&self) -> bool {
        self.startup_state == StartupMode::DashC
    }

    fn is_login_shell(&self) -> bool {
        self.login_shell != 0
    }

    fn option(&self, name: &str) -> bool {
        match name {
            "allexport" | "a" => self.allexport,
            "errexit" | "e" => self.errexit,
            "noglob" | "f" => self.noglob,
            "hashall" | "h" => self.hashall,
            "keyword" | "k" => self.keyword,
            "nounset" | "u" => self.nounset,
            "pipefail" => self.pipefail,
            "lastpipe" => self.lastpipe,
            "noclobber" | "C" => self.noclobber,
            "xtrace" | "x" => self.xtrace,
            "braceexpand" | "B" => self.braceexpand,
            "errtrace" | "E" => self.errtrace,
            "functrace" | "T" => self.functrace,
            "physical" | "P" => self.physical,
            "notify" | "b" => self.notify,
            "monitor" | "m" => self.monitor,
            "ignoreeof" => self.shopt_options.get(name).copied().unwrap_or(false),
            "interactive-comments" => self.shopt_options.get(name).copied().unwrap_or(true),
            "emacs" | "history" | "nolog" | "vi" => {
                self.shopt_options.get(name).copied().unwrap_or(false)
            }
            "noexec" | "n" => self.noexec,
            "onecmd" | "t" => self.just_one_command,
            "privileged" | "p" => self.privileged_mode,
            "verbose" | "v" => self.verbose_flag,
            "histexpand" | "H" => self
                .shopt_options
                .get("histexpand")
                .copied()
                .unwrap_or(false),
            "restricted" | "r" => self.restricted,
            "posix" => self.posixly_correct,
            "interactive" | "i" => self.interactive,
            _ => self.shopt_options.get(name).copied().unwrap_or_else(|| {
                default_shopt_value(name, self.interactive, self.posixly_correct)
            }),
        }
    }

    fn set_option(&mut self, name: &str, on: bool) {
        match name {
            "allexport" | "a" => self.allexport = on,
            "errexit" | "e" => self.errexit = on,
            "noglob" | "f" => self.noglob = on,
            "hashall" | "h" => self.hashall = on,
            "keyword" | "k" => self.keyword = on,
            "nounset" | "u" => self.nounset = on,
            "pipefail" => self.pipefail = on,
            "lastpipe" => self.lastpipe = on,
            "noclobber" | "C" => self.noclobber = on,
            "xtrace" | "x" => self.xtrace = on,
            "braceexpand" | "B" => self.braceexpand = on,
            "errtrace" | "E" => self.errtrace = on,
            "functrace" | "T" => self.functrace = on,
            "physical" | "P" => self.physical = on,
            "notify" | "b" => self.notify = on,
            "monitor" | "m" => {
                self.monitor = on;
                self.job_control = on;
            }
            "ignoreeof" => {
                self.shopt_options.insert(name.to_string(), on);
                if on {
                    self.variables.insert(
                        "IGNOREEOF".to_string(),
                        VariableEntry {
                            value: "10".to_string(),
                            has_value: true,
                            exported: false,
                            readonly: false,
                            attrs: VarAttrs::empty(),
                        },
                    );
                } else {
                    self.variables.remove("IGNOREEOF");
                    std::env::remove_var("IGNOREEOF");
                }
            }
            "interactive-comments" => {
                self.shopt_options.insert(name.to_string(), on);
            }
            "noexec" | "n" => self.noexec = on,
            "onecmd" | "t" => self.just_one_command = on,
            "privileged" | "p" => self.privileged_mode = on,
            "verbose" | "v" => self.verbose_flag = on,
            "histexpand" | "H" => {
                self.shopt_options.insert("histexpand".to_string(), on);
            }
            "history" => {
                let was_on = self.shopt_options.get("history").copied().unwrap_or(false);
                self.shopt_options.insert("history".to_string(), on);
                if on && !was_on {
                    self.configure_history_from_vars(true, false);
                }
            }
            "expand_aliases" => {
                self.aliases_enabled = on;
                self.shopt_options.insert(name.to_string(), on);
            }
            "restricted" | "r" => self.restricted = on,
            "posix" => {
                self.posixly_correct = on;
                if on {
                    self.shopt_options
                        .insert("inherit_errexit".to_string(), true);
                }
            }
            _ => {
                self.shopt_options.insert(name.to_string(), on);
            }
        }
    }

    fn last_async_pid(&self) -> Option<i32> {
        self.last_async_pid
    }

    fn set_last_async_pid(&mut self, pid: i32) {
        self.last_async_pid = Some(pid);
    }

    fn queue_coproc_cleanup(&mut self, name: String, pid_name: Option<String>) {
        self.pending_coproc_cleanups.push((name, pid_name));
    }

    fn take_coproc_cleanups(&mut self) -> Vec<(String, Option<String>)> {
        std::mem::take(&mut self.pending_coproc_cleanups)
    }

    fn prepare_nameref_target_assignment(&mut self, source: &str, target: &str) {
        if self.local_scopes.is_empty()
            || !self
                .local_scopes
                .last()
                .is_some_and(|scope| scope.contains_key(source))
        {
            return;
        }
        let base = state_array_reference(target)
            .map(|(name, _)| name)
            .unwrap_or(target);
        if base == source
            || self.variables.contains_key(base)
            || self.indexed_arrays.contains_key(base)
            || self.assoc_arrays.contains_key(base)
            || self.nameref_targets.contains_key(base)
            || std::env::var(base).is_ok()
        {
            return;
        }
        let _ = self.make_local(base);
    }

    fn shell_pid(&self) -> i32 {
        if let Some(v) = self.shell_pid_value {
            return v;
        }
        unsafe { libc::getpid() }
    }

    fn bashpid(&self) -> i32 {
        if let Some(v) = self.bashpid_cache {
            return v;
        }
        unsafe { libc::getpid() }
    }

    fn shell_start_epoch(&self) -> i64 {
        self.shell_start_epoch
    }

    fn subshell_level(&self) -> u32 {
        self.subshell_level
    }

    fn enter_subshell(&mut self) {
        self.subshell_level = self.subshell_level.saturating_add(1);
        self.bashpid_cache = Some(unsafe { libc::getpid() });
        self.subshell_environment = true;
        self.last_async_pid = None;
    }

    fn funcname_push(&mut self, name: &str, args: &[String]) {
        self.funcname_stack.push(name.to_string());
        self.funcname_source_stack.push(self.current_source_name());
        self.funcname_lineno_stack.push(self.current_call_line());
        self.bash_argc_stack.push(args.len());
        let mut values = args.iter().rev().cloned().collect::<Vec<_>>();
        values.append(&mut self.bash_argv_stack);
        self.bash_argv_stack = values;
    }

    fn funcname_pop(&mut self) {
        self.funcname_stack.pop();
        self.funcname_source_stack.pop();
        self.funcname_lineno_stack.pop();
        if let Some(argc) = self.bash_argc_stack.pop() {
            let end = argc.min(self.bash_argv_stack.len());
            self.bash_argv_stack.drain(0..end);
        }
    }

    fn source_frame_push(&mut self, source_name: &str) {
        self.funcname_stack.push("source".to_string());
        self.funcname_source_stack.push(source_name.to_string());
        self.funcname_lineno_stack.push(self.current_call_line());
        self.bash_argc_stack.push(0);
    }

    fn source_frame_pop(&mut self) {
        self.funcname_stack.pop();
        self.funcname_source_stack.pop();
        self.funcname_lineno_stack.pop();
        self.bash_argc_stack.pop();
    }

    fn set_array(&mut self, name: &str, values: Vec<String>) {
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
        let scalar = values.get(0).cloned().unwrap_or_default();
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
                if entry.attrs.contains(VarAttrs::ARRAY) && !entry.attrs.intersects(case_attrs) {
                    bt.insert(index, value);
                    return;
                }
            }
        }
        let indexed_exists = self.indexed_arrays.contains_key(name);
        let (prior_attrs, exported, has_array_attr, seed_scalar) = match self.variables.get(name) {
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
            let index = index as usize;
            if index < self.funcname_stack.len() {
                return self
                    .funcname_stack
                    .get(self.funcname_stack.len() - 1 - index)
                    .cloned();
            }
            if index == self.funcname_stack.len() && !self.funcname_stack.is_empty() {
                return Some("main".to_string());
            }
            return None;
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
            let index = index as usize;
            if index < self.funcname_stack.len() {
                return self
                    .funcname_stack
                    .get(self.funcname_stack.len() - 1 - index)
                    .map(|value| Cow::Borrowed(value.as_str()));
            }
            if index == self.funcname_stack.len() && !self.funcname_stack.is_empty() {
                return Some(Cow::Borrowed("main"));
            }
            return None;
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
            self.aliases.insert(key.to_string(), value);
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
                if entry.attrs.contains(VarAttrs::ASSOC) && !entry.attrs.intersects(case_attrs) {
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
        if name == "SHELLOPTS" {
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
        if name == "SHELLOPTS" {
            return VarAttrs::READONLY;
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
            if existed && !had_indexed && !was_array {
                if entry.has_value {
                    let scalar = entry.value.clone();
                    let bt = self.indexed_arrays.entry(name.to_string()).or_default();
                    if bt.get(0).is_none() {
                        bt.insert(0, scalar);
                    }
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

    fn resolve_nameref(&self, name: &str) -> Option<String> {
        let mut cur = name.to_string();
        for _ in 0..32 {
            match self.nameref_targets.get(&cur) {
                Some(target) if target != &cur => cur = target.clone(),
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
            } else {
                self.attrs(&name)
            };
            let scalar = if special_indexed.is_some() {
                None
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
                | "SHELLOPTS"
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
        } else {
            self.attrs(name)
        };
        let scalar = if special_indexed.is_some() {
            None
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

    fn make_options_local(&mut self) {
        let snapshot = self.snapshot_options();
        if let Some(slot) = self.local_option_scopes.last_mut() {
            slot.get_or_insert(snapshot);
        }
    }

    fn alias_set(&mut self, name: &str, value: String) {
        self.aliases.insert(name.to_string(), value);
    }
    fn alias_get(&self, name: &str) -> Option<String> {
        self.aliases.get(name).cloned()
    }
    fn alias_unset(&mut self, name: &str) {
        self.aliases.remove(name);
    }
    fn alias_iter(&self) -> Vec<(String, String)> {
        self.aliases
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
    fn aliases_enabled(&self) -> bool {
        self.aliases_enabled
    }
    fn set_aliases_enabled(&mut self, on: bool) {
        self.aliases_enabled = on;
    }

    fn umask_get(&self) -> u32 {
        unsafe {
            let cur = libc::umask(0);
            libc::umask(cur);
            cur as u32 & 0o777
        }
    }
    fn umask_set(&mut self, mask: u32) {
        unsafe { libc::umask(mask as libc::mode_t) };
    }

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

    fn hash_set(&mut self, name: &str, path: std::path::PathBuf) {
        self.command_hash.insert(name.to_string(), path);
        self.command_hash_hits.insert(name.to_string(), 0);
    }
    fn hash_get(&self, name: &str) -> Option<std::path::PathBuf> {
        self.command_hash.get(name).cloned()
    }
    fn hash_get_with_hit(&mut self, name: &str) -> Option<std::path::PathBuf> {
        let path = self.command_hash.get(name).cloned();
        if path.is_some() {
            *self.command_hash_hits.entry(name.to_string()).or_insert(0) += 1;
        }
        path
    }
    fn hash_remove(&mut self, name: &str) {
        self.command_hash.remove(name);
        self.command_hash_hits.remove(name);
    }
    fn hash_clear(&mut self) {
        self.command_hash.clear();
        self.command_hash_hits.clear();
    }
    fn hash_iter(&self) -> Vec<(String, std::path::PathBuf)> {
        self.command_hash
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
    fn hash_iter_with_hits(&self) -> Vec<(String, std::path::PathBuf, u64)> {
        let mut entries = self
            .command_hash
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.clone(),
                    self.command_hash_hits.get(k).copied().unwrap_or(0),
                )
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|(k, _, _)| bash_command_bucket(k));
        entries
    }

    fn dirs_iter(&self) -> Vec<std::path::PathBuf> {
        let mut out = Vec::with_capacity(self.dirs_stack.len() + 1);
        if let Ok(cur) = std::env::current_dir() {
            out.push(cur);
        }
        out.extend(self.dirs_stack.iter().cloned());
        out
    }
    fn dirs_push(&mut self, path: std::path::PathBuf) {
        self.dirs_stack.insert(0, path);
    }
    fn dirs_pop(&mut self, index: usize) -> Option<std::path::PathBuf> {
        if index >= self.dirs_stack.len() {
            return None;
        }
        Some(self.dirs_stack.remove(index))
    }
    fn dirs_set_top(&mut self, path: std::path::PathBuf) {
        if !self.dirs_stack.is_empty() {
            self.dirs_stack[0] = path;
        } else {
            self.dirs_stack.push(path);
        }
    }
    fn dirs_set_stack(&mut self, stack: Vec<std::path::PathBuf>) {
        self.dirs_stack = stack;
    }
    fn dirs_clear(&mut self) {
        self.dirs_stack.clear();
    }

    fn builtin_enabled(&self, name: &str) -> bool {
        !self.disabled_builtins.get(name).copied().unwrap_or(false)
    }
    fn builtin_set_enabled(&mut self, name: &str, on: bool) {
        if on {
            self.disabled_builtins.remove(name);
        } else {
            self.disabled_builtins.insert(name.to_string(), true);
        }
    }

    fn function_is_readonly(&self, name: &str) -> bool {
        self.function_readonly.contains(name)
    }
    fn function_set_readonly(&mut self, name: &str) {
        self.function_readonly.insert(name.to_string());
    }

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

    fn jobs_table(&self) -> Option<&JobTable> {
        Some(&self.jobs)
    }
    fn jobs_table_mut(&mut self) -> Option<&mut JobTable> {
        Some(&mut self.jobs)
    }

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

    fn compspec_set(&mut self, slot: CompSlot, key: Option<&str>, spec: CompSpec) {
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

    fn pending_signal_take(&mut self, sig: i32) -> u32 {
        crate::signals::pending_signal_take(sig)
    }
    fn running_trap(&self) -> Option<i32> {
        self.running_trap_sig
    }
    fn set_running_trap(&mut self, sig: Option<i32>) {
        self.running_trap_sig = sig;
    }
    fn run_debug_trap_hook(&mut self) {
        crate::traps::run_debug_trap(self);
    }
    fn run_err_trap_hook(&mut self) {
        crate::traps::run_err_trap(self);
    }
    fn run_return_trap_hook(&mut self) {
        crate::traps::run_return_trap(self);
    }
    fn run_pending_traps_hook(&mut self) {
        crate::traps::run_pending_traps(self);
    }
    fn run_exit_trap_hook(&mut self) -> Option<i32> {
        crate::traps::run_exit_trap(self)
    }
    fn shell_pgrp(&self) -> i32 {
        self.shell_pgrp_value
    }
    fn set_shell_pgrp(&mut self, pgrp: i32) {
        self.shell_pgrp_value = pgrp;
    }
    fn tty_fd(&self) -> Option<i32> {
        self.tty_fd_value
    }
    fn set_tty_fd(&mut self, fd: Option<i32>) {
        self.tty_fd_value = fd;
    }
    fn job_control_enabled(&self) -> bool {
        self.job_control
    }
    fn set_job_control_enabled(&mut self, on: bool) {
        self.job_control = on;
    }
}

/// Map raw input like "0", "9", "INT", "SIGINT" to a canonical short name
/// usable as a `traps` map key. Reserved names (`EXIT`, `ERR`, `RETURN`,
/// `DEBUG`) pass through.
pub fn canonical_trap_signal(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
    match stripped {
        "0" => return "EXIT".to_string(),
        "EXIT" | "ERR" | "RETURN" | "DEBUG" => return stripped.to_string(),
        _ => {}
    }
    if let Ok(num) = stripped.parse::<i32>() {
        if let Some(short) = signal_short_name(num) {
            return short.to_string();
        }
    }
    stripped.to_string()
}

pub fn signal_short_name(num: i32) -> Option<&'static str> {
    Some(match num {
        1 => "HUP",
        2 => "INT",
        3 => "QUIT",
        4 => "ILL",
        5 => "TRAP",
        6 => "ABRT",
        7 => "BUS",
        8 => "FPE",
        9 => "KILL",
        10 => "USR1",
        11 => "SEGV",
        12 => "USR2",
        13 => "PIPE",
        14 => "ALRM",
        15 => "TERM",
        16 => "STKFLT",
        17 => "CHLD",
        18 => "CONT",
        19 => "STOP",
        20 => "TSTP",
        21 => "TTIN",
        22 => "TTOU",
        23 => "URG",
        24 => "XCPU",
        25 => "XFSZ",
        26 => "VTALRM",
        27 => "PROF",
        28 => "WINCH",
        29 => "IO",
        30 => "PWR",
        31 => "SYS",
        _ => return None,
    })
}

fn signal_number_for_short_name(name: &str) -> Option<i32> {
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "TRAP" => 5,
        "ABRT" => 6,
        "BUS" => 7,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        "STKFLT" => 16,
        "CHLD" => 17,
        "CONT" => 18,
        "STOP" => 19,
        "TSTP" => 20,
        "TTIN" => 21,
        "TTOU" => 22,
        "URG" => 23,
        "XCPU" => 24,
        "XFSZ" => 25,
        "VTALRM" => 26,
        "PROF" => 27,
        "WINCH" => 28,
        "IO" => 29,
        "PWR" => 30,
        "SYS" => 31,
        _ => return None,
    })
}

fn startup_ignored_trap_action(canonical_signal: &str) -> Option<String> {
    let sig = signal_number_for_short_name(canonical_signal)?;
    if sig != libc::SIGPIPE && crate::signals::signal_ignored_at_start(sig) {
        Some(String::new())
    } else {
        None
    }
}

fn state_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn state_array_reference(value: &str) -> Option<(&str, &str)> {
    let open = value.find('[')?;
    let close = value.rfind(']')?;
    if close + 1 != value.len() {
        return None;
    }
    let name = &value[..open];
    if !state_valid_name(name) {
        return None;
    }
    let subscript = &value[open + 1..close];
    if subscript.starts_with(['[', ']']) {
        return None;
    }
    Some((name, subscript))
}

fn state_nameref_target_is_valid(target: &str) -> bool {
    state_valid_name(target) || state_array_reference(target).is_some()
}

impl ShellState {
    fn option_letters(&self) -> String {
        let mut letters = String::new();
        if self.interactive {
            letters.push('i');
        }
        if self.read_from_stdin {
            letters.push('s');
        }
        if self.allexport {
            letters.push('a');
        }
        if self.errexit {
            letters.push('e');
        }
        if self.noglob {
            letters.push('f');
        }
        if self.hashall {
            letters.push('h');
        }
        if self.keyword {
            letters.push('k');
        }
        if self.monitor {
            letters.push('m');
        }
        if self.noexec {
            letters.push('n');
        }
        if self.privileged_mode {
            letters.push('p');
        }
        if self.just_one_command {
            letters.push('t');
        }
        if self.nounset {
            letters.push('u');
        }
        if self.verbose_flag {
            letters.push('v');
        }
        if self.xtrace {
            letters.push('x');
        }
        if self.braceexpand {
            letters.push('B');
        }
        if self.noclobber {
            letters.push('C');
        }
        if self.errtrace {
            letters.push('E');
        }
        if self.option("histexpand") {
            letters.push('H');
        }
        if self.physical {
            letters.push('P');
        }
        if self.functrace {
            letters.push('T');
        }
        letters
    }

    fn shellopts_value(&self) -> String {
        cherubsh_builtins::options::SET_OPTIONS
            .iter()
            .filter_map(|opt| self.option(opt.long).then_some(opt.long))
            .collect::<Vec<_>>()
            .join(":")
    }

    fn sync_export_env(&self, name: &str) {
        if let Some(entry) = self.variables.get(name) {
            if entry.exported && entry.has_value {
                std::env::set_var(name, &entry.value);
                return;
            }
            if let Some(saved_entry) = self
                .global_saved_snapshot(name)
                .and_then(|saved| saved.entry.as_ref())
            {
                if saved_entry.exported && saved_entry.has_value {
                    std::env::set_var(name, &saved_entry.value);
                    return;
                }
            }
        }
        std::env::remove_var(name);
    }

    fn snapshot_options(&self) -> SavedOptions {
        SavedOptions {
            allexport: self.allexport,
            errexit: self.errexit,
            nounset: self.nounset,
            noglob: self.noglob,
            hashall: self.hashall,
            keyword: self.keyword,
            pipefail: self.pipefail,
            lastpipe: self.lastpipe,
            noclobber: self.noclobber,
            xtrace: self.xtrace,
            braceexpand: self.braceexpand,
            errtrace: self.errtrace,
            functrace: self.functrace,
            physical: self.physical,
            notify: self.notify,
            monitor: self.monitor,
            job_control: self.job_control,
            noexec: self.noexec,
            just_one_command: self.just_one_command,
            privileged_mode: self.privileged_mode,
            verbose_flag: self.verbose_flag,
            restricted: self.restricted,
            posixly_correct: self.posixly_correct,
            shopt_options: self.shopt_options.clone(),
        }
    }

    fn restore_options(&mut self, saved: SavedOptions) {
        self.allexport = saved.allexport;
        self.errexit = saved.errexit;
        self.nounset = saved.nounset;
        self.noglob = saved.noglob;
        self.hashall = saved.hashall;
        self.keyword = saved.keyword;
        self.pipefail = saved.pipefail;
        self.lastpipe = saved.lastpipe;
        self.noclobber = saved.noclobber;
        self.xtrace = saved.xtrace;
        self.braceexpand = saved.braceexpand;
        self.errtrace = saved.errtrace;
        self.functrace = saved.functrace;
        self.physical = saved.physical;
        self.notify = saved.notify;
        self.monitor = saved.monitor;
        self.job_control = saved.job_control;
        self.noexec = saved.noexec;
        self.just_one_command = saved.just_one_command;
        self.privileged_mode = saved.privileged_mode;
        self.verbose_flag = saved.verbose_flag;
        self.restricted = saved.restricted;
        self.posixly_correct = saved.posixly_correct;
        self.shopt_options = saved.shopt_options;
        if self.option("ignoreeof") {
            self.variables.insert(
                "IGNOREEOF".to_string(),
                VariableEntry {
                    value: "10".to_string(),
                    has_value: true,
                    exported: false,
                    readonly: false,
                    attrs: VarAttrs::empty(),
                },
            );
        } else {
            self.variables.remove("IGNOREEOF");
            std::env::remove_var("IGNOREEOF");
        }
    }
}
