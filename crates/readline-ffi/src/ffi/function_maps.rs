#[no_mangle]
pub unsafe extern "C" fn rl_variable_value(name: *const c_char) -> *mut c_char {
    let name = c_text(name).unwrap_or_default().to_ascii_lowercase();
    readline_store()
        .lock()
        .expect("readline lock")
        .variables
        .get(&name)
        .map_or(ptr::null_mut(), |value| value.as_ptr() as *mut c_char)
}

#[no_mangle]
pub extern "C" fn rl_untranslate_keyseq(key: c_int) -> *mut c_char {
    let value = if key == 0x1b {
        "\\e".to_string()
    } else if (0..32).contains(&key) {
        format!("\\C-{}", ((key as u8) | 0x40) as char)
    } else {
        char::from_u32(key as u32).unwrap_or('\u{fffd}').to_string()
    };
    unsafe { malloc_string(&value) }
}

const BUILTIN_FUNMAP: &[(&str, rl_command_func_t)] = &[
    ("abort", rl_abort),
    ("accept-line", rl_newline),
    ("arrow-key-prefix", rl_arrow_keys),
    ("backward-byte", rl_backward_byte),
    ("backward-char", rl_backward_char),
    ("backward-delete-char", rl_rubout),
    ("backward-kill-line", rl_backward_kill_line),
    ("backward-kill-word", rl_backward_kill_word),
    ("backward-word", rl_backward_word),
    ("beginning-of-history", rl_beginning_of_history),
    ("beginning-of-line", rl_beg_of_line),
    ("bracketed-paste-begin", rl_bracketed_paste_begin),
    ("call-last-kbd-macro", rl_call_last_kbd_macro),
    ("capitalize-word", rl_capitalize_word),
    ("character-search", rl_char_search),
    ("character-search-backward", rl_backward_char_search),
    ("clear-display", rl_clear_display),
    ("clear-screen", rl_clear_screen),
    ("complete", rl_complete),
    ("copy-backward-word", rl_copy_backward_word),
    ("copy-forward-word", rl_copy_forward_word),
    ("copy-region-as-kill", rl_copy_region_to_kill),
    ("delete-char", rl_delete),
    ("delete-char-or-list", rl_delete_or_show_completions),
    ("delete-horizontal-space", rl_delete_horizontal_space),
    ("digit-argument", rl_digit_argument),
    ("do-lowercase-version", rl_do_lowercase_version),
    ("downcase-word", rl_downcase_word),
    ("dump-functions", rl_dump_functions),
    ("dump-macros", rl_dump_macros),
    ("dump-variables", rl_dump_variables),
    ("emacs-editing-mode", rl_emacs_editing_mode),
    ("end-kbd-macro", rl_end_kbd_macro),
    ("end-of-history", rl_end_of_history),
    ("end-of-line", rl_end_of_line),
    ("exchange-point-and-mark", rl_exchange_point_and_mark),
    ("execute-named-command", rl_execute_named_command),
    ("export-completions", rl_export_completions),
    ("fetch-history", rl_fetch_history),
    ("forward-backward-delete-char", rl_rubout_or_delete),
    ("forward-byte", rl_forward_byte),
    ("forward-char", rl_forward_char),
    ("forward-search-history", rl_forward_search_history),
    ("forward-word", rl_forward_word),
    ("history-search-backward", rl_history_search_backward),
    ("history-search-forward", rl_history_search_forward),
    (
        "history-substring-search-backward",
        rl_history_substr_search_backward,
    ),
    (
        "history-substring-search-forward",
        rl_history_substr_search_forward,
    ),
    ("insert-comment", rl_insert_comment),
    ("insert-completions", rl_insert_completions),
    ("kill-whole-line", rl_kill_full_line),
    ("kill-line", rl_kill_line),
    ("kill-region", rl_kill_region),
    ("kill-word", rl_kill_word),
    ("menu-complete", rl_menu_complete),
    ("menu-complete-backward", rl_backward_menu_complete),
    ("next-history", rl_get_next_history),
    ("next-screen-line", rl_next_screen_line),
    (
        "non-incremental-forward-search-history",
        rl_noninc_forward_search,
    ),
    (
        "non-incremental-reverse-search-history",
        rl_noninc_reverse_search,
    ),
    (
        "non-incremental-forward-search-history-again",
        rl_noninc_forward_search_again,
    ),
    (
        "non-incremental-reverse-search-history-again",
        rl_noninc_reverse_search_again,
    ),
    ("old-menu-complete", rl_old_menu_complete),
    ("operate-and-get-next", rl_operate_and_get_next),
    ("overwrite-mode", rl_overwrite_mode),
    ("possible-completions", rl_possible_completions),
    ("previous-history", rl_get_previous_history),
    ("previous-screen-line", rl_previous_screen_line),
    ("print-last-kbd-macro", rl_print_last_kbd_macro),
    ("quoted-insert", rl_quoted_insert),
    ("re-read-init-file", rl_re_read_init_file),
    ("redraw-current-line", rl_refresh_line),
    ("reverse-search-history", rl_reverse_search_history),
    ("revert-line", rl_revert_line),
    ("self-insert", rl_insert),
    ("set-mark", rl_set_mark),
    ("skip-csi-sequence", rl_skip_csi_sequence),
    ("start-kbd-macro", rl_start_kbd_macro),
    ("tab-insert", rl_tab_insert),
    ("tilde-expand", rl_tilde_expand),
    ("transpose-chars", rl_transpose_chars),
    ("transpose-words", rl_transpose_words),
    ("tty-status", rl_tty_status),
    ("undo", rl_undo_command),
    ("universal-argument", rl_universal_argument),
    ("unix-filename-rubout", rl_unix_filename_rubout),
    ("unix-line-discard", rl_unix_line_discard),
    ("unix-word-rubout", rl_unix_word_rubout),
    ("upcase-word", rl_upcase_word),
    ("yank", rl_yank),
    ("yank-last-arg", rl_yank_last_arg),
    ("yank-nth-arg", rl_yank_nth_arg),
    ("yank-pop", rl_yank_pop),
    ("vi-append-eol", rl_vi_append_eol),
    ("vi-append-mode", rl_vi_append_mode),
    ("vi-arg-digit", rl_vi_arg_digit),
    ("vi-back-to-indent", rl_vi_back_to_indent),
    ("vi-backward-bigword", rl_vi_bWord),
    ("vi-backward-word", rl_vi_bword),
    ("vi-bWord", rl_vi_bWord),
    ("vi-bword", rl_vi_bword),
    ("vi-change-case", rl_vi_change_case),
    ("vi-change-char", rl_vi_change_char),
    ("vi-change-to", rl_vi_change_to),
    ("vi-char-search", rl_vi_char_search),
    ("vi-column", rl_vi_column),
    ("vi-complete", rl_vi_complete),
    ("vi-delete", rl_vi_delete),
    ("vi-delete-to", rl_vi_delete_to),
    ("vi-eWord", rl_vi_eWord),
    ("vi-editing-mode", rl_vi_editing_mode),
    ("vi-end-bigword", rl_vi_eWord),
    ("vi-end-word", rl_vi_end_word),
    ("vi-eof-maybe", rl_vi_eof_maybe),
    ("vi-eword", rl_vi_eword),
    ("vi-fWord", rl_vi_fWord),
    ("vi-fetch-history", rl_vi_fetch_history),
    ("vi-first-print", rl_vi_first_print),
    ("vi-forward-bigword", rl_vi_fWord),
    ("vi-forward-word", rl_vi_fword),
    ("vi-fword", rl_vi_fword),
    ("vi-goto-mark", rl_vi_goto_mark),
    ("vi-insert-beg", rl_vi_insert_beg),
    ("vi-insertion-mode", rl_vi_insert_mode),
    ("vi-match", rl_vi_match),
    ("vi-movement-mode", rl_vi_movement_mode),
    ("vi-next-word", rl_vi_next_word),
    ("vi-overstrike", rl_vi_overstrike),
    ("vi-overstrike-delete", rl_vi_overstrike_delete),
    ("vi-prev-word", rl_vi_prev_word),
    ("vi-put", rl_vi_put),
    ("vi-redo", rl_vi_redo),
    ("vi-replace", rl_vi_replace),
    ("vi-rubout", rl_vi_rubout),
    ("vi-search", rl_vi_search),
    ("vi-search-again", rl_vi_search_again),
    ("vi-set-mark", rl_vi_set_mark),
    ("vi-subst", rl_vi_subst),
    ("vi-tilde-expand", rl_vi_tilde_expand),
    ("vi-undo", rl_vi_undo),
    ("vi-unix-word-rubout", rl_vi_unix_word_rubout),
    ("vi-yank-arg", rl_vi_yank_arg),
    ("vi-yank-pop", rl_vi_yank_pop),
    ("vi-yank-to", rl_vi_yank_to),
];

fn named_function(name: &str) -> Option<rl_command_func_t> {
    rl_initialize_funmap();
    let address = readline_store()
        .lock()
        .expect("readline lock")
        .funmap_functions
        .get(&name.to_ascii_lowercase())
        .copied()?;
    Some(unsafe { std::mem::transmute::<usize, rl_command_func_t>(address) })
}

#[no_mangle]
pub unsafe extern "C" fn rl_named_function(name: *const c_char) -> Option<rl_command_func_t> {
    c_text(name).and_then(|name| named_function(&name))
}

#[no_mangle]
pub unsafe extern "C" fn rl_function_of_keyseq(
    sequence: *const c_char,
    keymap: Keymap,
    kind: *mut c_int,
) -> Option<rl_command_func_t> {
    let length = if sequence.is_null() {
        0
    } else {
        CStr::from_ptr(sequence).to_bytes().len()
    };
    rl_function_of_keyseq_len(sequence, length, keymap, kind)
}

#[no_mangle]
pub unsafe extern "C" fn rl_function_of_keyseq_len(
    sequence: *const c_char,
    length: usize,
    keymap: Keymap,
    kind: *mut c_int,
) -> Option<rl_command_func_t> {
    if sequence.is_null() || length == 0 {
        return None;
    }
    let keymap = if keymap.is_null() {
        rl_get_keymap()
    } else {
        keymap
    };
    let sequence = std::slice::from_raw_parts(sequence.cast::<u8>(), length);
    let entry = keyseq_entry(keymap, sequence, false)?;
    if !kind.is_null() {
        *kind = (*entry).r#type as c_int;
    }
    if !(*entry).function.is_null() {
        Some(std::mem::transmute::<*mut c_void, rl_command_func_t>(
            (*entry).function,
        ))
    } else {
        None
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_trim_arg_from_keyseq(
    sequence: *const c_char,
    length: usize,
    keymap: Keymap,
) -> c_int {
    if sequence.is_null() {
        return -1;
    }
    let original_map = if keymap.is_null() {
        rl_get_keymap()
    } else {
        keymap
    };
    if original_map.is_null() {
        return -1;
    }
    let sequence = std::slice::from_raw_parts(sequence.cast::<u8>(), length);
    let mut map = original_map;
    let mut parsed = 0usize;
    let mut parsing_digits = 0u8;
    for (index, key) in sequence.iter().copied().enumerate() {
        if parsing_digits == 2 && key == b'-' {
            parsed = index + 1;
            continue;
        }
        if parsing_digits != 0 {
            if key.is_ascii_digit() {
                parsed = index + 1;
                continue;
            }
            parsing_digits = 0;
        }
        let mut entry = &*map.cast::<KEYMAP_ENTRY>().add(key as usize);
        if entry.r#type == 1 {
            if entry.function.is_null() {
                return parsed.min(c_int::MAX as usize) as c_int;
            }
            map = entry.function;
            if index + 1 < sequence.len() {
                continue;
            }
            entry = &*map.cast::<KEYMAP_ENTRY>().add(256);
        }
        if entry.r#type != 0 {
            return parsed.min(c_int::MAX as usize) as c_int;
        }
        let function = entry.function;
        let digit = command_pointer(rl_digit_argument);
        let universal = command_pointer(rl_universal_argument);
        let vi_digit = command_pointer(rl_vi_arg_digit);
        if function != digit && function != universal && function != vi_digit {
            return parsed.min(c_int::MAX as usize) as c_int;
        }
        if index + 1 == sequence.len() {
            return -1;
        }
        parsing_digits = if function == universal || (function == digit && key == b'-') {
            2
        } else {
            1
        };
        map = original_map;
        parsed = index + 1;
    }
    -1
}

#[no_mangle]
pub unsafe extern "C" fn rl_list_funmap_names() {
    let names = rl_funmap_names();
    if names.is_null() {
        return;
    }
    let mut index = 0usize;
    while !(*names.add(index)).is_null() {
        write_readline_output(&c_text(*names.add(index)).unwrap_or_default());
        index += 1;
    }
    libc::free(names.cast());
}

#[no_mangle]
pub unsafe extern "C" fn rl_invoking_keyseqs_in_map(
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> *mut *mut c_char {
    if keymap.is_null() {
        return ptr::null_mut();
    }
    let wanted = function.map_or(ptr::null_mut(), command_pointer);
    let mut values = Vec::new();
    let mut visited = Vec::new();
    collect_invoking_keyseqs(keymap, wanted, String::new(), &mut visited, &mut values);
    completion_array(&values, false)
}

unsafe fn collect_invoking_keyseqs(
    keymap: Keymap,
    wanted: *mut c_void,
    prefix: String,
    visited: &mut Vec<usize>,
    output: &mut Vec<String>,
) {
    if keymap.is_null() || visited.contains(&(keymap as usize)) {
        return;
    }
    visited.push(keymap as usize);
    for key in 0usize..256 {
        let entry = &*keymap.cast::<KEYMAP_ENTRY>().add(key);
        let mut sequence = prefix.clone();
        sequence.push_str(&key_name(key as u8));
        match entry.r#type {
            0 | 2 if entry.function == wanted => output.push(sequence),
            1 if !entry.function.is_null() => {
                collect_invoking_keyseqs(entry.function, wanted, sequence, visited, output);
            }
            _ => {}
        }
    }
    visited.pop();
}

fn key_name(key: u8) -> String {
    match key {
        0x1b => "\\e".to_string(),
        0x7f => "\\C-?".to_string(),
        0..=0x1f => format!("\\C-{}", ((key | 0x40) as char).to_ascii_lowercase()),
        b'\\' | b'"' => format!("\\{}", key as char),
        0x80..=0xff => format!("\\{:03o}", key),
        _ => (key as char).to_string(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_invoking_keyseqs(
    function: Option<rl_command_func_t>,
) -> *mut *mut c_char {
    rl_invoking_keyseqs_in_map(function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_print_keybinding(name: *const c_char, keymap: Keymap, readable: c_int) {
    let Some(name_text) = c_text(name) else {
        return;
    };
    let function = rl_named_function(name);
    let keymap = if keymap.is_null() {
        rl_get_keymap()
    } else {
        keymap
    };
    let sequences = function.map_or(ptr::null_mut(), |function| {
        rl_invoking_keyseqs_in_map(Some(function), keymap)
    });
    let mut values = Vec::new();
    if !sequences.is_null() {
        let mut index = 0usize;
        while !(*sequences.add(index)).is_null() {
            values.push(c_text(*sequences.add(index)).unwrap_or_default());
            libc::free((*sequences.add(index)).cast());
            index += 1;
        }
        libc::free(sequences.cast());
    }
    if readable != 0 {
        if values.is_empty() {
            write_readline_output(&format!("# {name_text} (not bound)"));
        } else {
            for sequence in values {
                write_readline_output(&format!("\"{sequence}\": {name_text}"));
            }
        }
    } else if values.is_empty() {
        write_readline_output(&format!("{name_text} is not bound to any keys"));
    } else {
        let shown = values
            .iter()
            .take(5)
            .map(|sequence| format!("\"{sequence}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let ending = if values.len() > 5 { ", ..." } else { "." };
        write_readline_output(&format!("{name_text} can be found on {shown}{ending}"));
    }
}

#[no_mangle]
pub extern "C" fn rl_function_dumper(readable: c_int) {
    unsafe {
        let names = rl_funmap_names();
        if names.is_null() {
            return;
        }
        write_readline_output("");
        let mut index = 0usize;
        while !(*names.add(index)).is_null() {
            rl_print_keybinding(*names.add(index), rl_get_keymap(), readable);
            index += 1;
        }
        libc::free(names.cast());
    }
}

#[no_mangle]
pub extern "C" fn rl_macro_dumper(_readable: c_int) {
    let mut macros = Vec::new();
    let mut visited = Vec::new();
    unsafe {
        collect_macros(rl_get_keymap(), String::new(), &mut visited, &mut macros);
    }
    for (sequence, macro_text) in macros {
        unsafe { write_readline_output(&format!("\"{sequence}\": \"{macro_text}\"")) };
    }
}

#[no_mangle]
pub extern "C" fn rl_variable_dumper(_readable: c_int) {
    for (name, value) in &readline_store().lock().expect("readline lock").variables {
        unsafe { write_readline_output(&format!("set {name} {}", value.to_string_lossy())) };
    }
}

unsafe fn write_readline_output(text: &str) {
    let stream = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    let text = clean_c_string(&format!("{text}\n"));
    libc::fputs(text.as_ptr(), stream);
    libc::fflush(stream);
}

unsafe fn collect_macros(
    keymap: Keymap,
    prefix: String,
    visited: &mut Vec<usize>,
    output: &mut Vec<(String, String)>,
) {
    if keymap.is_null() || visited.contains(&(keymap as usize)) {
        return;
    }
    visited.push(keymap as usize);
    for key in 0usize..256 {
        let entry = &*keymap.cast::<KEYMAP_ENTRY>().add(key);
        let mut sequence = prefix.clone();
        sequence.push_str(&key_name(key as u8));
        if entry.r#type == 1 && !entry.function.is_null() {
            collect_macros(entry.function, sequence, visited, output);
        } else if entry.r#type == 2 && !entry.function.is_null() {
            let macro_text = c_text(entry.function.cast()).unwrap_or_default();
            output.push((sequence, macro_text.replace('"', "\\\"")));
        }
    }
    visited.pop();
}

#[no_mangle]
pub extern "C" fn rl_get_keymap_name(keymap: Keymap) -> *mut c_char {
    let store = readline_store().lock().expect("readline lock");
    store
        .keymap_names
        .iter()
        .find_map(|(name, address)| {
            (*address == keymap as usize).then(|| unsafe { malloc_string(name) })
        })
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_keymap_from_edit_mode() {
    let name = if rl_editing_mode == 0 {
        clean_c_string("vi-insertion")
    } else {
        clean_c_string("emacs-standard")
    };
    let keymap = rl_get_keymap_by_name(name.as_ptr());
    rl_set_keymap(keymap);
}

#[no_mangle]
pub extern "C" fn rl_get_keymap_name_from_edit_mode() -> *mut c_char {
    unsafe {
        malloc_string(if rl_editing_mode == 0 {
            "vi-insertion"
        } else {
            "emacs-standard"
        })
    }
}

#[no_mangle]
pub static mut funmap_program_specific_entry_start: c_int = 0;

#[no_mangle]
pub unsafe extern "C" fn rl_add_funmap_entry(
    name: *const c_char,
    function: Option<rl_command_func_t>,
) -> c_int {
    let Some(name_text) = c_text(name) else {
        return -1;
    };
    let name_pointer = malloc_string(&name_text);
    if name_pointer.is_null() {
        return -1;
    }
    let entry = Box::into_raw(Box::new(FUNMAP {
        name: name_pointer,
        function,
    }));
    let mut store = readline_store().lock().expect("readline lock");
    store.funmap_entries.push(entry as usize);
    if let Some(function) = function {
        store
            .funmap_functions
            .entry(name_text.to_ascii_lowercase())
            .or_insert(function as *const () as usize);
    }
    let array = libc::calloc(
        store.funmap_entries.len() + 1,
        std::mem::size_of::<*mut FUNMAP>(),
    ) as *mut *mut FUNMAP;
    if array.is_null() {
        return -1;
    }
    for (index, entry) in store.funmap_entries.iter().copied().enumerate() {
        *array.add(index) = entry as *mut FUNMAP;
    }
    if store.funmap_array != 0 {
        libc::free(store.funmap_array as *mut c_void);
    }
    store.funmap_array = array as usize;
    funmap = array;
    store.funmap_entries.len().min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_funmap_names() -> *mut *const c_char {
    rl_initialize_funmap();
    let mut names: Vec<*const c_char> = readline_store()
        .lock()
        .expect("readline lock")
        .funmap_entries
        .iter()
        .map(|entry| (*(*entry as *mut FUNMAP)).name)
        .collect();
    names.sort_by(|left, right| libc::strcoll(*left, *right).cmp(&0));
    let output =
        libc::calloc(names.len() + 1, std::mem::size_of::<*const c_char>()) as *mut *const c_char;
    if output.is_null() {
        return ptr::null_mut();
    }
    for (index, name) in names.into_iter().enumerate() {
        *output.add(index) = name;
    }
    output
}

#[no_mangle]
pub extern "C" fn rl_initialize_funmap() {
    {
        let mut store = readline_store().lock().expect("readline lock");
        if store.funmap_initialized {
            return;
        }
        store.funmap_initialized = true;
    }
    for &(name, function) in BUILTIN_FUNMAP {
        let name = clean_c_string(name);
        unsafe {
            rl_add_funmap_entry(name.as_ptr(), Some(function));
        }
    }
    unsafe {
        funmap_program_specific_entry_start =
            BUILTIN_FUNMAP.len().min(c_int::MAX as usize) as c_int;
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_push_macro_input(text: *mut c_char) {
    if !text.is_null() {
        rl_executing_macro = text;
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_text(start: c_int, end: c_int) -> *mut c_char {
    let line = current_line();
    let start = clamp_boundary(&line, start.max(0) as usize);
    let end = clamp_boundary(&line, end.max(0) as usize).max(start);
    malloc_string(&line[start..end])
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_termcap(capability: *const c_char) -> *mut c_char {
    match c_text(capability).as_deref() {
        Some("ce") => malloc_string("\x1b[K"),
        Some("cl") => malloc_string("\x1b[2J\x1b[H"),
        Some("cr") => malloc_string("\r"),
        Some("le") => malloc_string("\x1b[D"),
        Some("nd") => malloc_string("\x1b[C"),
        Some("up") => malloc_string("\x1b[A"),
        _ => ptr::null_mut(),
    }
}
