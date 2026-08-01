struct ReadlineStore {
    editor: Option<LineEditor>,
    line_allocation: usize,
    line_capacity: usize,
    prompt_allocation: usize,
    callback: Option<rl_vcpfunc_t>,
    callback_prompt: String,
    callback_raw_mode: Option<RawMode>,
    callback_prepped: bool,
    callback_buffer: Vec<u8>,
    variables: std::collections::BTreeMap<String, CString>,
    kill_ring: Vec<String>,
    undo: Vec<(String, usize)>,
    filename_generator: Option<(String, Vec<String>, usize)>,
    current_keymap: usize,
    keymap_names: std::collections::BTreeMap<String, usize>,
    saved_line: Option<(String, usize, usize)>,
    mark_active: bool,
    keep_mark_active: bool,
    saved_prompt: Option<String>,
    keyboard_timeout_us: c_int,
    timeout_duration: Option<Duration>,
    timeout_deadline: Option<Instant>,
    macro_recording: bool,
    current_macro: String,
    last_macro: String,
    last_init_file: Option<PathBuf>,
    last_search: Option<(String, bool)>,
    last_vi_operator: Option<(c_int, c_int)>,
    funmap_entries: Vec<usize>,
    funmap_functions: std::collections::BTreeMap<String, usize>,
    funmap_initialized: bool,
    funmap_array: usize,
    signal_handlers: std::collections::BTreeMap<c_int, usize>,
    paren_blink_timeout_us: c_int,
    tty_echoing: c_int,
}

impl ReadlineStore {
    fn new() -> Self {
        let mut keymap = RustKeymap::new("emacs");
        keymap.install_emacs_defaults();
        Self {
            editor: Some(LineEditor::new(keymap)),
            line_allocation: 0,
            line_capacity: 0,
            prompt_allocation: 0,
            callback: None,
            callback_prompt: String::new(),
            callback_raw_mode: None,
            callback_prepped: false,
            callback_buffer: Vec::new(),
            variables: std::collections::BTreeMap::new(),
            kill_ring: Vec::new(),
            undo: Vec::new(),
            filename_generator: None,
            current_keymap: 0,
            keymap_names: std::collections::BTreeMap::new(),
            saved_line: None,
            mark_active: false,
            keep_mark_active: false,
            saved_prompt: None,
            keyboard_timeout_us: 100_000,
            timeout_duration: None,
            timeout_deadline: None,
            macro_recording: false,
            current_macro: String::new(),
            last_macro: String::new(),
            last_init_file: None,
            last_search: None,
            last_vi_operator: None,
            funmap_entries: Vec::new(),
            funmap_functions: std::collections::BTreeMap::new(),
            funmap_initialized: false,
            funmap_array: 0,
            signal_handlers: std::collections::BTreeMap::new(),
            paren_blink_timeout_us: 500_000,
            tty_echoing: 1,
        }
    }
}

fn readline_store() -> &'static Mutex<ReadlineStore> {
    static STORE: OnceLock<Mutex<ReadlineStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(ReadlineStore::new()))
}

unsafe fn set_line_buffer(line: &str, point: usize) {
    let mut store = readline_store().lock().expect("readline lock");
    if store.line_allocation != 0 {
        libc::free(store.line_allocation as *mut c_void);
    }
    let allocation = malloc_string(line);
    store.line_allocation = allocation as usize;
    store.line_capacity = line.len().saturating_add(1);
    rl_line_buffer = allocation;
    rl_end = line.len().min(c_int::MAX as usize) as c_int;
    rl_point = point.min(line.len()).min(c_int::MAX as usize) as c_int;
}

unsafe fn current_line() -> String {
    c_text(rl_line_buffer).unwrap_or_default()
}

unsafe fn line_snapshot() -> (String, usize) {
    (current_line(), rl_point.max(0) as usize)
}

unsafe fn save_undo() {
    let snapshot = line_snapshot();
    let mut store = readline_store().lock().expect("readline lock");
    store.undo.push(snapshot);
    if store.undo.len() > 256 {
        store.undo.remove(0);
    }
}

fn clamp_boundary(text: &str, mut point: usize) -> usize {
    point = point.min(text.len());
    while point > 0 && !text.is_char_boundary(point) {
        point -= 1;
    }
    point
}

struct FfiHistory {
    entries: Vec<String>,
}

impl FfiHistory {
    fn snapshot() -> Self {
        let store = history_store().lock().expect("history lock");
        Self {
            entries: store
                .entries
                .iter()
                .map(|entry| unsafe {
                    c_text((*entry as *mut HIST_ENTRY).as_ref().unwrap().line).unwrap_or_default()
                })
                .collect(),
        }
    }
}

impl HistoryProvider for FfiHistory {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, index: usize) -> Option<String> {
        self.entries.get(index).cloned()
    }
}

struct FfiCompleter;

impl CompletionProvider for FfiCompleter {
    fn complete(&mut self, line: &str, point: usize) -> Completion {
        unsafe { ffi_complete(line, point) }
    }

    fn run_shell_command(
        &mut self,
        command: &str,
        line: &str,
        point: usize,
    ) -> Option<(String, usize)> {
        let payload = command.strip_prefix("readline-function:")?;
        let (address, key) = payload.split_once(':')?;
        let address = address.parse::<usize>().ok()?;
        let key = key.parse::<c_int>().ok()?;
        unsafe {
            set_line_buffer(line, point);
            let function = std::mem::transmute::<usize, rl_command_func_t>(address);
            rl_dispatching = 1;
            rl_readline_state |= RL_STATE_DISPATCHING;
            let status = function(1, key);
            rl_readline_state &= !RL_STATE_DISPATCHING;
            rl_dispatching = 0;
            rl_last_func = Some(function);
            if status < 0 {
                return None;
            }
            Some((current_line(), rl_point.max(0) as usize))
        }
    }
}

unsafe fn ffi_complete(line: &str, point: usize) -> Completion {
    if rl_inhibit_completion != 0 {
        return Completion::default();
    }
    let breaks = c_text(if rl_completer_word_break_characters.is_null() {
        rl_basic_word_break_characters
    } else {
        rl_completer_word_break_characters
    })
    .unwrap_or_else(|| " \t\n".to_string());
    let point = point.min(line.len());
    let start = line[..point]
        .char_indices()
        .rev()
        .find_map(|(index, ch)| breaks.contains(ch).then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let text = &line[start..point];
    let mut matches = if let Some(callback) = rl_attempted_completion_function {
        let text_c = clean_c_string(text);
        let array = callback(text_c.as_ptr(), start as c_int, point as c_int);
        take_completion_array(array, true)
    } else if let Some(generator) = rl_completion_entry_function {
        run_generator(generator, text)
    } else {
        filename_matches(text)
    };
    if rl_sort_completion_matches != 0 {
        matches.sort_by(|left, right| {
            let left = clean_c_string(left);
            let right = clean_c_string(right);
            libc::strcoll(left.as_ptr(), right.as_ptr()).cmp(&0)
        });
    }
    if rl_ignore_completion_duplicates != 0 {
        matches.dedup();
    }
    Completion {
        matches,
        replace_start: start,
        suppress_append: rl_completion_suppress_append != 0,
        append_character: char::from_u32(rl_completion_append_character.max(0) as u32),
        filenames: rl_filename_completion_desired != 0,
    }
}

unsafe fn take_completion_array(array: *mut *mut c_char, skip_common: bool) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    let mut values = Vec::new();
    let mut index = 0usize;
    while !(*array.add(index)).is_null() {
        values.push(c_text(*array.add(index)).unwrap_or_default());
        libc::free((*array.add(index)).cast());
        index += 1;
    }
    libc::free(array.cast());
    if skip_common && values.len() > 1 {
        values.remove(0);
    }
    values
}

unsafe fn run_generator(generator: rl_compentry_func_t, text: &str) -> Vec<String> {
    let text = clean_c_string(text);
    let mut matches = Vec::new();
    for state in 0..100_000 {
        let value = generator(text.as_ptr(), state);
        if value.is_null() {
            break;
        }
        matches.push(c_text(value).unwrap_or_default());
        libc::free(value.cast());
    }
    matches
}

fn filename_matches(text: &str) -> Vec<String> {
    unsafe { rl_filename_completion_desired = 1 };
    let path = PathBuf::from(text);
    let (directory, prefix) = if text.ends_with('/') {
        (path, String::new())
    } else {
        (
            path.parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    let shown_directory = if text.ends_with('/') {
        text.to_string()
    } else {
        text.rsplit_once('/')
            .map_or(String::new(), |(head, _)| format!("{head}/"))
    };
    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && (prefix.starts_with('.') || !name.starts_with('.')) {
                let mut candidate = format!("{shown_directory}{name}");
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    candidate.push('/');
                }
                matches.push(candidate);
            }
        }
    }
    matches
}

fn command_pointer(function: rl_command_func_t) -> *mut c_void {
    function as *const () as *mut c_void
}

fn lineedit_sequence(sequence: &[u8]) -> String {
    let mut rendered = String::new();
    let mut index = 0;
    while index < sequence.len() {
        let key = sequence[index];
        if key == 0x1b && index + 1 < sequence.len() {
            let next = sequence[index + 1];
            if !matches!(next, b'[' | b'O' | 0x1b) && (0x20..=0x7e).contains(&next) {
                rendered.push_str("\\M-");
                rendered.push(next as char);
                index += 2;
                continue;
            }
        }
        rendered.push_str(
            match key {
                b'\t' => "\\t".to_string(),
                b'\n' => "\\C-j".to_string(),
                b'\r' => "\\C-m".to_string(),
                0x08 | 0x7f => "\\C-h".to_string(),
                0x1b => "\\e".to_string(),
                0..=0x1f => format!("\\C-{}", ((key | 0x40) as char).to_ascii_lowercase()),
                b'\\' => "\\\\".to_string(),
                _ => (key as char).to_string(),
            }
            .as_str(),
        );
        index += 1;
    }
    rendered
}

fn edit_action_for_function(function: rl_command_func_t) -> Option<EditAction> {
    let address = command_pointer(function);
    let mappings = [
        (command_pointer(rl_insert), EditAction::SelfInsert),
        (command_pointer(rl_beg_of_line), EditAction::BeginningOfLine),
        (command_pointer(rl_end_of_line), EditAction::EndOfLine),
        (command_pointer(rl_forward_char), EditAction::ForwardChar),
        (command_pointer(rl_backward_char), EditAction::BackwardChar),
        (command_pointer(rl_forward_word), EditAction::ForwardWord),
        (command_pointer(rl_backward_word), EditAction::BackwardWord),
        (command_pointer(rl_delete), EditAction::DeleteChar),
        (command_pointer(rl_rubout), EditAction::BackwardDeleteChar),
        (command_pointer(rl_kill_line), EditAction::KillLine),
        (
            command_pointer(rl_backward_kill_line),
            EditAction::BackwardKillLine,
        ),
        (command_pointer(rl_kill_word), EditAction::KillWord),
        (
            command_pointer(rl_backward_kill_word),
            EditAction::BackwardKillWord,
        ),
        (command_pointer(rl_kill_region), EditAction::KillRegion),
        (command_pointer(rl_yank), EditAction::Yank),
        (command_pointer(rl_yank_pop), EditAction::YankPop),
        (command_pointer(rl_yank_last_arg), EditAction::YankLastArg),
        (command_pointer(rl_yank_nth_arg), EditAction::YankNthArg),
        (
            command_pointer(rl_get_previous_history),
            EditAction::PreviousHistory,
        ),
        (
            command_pointer(rl_get_next_history),
            EditAction::NextHistory,
        ),
        (
            command_pointer(rl_beginning_of_history),
            EditAction::BeginningOfHistory,
        ),
        (command_pointer(rl_end_of_history), EditAction::EndOfHistory),
        (
            command_pointer(rl_reverse_search_history),
            EditAction::ReverseSearchHistory,
        ),
        (
            command_pointer(rl_forward_search_history),
            EditAction::ForwardSearchHistory,
        ),
        (command_pointer(rl_newline), EditAction::AcceptLine),
        (command_pointer(rl_complete), EditAction::Complete),
        (
            command_pointer(rl_possible_completions),
            EditAction::PossibleCompletions,
        ),
        (
            command_pointer(rl_insert_completions),
            EditAction::InsertCompletions,
        ),
        (command_pointer(rl_undo_command), EditAction::UndoCmd),
        (command_pointer(rl_clear_screen), EditAction::ClearScreen),
        (command_pointer(rl_refresh_line), EditAction::Redraw),
        (command_pointer(rl_abort), EditAction::Abort),
        (command_pointer(rl_tilde_expand), EditAction::Tilde),
        (
            command_pointer(rl_vi_movement_mode),
            EditAction::ViMovementMode,
        ),
        (
            command_pointer(rl_vi_insertion_mode),
            EditAction::ViInsertionMode,
        ),
        (command_pointer(rl_vi_append_mode), EditAction::ViAppendMode),
        (command_pointer(rl_vi_append_eol), EditAction::ViAppendEol),
    ];
    mappings
        .into_iter()
        .find_map(|(candidate, action)| (candidate == address).then_some(action))
}

fn editor_action_for_function(
    keymap: &mut RustKeymap,
    function: rl_command_func_t,
    key: u8,
) -> EditAction {
    edit_action_for_function(function).unwrap_or_else(|| {
        let index = keymap.shell_commands.len() as u32;
        keymap.shell_commands.push(format!(
            "readline-function:{}:{key}",
            command_pointer(function) as usize
        ));
        EditAction::ShellCommand(index)
    })
}

unsafe fn collect_editor_bindings(
    map: Keymap,
    prefix: &mut Vec<u8>,
    path: &mut Vec<usize>,
    output: &mut RustKeymap,
) {
    if map.is_null() || path.len() >= 64 || path.contains(&(map as usize)) {
        return;
    }
    path.push(map as usize);
    for key in 0u16..=255 {
        let entry = &*map.cast::<KEYMAP_ENTRY>().add(key as usize);
        if entry.function.is_null() {
            continue;
        }
        prefix.push(key as u8);
        match entry.r#type {
            0 => {
                let function =
                    std::mem::transmute::<*mut c_void, rl_command_func_t>(entry.function);
                let action = editor_action_for_function(output, function, key as u8);
                output.bind(lineedit_sequence(prefix), action);
            }
            1 => collect_editor_bindings(entry.function, prefix, path, output),
            2 => {
                let index = output.macros.len() as u32;
                output
                    .macros
                    .push(c_text(entry.function.cast()).unwrap_or_default());
                output.bind(lineedit_sequence(prefix), EditAction::Macro(index));
            }
            _ => {}
        }
        prefix.pop();
    }
    path.pop();
}

unsafe fn editor_keymap_from_c(keymap: Keymap) -> RustKeymap {
    let name = readline_store()
        .lock()
        .expect("readline lock")
        .keymap_names
        .iter()
        .find_map(|(name, address)| (*address == keymap as usize).then(|| name.clone()))
        .unwrap_or_else(|| "readline".to_string());
    let mut output = RustKeymap::new(name);
    let emacs = (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_insert = (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_command = (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    if keymap == emacs {
        output.install_emacs_defaults();
    } else if keymap == vi_insert {
        output.install_vi_insert_defaults();
    } else if keymap == vi_command {
        output.install_vi_movement_defaults();
    }
    collect_editor_bindings(keymap, &mut Vec::new(), &mut Vec::new(), &mut output);
    output
}

unsafe fn sync_editor_function_binding(
    keymap: Keymap,
    sequence: &[u8],
    function: Option<rl_command_func_t>,
) {
    let mut store = readline_store().lock().expect("readline lock");
    if store.current_keymap != keymap as usize {
        return;
    }
    let Some(editor) = store.editor.as_mut() else {
        return;
    };
    let sequence_text = lineedit_sequence(sequence);
    let Some(function) = function else {
        editor.keymap.unbind(&sequence_text);
        return;
    };
    let key = sequence.last().copied().unwrap_or_default();
    let action = editor_action_for_function(&mut editor.keymap, function, key);
    editor.keymap.bind(sequence_text, action);
}

unsafe fn sync_editor_macro_binding(keymap: Keymap, sequence: &[u8], text: &str) {
    let mut store = readline_store().lock().expect("readline lock");
    if store.current_keymap != keymap as usize {
        return;
    }
    let Some(editor) = store.editor.as_mut() else {
        return;
    };
    let index = editor.keymap.macros.len() as u32;
    editor.keymap.macros.push(text.to_string());
    editor
        .keymap
        .bind(lineedit_sequence(sequence), EditAction::Macro(index));
}

fn initialize_keymaps() {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    INITIALIZED.get_or_init(|| unsafe {
        for key in 32usize..=126 {
            emacs_standard_keymap[key] = KEYMAP_ENTRY {
                r#type: 0,
                function: command_pointer(rl_insert),
            };
            vi_insertion_keymap[key] = emacs_standard_keymap[key];
        }
        emacs_standard_keymap[1].function = command_pointer(rl_beg_of_line);
        emacs_standard_keymap[2].function = command_pointer(rl_backward_char);
        emacs_standard_keymap[4].function = command_pointer(rl_delete);
        emacs_standard_keymap[5].function = command_pointer(rl_end_of_line);
        emacs_standard_keymap[6].function = command_pointer(rl_forward_char);
        emacs_standard_keymap[8].function = command_pointer(rl_rubout);
        emacs_standard_keymap[9].function = command_pointer(rl_complete);
        emacs_standard_keymap[10].function = command_pointer(rl_newline);
        emacs_standard_keymap[11].function = command_pointer(rl_kill_line);
        emacs_standard_keymap[13].function = command_pointer(rl_newline);
        emacs_standard_keymap[21].function = command_pointer(rl_unix_line_discard);
        emacs_standard_keymap[23].function = command_pointer(rl_unix_word_rubout);
        emacs_standard_keymap[25].function = command_pointer(rl_yank);
        emacs_standard_keymap[24] = KEYMAP_ENTRY {
            r#type: 1,
            function: (&raw mut emacs_ctlx_keymap).cast::<KEYMAP_ENTRY>().cast(),
        };
        emacs_standard_keymap[27] = KEYMAP_ENTRY {
            r#type: 1,
            function: (&raw mut emacs_meta_keymap).cast::<KEYMAP_ENTRY>().cast(),
        };
        emacs_meta_keymap[b'b' as usize].function = command_pointer(rl_backward_word);
        emacs_meta_keymap[b'f' as usize].function = command_pointer(rl_forward_word);
        emacs_meta_keymap[b'd' as usize].function = command_pointer(rl_kill_word);
        vi_insertion_keymap[27].function = command_pointer(rl_vi_movement_mode);
        vi_movement_keymap[b'h' as usize].function = command_pointer(rl_backward_char);
        vi_movement_keymap[b'l' as usize].function = command_pointer(rl_forward_char);
        vi_movement_keymap[b'i' as usize].function = command_pointer(rl_vi_insertion_mode);
        vi_movement_keymap[b'x' as usize].function = command_pointer(rl_delete);

        let emacs = (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let emacs_meta = (&raw mut emacs_meta_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let emacs_ctlx = (&raw mut emacs_ctlx_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let vi_insert = (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let vi_command = (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let mut store = readline_store().lock().expect("readline lock");
        store.current_keymap = emacs as usize;
        for name in ["emacs", "emacs-standard"] {
            store.keymap_names.insert(name.to_string(), emacs as usize);
        }
        store
            .keymap_names
            .insert("emacs-meta".to_string(), emacs_meta as usize);
        store
            .keymap_names
            .insert("emacs-ctlx".to_string(), emacs_ctlx as usize);
        for name in ["vi", "vi-insertion", "vi-insert"] {
            store
                .keymap_names
                .insert(name.to_string(), vi_insert as usize);
        }
        for name in ["vi-move", "vi-command", "vi-movement"] {
            store
                .keymap_names
                .insert(name.to_string(), vi_command as usize);
        }
        rl_executing_keymap = emacs;
        rl_binding_keymap = emacs;
    });
}
