#[no_mangle]
pub static mut emacs_standard_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut emacs_meta_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut emacs_ctlx_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut vi_insertion_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut vi_movement_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];

#[no_mangle]
pub unsafe extern "C" fn rl_make_bare_keymap() -> Keymap {
    libc::calloc(257, std::mem::size_of::<KEYMAP_ENTRY>())
}

#[no_mangle]
pub unsafe extern "C" fn rl_empty_keymap(keymap: Keymap) -> c_int {
    if keymap.is_null() {
        return 0;
    }
    (0..256)
        .all(|index| {
            let entry = &*keymap.cast::<KEYMAP_ENTRY>().add(index);
            entry.r#type == 0 && entry.function.is_null()
        })
        .into()
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_keymap(keymap: Keymap) -> Keymap {
    if keymap.is_null() {
        return ptr::null_mut();
    }
    let copy = rl_make_bare_keymap();
    if !copy.is_null() {
        ptr::copy_nonoverlapping(
            keymap.cast::<KEYMAP_ENTRY>(),
            copy.cast::<KEYMAP_ENTRY>(),
            257,
        );
    }
    copy
}

#[no_mangle]
pub unsafe extern "C" fn rl_make_keymap() -> Keymap {
    let keymap = rl_make_bare_keymap();
    if !keymap.is_null() {
        for key in 32usize..=126 {
            (*keymap.cast::<KEYMAP_ENTRY>().add(key)).function = command_pointer(rl_insert);
        }
        for key in 128usize..=255 {
            (*keymap.cast::<KEYMAP_ENTRY>().add(key)).function = command_pointer(rl_insert);
        }
        (*keymap.cast::<KEYMAP_ENTRY>().add(b'\t' as usize)).function = command_pointer(rl_insert);
        (*keymap.cast::<KEYMAP_ENTRY>().add(0x7f)).function = command_pointer(rl_rubout);
        (*keymap.cast::<KEYMAP_ENTRY>().add(0x08)).function = command_pointer(rl_rubout);
    }
    keymap
}

fn static_keymap(keymap: Keymap) -> bool {
    let addresses = [
        (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut emacs_meta_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut emacs_ctlx_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
    ];
    addresses.contains(&keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_discard_keymap(keymap: Keymap) {
    if keymap.is_null() || static_keymap(keymap) {
        return;
    }
    for index in 0..257 {
        let entry = &mut *keymap.cast::<KEYMAP_ENTRY>().add(index);
        match entry.r#type {
            1 if !entry.function.is_null() => {
                let child = entry.function as Keymap;
                rl_discard_keymap(child);
                libc::free(child);
            }
            2 if !entry.function.is_null() => libc::free(entry.function),
            _ => {}
        }
        entry.r#type = 0;
        entry.function = ptr::null_mut();
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_free_keymap(keymap: Keymap) {
    if !keymap.is_null() && !static_keymap(keymap) {
        rl_discard_keymap(keymap);
        libc::free(keymap);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_keymap_by_name(name: *const c_char) -> Keymap {
    initialize_keymaps();
    let name = c_text(name).unwrap_or_default().to_ascii_lowercase();
    readline_store()
        .lock()
        .expect("readline lock")
        .keymap_names
        .get(&name)
        .copied()
        .unwrap_or(0) as Keymap
}

#[no_mangle]
pub extern "C" fn rl_get_keymap() -> Keymap {
    initialize_keymaps();
    readline_store()
        .lock()
        .expect("readline lock")
        .current_keymap as Keymap
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_keymap(keymap: Keymap) {
    if keymap.is_null() {
        return;
    }
    initialize_keymaps();
    let editor_keymap = editor_keymap_from_c(keymap);
    let emacs = (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_insert = (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_command = (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let mut store = readline_store().lock().expect("readline lock");
    store.current_keymap = keymap as usize;
    if let Some(editor) = store.editor.as_mut() {
        if keymap == vi_command {
            editor.set_vi_command_keymap(editor_keymap);
            editor.vi_mode = true;
        } else {
            editor.keymap = editor_keymap;
            editor.vi_mode = false;
            editor.set_self_insert_unbound(keymap == emacs || keymap == vi_insert);
        }
    }
    drop(store);
    rl_executing_keymap = keymap;
    rl_binding_keymap = keymap;
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_keymap_name(name: *const c_char, keymap: Keymap) -> c_int {
    let Some(name) = c_text(name) else { return -1 };
    if keymap.is_null() {
        return -1;
    }
    initialize_keymaps();
    let name = name.to_ascii_lowercase();
    const BUILTIN_NAMES: &[&str] = &[
        "emacs",
        "emacs-standard",
        "emacs-meta",
        "emacs-ctlx",
        "vi",
        "vi-move",
        "vi-command",
        "vi-insert",
        "vi-insertion",
    ];
    if static_keymap(keymap) || BUILTIN_NAMES.contains(&name.as_str()) {
        return -1;
    }
    let mut store = readline_store().lock().expect("readline lock");
    if let Some(old_name) = store
        .keymap_names
        .iter()
        .find_map(|(old_name, address)| (*address == keymap as usize).then(|| old_name.clone()))
    {
        store.keymap_names.remove(&old_name);
    }
    store.keymap_names.insert(name, keymap as usize);
    0
}

unsafe fn bind_in_map(key: c_int, function: Option<rl_command_func_t>, keymap: Keymap) -> c_int {
    if !(0..257).contains(&key) || keymap.is_null() {
        return -1;
    }
    let entry = keymap.cast::<KEYMAP_ENTRY>().add(key as usize);
    (*entry).r#type = 0;
    (*entry).function = function.map_or(ptr::null_mut(), command_pointer);
    sync_editor_function_binding(keymap, &[key as u8], function);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key(key: c_int, function: Option<rl_command_func_t>) -> c_int {
    bind_in_map(key, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key_in_map(
    key: c_int,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    bind_in_map(key, function, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_key(key: c_int) -> c_int {
    bind_in_map(key, None, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_key_in_map(key: c_int, keymap: Keymap) -> c_int {
    bind_in_map(key, None, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key_if_unbound(
    key: c_int,
    function: Option<rl_command_func_t>,
) -> c_int {
    rl_bind_key_if_unbound_in_map(key, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key_if_unbound_in_map(
    key: c_int,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    if !(0..257).contains(&key) || keymap.is_null() {
        return -1;
    }
    if (*keymap.cast::<KEYMAP_ENTRY>().add(key as usize))
        .function
        .is_null()
    {
        bind_in_map(key, function, keymap)
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_function_in_map(
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    if keymap.is_null() {
        return -1;
    }
    let wanted = function.map_or(ptr::null_mut(), command_pointer);
    let mut removed = false;
    for index in 0..257 {
        let entry = keymap.cast::<KEYMAP_ENTRY>().add(index);
        if (*entry).r#type == 0 && (*entry).function == wanted {
            (*entry).function = ptr::null_mut();
            removed = true;
        } else if (*entry).r#type == 1
            && !(*entry).function.is_null()
            && rl_unbind_function_in_map(function, (*entry).function) == 1
        {
            removed = true;
        }
    }
    removed as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_command_in_map(command: *const c_char, keymap: Keymap) -> c_int {
    let function = rl_named_function(command);
    function.map_or(0, |function| {
        rl_unbind_function_in_map(Some(function), keymap)
    })
}

fn translate_key_sequence(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut output = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 >= bytes.len() {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        match bytes[index + 1] {
            b'e' | b'E' => {
                output.push(0x1b);
                index += 2;
            }
            b'C' | b'c' if bytes.get(index + 2) == Some(&b'-') && index + 3 < bytes.len() => {
                output.push(bytes[index + 3].to_ascii_uppercase() & 0x1f);
                index += 4;
            }
            b'M' | b'm' if bytes.get(index + 2) == Some(&b'-') && index + 3 < bytes.len() => {
                output.push(0x1b);
                output.push(bytes[index + 3]);
                index += 4;
            }
            b'n' => {
                output.push(b'\n');
                index += 2;
            }
            b'r' => {
                output.push(b'\r');
                index += 2;
            }
            b't' => {
                output.push(b'\t');
                index += 2;
            }
            value => {
                output.push(value);
                index += 2;
            }
        }
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn rl_translate_keyseq(
    input: *const c_char,
    output: *mut c_char,
    length: *mut c_int,
) -> c_int {
    if output.is_null() || length.is_null() {
        return 1;
    }
    let translated = translate_key_sequence(&c_text(input).unwrap_or_default());
    ptr::copy_nonoverlapping(translated.as_ptr(), output.cast(), translated.len());
    *length = translated.len().min(c_int::MAX as usize) as c_int;
    0
}

unsafe fn bind_keyseq_in_map(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
    let Some(entry) = keyseq_entry(keymap, &translated, true) else {
        return -1;
    };
    if (*entry).r#type == 2 && !(*entry).function.is_null() {
        libc::free((*entry).function);
    }
    (*entry).r#type = 0;
    (*entry).function = function.map_or(ptr::null_mut(), command_pointer);
    sync_editor_function_binding(keymap, &translated, function);
    0
}

unsafe fn keyseq_entry(
    mut keymap: Keymap,
    sequence: &[u8],
    create: bool,
) -> Option<*mut KEYMAP_ENTRY> {
    let (&last, prefix) = sequence.split_last()?;
    if keymap.is_null() {
        return None;
    }
    for key in prefix {
        let entry = keymap.cast::<KEYMAP_ENTRY>().add(*key as usize);
        if (*entry).r#type == 1 && !(*entry).function.is_null() {
            keymap = (*entry).function;
            continue;
        }
        if !create {
            return None;
        }
        if (*entry).r#type == 2 && !(*entry).function.is_null() {
            libc::free((*entry).function);
        }
        let child = rl_make_bare_keymap();
        if child.is_null() {
            return None;
        }
        (*entry).r#type = 1;
        (*entry).function = child;
        keymap = child;
    }
    Some(keymap.cast::<KEYMAP_ENTRY>().add(last as usize))
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
) -> c_int {
    bind_keyseq_in_map(sequence, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq_in_map(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    bind_keyseq_in_map(sequence, function, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq_if_unbound(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
) -> c_int {
    rl_bind_keyseq_if_unbound_in_map(sequence, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq_if_unbound_in_map(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
    let bound =
        keyseq_entry(keymap, &translated, false).is_some_and(|entry| !(*entry).function.is_null());
    if bound {
        1
    } else {
        bind_keyseq_in_map(sequence, function, keymap)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_key(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    rl_bind_keyseq_in_map(sequence, function, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_macro_bind(
    sequence: *const c_char,
    macro_text: *const c_char,
    keymap: Keymap,
) -> c_int {
    let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
    let Some(entry) = keyseq_entry(keymap, &translated, true) else {
        return -1;
    };
    if (*entry).r#type == 2 && !(*entry).function.is_null() {
        libc::free((*entry).function);
    }
    let macro_text = translate_key_sequence(&c_text(macro_text).unwrap_or_default());
    (*entry).r#type = 2;
    (*entry).function = malloc_string(&String::from_utf8_lossy(&macro_text)).cast();
    sync_editor_macro_binding(keymap, &translated, &String::from_utf8_lossy(&macro_text));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_generic_bind(
    kind: c_int,
    sequence: *const c_char,
    data: *mut c_char,
    keymap: Keymap,
) -> c_int {
    match kind {
        0 => {
            let function = (!data.is_null())
                .then(|| std::mem::transmute::<*mut c_char, rl_command_func_t>(data));
            rl_bind_keyseq_in_map(sequence, function, keymap)
        }
        1 => {
            let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
            let Some(entry) = keyseq_entry(keymap, &translated, true) else {
                return -1;
            };
            if (*entry).r#type == 2 && !(*entry).function.is_null() {
                libc::free((*entry).function);
            }
            (*entry).r#type = 1;
            (*entry).function = data.cast();
            0
        }
        2 => rl_macro_bind(sequence, data, keymap),
        _ => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_add_defun(
    name: *const c_char,
    function: Option<rl_command_func_t>,
    key: c_int,
) -> c_int {
    if name.is_null() || function.is_none() {
        return -1;
    }
    rl_add_funmap_entry(name, function);
    if key >= 0 {
        rl_bind_key(key, function)
    } else {
        0
    }
}

macro_rules! action_alias {
    ($($name:ident => $target:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub unsafe extern "C" fn $name(count: c_int, key: c_int) -> c_int {
                $target(count, key)
            }
        )*
    };
}

action_alias! {
    rl_forward_byte => rl_forward_char,
    rl_forward => rl_forward_char,
    rl_backward_byte => rl_backward_char,
    rl_backward => rl_backward_char,
    rl_previous_screen_line => rl_backward_char,
    rl_next_screen_line => rl_forward_char,
    rl_rubout_or_delete => rl_rubout,
    rl_delete_or_show_completions => rl_delete,
    rl_unix_word_rubout => rl_backward_kill_word,
    rl_unix_filename_rubout => rl_backward_kill_word,
    rl_unix_line_discard => rl_backward_kill_line,
    rl_revert_line => rl_undo_command,
    rl_old_menu_complete => rl_complete,
    rl_menu_complete => rl_complete,
    rl_backward_menu_complete => rl_complete,
    rl_vi_complete => rl_complete,
    rl_vi_tilde_expand => rl_tilde_expand,
    rl_vi_prev_word => rl_backward_word,
    rl_vi_next_word => rl_forward_word,
    rl_vi_end_word => rl_forward_word,
    rl_vi_fWord => rl_forward_word,
    rl_vi_bWord => rl_backward_word,
    rl_vi_eWord => rl_forward_word,
    rl_vi_fword => rl_forward_word,
    rl_vi_bword => rl_backward_word,
    rl_vi_eword => rl_forward_word,
    rl_vi_rubout => rl_rubout,
    rl_vi_delete => rl_delete,
    rl_vi_unix_word_rubout => rl_backward_kill_word,
    rl_vi_undo => rl_undo_command,
    rl_vi_eof_maybe => rl_delete,
}
