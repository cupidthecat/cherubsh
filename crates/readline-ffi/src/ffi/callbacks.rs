#[no_mangle]
pub unsafe extern "C" fn rl_callback_handler_install(
    prompt: *const c_char,
    callback: Option<rl_vcpfunc_t>,
) {
    rl_initialize();
    rl_set_prompt(prompt);
    rl_readline_state |= RL_STATE_CALLBACK;
    rl_readline_state &= !RL_STATE_EOF;
    rl_eof_found = 0;
    let prompt = c_text(prompt).unwrap_or_default();
    let prep_function = rl_prep_term_function;
    if let Some(prep) = prep_function {
        prep(1);
    }
    let raw_mode = if prep_function.is_none() && libc::isatty(libc::STDIN_FILENO) != 0 {
        RawMode::enter().ok()
    } else {
        None
    };
    let mut store = readline_store().lock().expect("readline lock");
    store.callback = callback;
    store.callback_prompt = prompt.clone();
    store.callback_raw_mode = raw_mode;
    store.callback_prepped = true;
    store.callback_buffer.clear();
    drop(store);
    set_line_buffer("", 0);
    write_callback_prompt(&prompt);
}

unsafe fn write_callback_prompt(prompt: &str) {
    if let Some(redisplay) = rl_redisplay_function {
        redisplay();
    } else {
        write_callback_bytes(prompt.as_bytes());
    }
}

unsafe fn write_callback_bytes(bytes: &[u8]) {
    let descriptor = if rl_outstream.is_null() {
        libc::STDOUT_FILENO
    } else {
        libc::fileno(rl_outstream)
    };
    libc::write(descriptor, bytes.as_ptr().cast(), bytes.len());
}

#[no_mangle]
pub unsafe extern "C" fn rl_callback_read_char() {
    let (callback, prompt) = {
        let store = readline_store().lock().expect("readline lock");
        (store.callback, store.callback_prompt.clone())
    };
    let Some(callback) = callback else { return };
    let key = rl_read_key();
    let line = if key < 0 {
        if key != libc::EOF {
            return;
        }
        rl_eof_found = 1;
        rl_readline_state |= RL_STATE_EOF;
        ptr::null_mut()
    } else {
        let keymap = rl_get_keymap();
        let entry = if keymap.is_null() || key as usize >= 257 {
            None
        } else {
            Some(*keymap.cast::<KEYMAP_ENTRY>().add(key as usize))
        };
        let function = entry
            .filter(|entry| entry.r#type == 0 && !entry.function.is_null())
            .map(|entry| std::mem::transmute::<*mut c_void, rl_command_func_t>(entry.function));
        let previous_dispatching = rl_dispatching;
        rl_dispatching = 1;
        rl_readline_state |= RL_STATE_DISPATCHING;
        if let Some(function) = function {
            function(1, key);
            rl_last_func = Some(function);
        } else if (0x20..=0x7e).contains(&key) {
            rl_insert(1, key);
            rl_last_func = Some(rl_insert);
        }
        rl_readline_state &= !RL_STATE_DISPATCHING;
        rl_dispatching = previous_dispatching;

        let redisplay_function = rl_redisplay_function;
        if key != b'\n' as c_int && key != b'\r' as c_int {
            if let Some(redisplay) = redisplay_function {
                redisplay();
            } else if (0x20..=0x7e).contains(&key) {
                write_callback_bytes(&[key as u8]);
            }
        } else if redisplay_function.is_none() {
            write_callback_bytes(b"\n");
        }
        let current = current_line();
        readline_store()
            .lock()
            .expect("readline lock")
            .callback_buffer = current.as_bytes().to_vec();
        if rl_done == 0 {
            return;
        }
        rl_done = 0;
        malloc_string(&current)
    };
    let was_prepped = {
        let mut store = readline_store().lock().expect("readline lock");
        drop(store.callback_raw_mode.take());
        std::mem::replace(&mut store.callback_prepped, false)
    };
    if was_prepped {
        if let Some(deprep) = rl_deprep_term_function {
            deprep();
        }
    }
    callback(line);
    if !rl_line_buffer.is_null() && *rl_line_buffer != 0 {
        set_line_buffer("", 0);
        readline_store()
            .lock()
            .expect("readline lock")
            .callback_buffer
            .clear();
    }
    if readline_store()
        .lock()
        .expect("readline lock")
        .callback
        .is_some()
    {
        let prep_function = rl_prep_term_function;
        if let Some(prep) = prep_function {
            prep(1);
        }
        let raw_mode = if prep_function.is_none() && libc::isatty(libc::STDIN_FILENO) != 0 {
            RawMode::enter().ok()
        } else {
            None
        };
        {
            let mut store = readline_store().lock().expect("readline lock");
            store.callback_raw_mode = raw_mode;
            store.callback_prepped = true;
        }
        let prompt_value = clean_c_string(&prompt);
        rl_set_prompt(prompt_value.as_ptr());
        write_callback_prompt(&prompt);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_callback_handler_remove() {
    let (raw_mode, was_prepped) = {
        let mut store = readline_store().lock().expect("readline lock");
        store.callback = None;
        store.callback_prompt.clear();
        store.callback_buffer.clear();
        (
            store.callback_raw_mode.take(),
            std::mem::replace(&mut store.callback_prepped, false),
        )
    };
    drop(raw_mode);
    if was_prepped {
        if let Some(deprep) = rl_deprep_term_function {
            deprep();
        }
    }
    rl_readline_state &= !RL_STATE_CALLBACK;
}

#[no_mangle]
pub extern "C" fn rl_callback_sigcleanup() {
    unsafe { rl_free_line_state() };
}
