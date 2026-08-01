#[no_mangle]
pub extern "C" fn rl_start_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    store.current_macro.clear();
    store.macro_recording = true;
    0
}

#[no_mangle]
pub extern "C" fn rl_end_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    if !store.macro_recording {
        return 1;
    }
    store.macro_recording = false;
    store.last_macro = std::mem::take(&mut store.current_macro);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_call_last_kbd_macro(count: c_int, _key: c_int) -> c_int {
    let macro_text = readline_store()
        .lock()
        .expect("readline lock")
        .last_macro
        .clone();
    if macro_text.is_empty() {
        rl_ding();
        return 1;
    }
    let executing = malloc_string(&macro_text);
    rl_executing_macro = executing;
    for _ in 0..count.max(1) {
        for key in macro_text.bytes() {
            let entry = keyseq_entry(rl_get_keymap(), &[key], false);
            if let Some(entry) = entry.filter(|entry| (**entry).r#type == 0) {
                let function = (*entry).function;
                if !function.is_null() {
                    let function = std::mem::transmute::<*mut c_void, rl_command_func_t>(function);
                    function(1, key as c_int);
                    continue;
                }
            }
            rl_insert(1, key as c_int);
        }
    }
    libc::free(executing.cast());
    rl_executing_macro = ptr::null_mut();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_print_last_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let macro_text = readline_store()
        .lock()
        .expect("readline lock")
        .last_macro
        .clone();
    let stream = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    let macro_text = clean_c_string(&format!("{macro_text}\n"));
    libc::fputs(macro_text.as_ptr(), stream);
    libc::fflush(stream);
    0
}
