#[no_mangle]
pub extern "C" fn rl_set_paren_blink_timeout(microseconds: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    let previous = store.paren_blink_timeout_us;
    if microseconds >= 0 {
        store.paren_blink_timeout_us = microseconds;
    }
    previous
}

#[no_mangle]
pub extern "C" fn rl_clear_history() {
    clear_history();
}

#[no_mangle]
pub unsafe extern "C" fn rl_maybe_save_line() -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    if store.saved_line.is_none() {
        store.saved_line = Some((
            current_line(),
            rl_point.max(0) as usize,
            rl_mark.max(0) as usize,
        ));
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_maybe_unsave_line() -> c_int {
    let saved = readline_store()
        .lock()
        .expect("readline lock")
        .saved_line
        .take();
    if let Some((line, point, mark)) = saved {
        set_line_buffer(&line, point);
        rl_mark = mark.min(line.len()).min(c_int::MAX as usize) as c_int;
    } else {
        rl_ding();
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_maybe_replace_line() -> c_int {
    let position = where_history();
    let entry = current_history();
    if !entry.is_null() {
        let line = current_line();
        if c_text((*entry).line).as_deref() != Some(line.as_str()) {
            let old =
                replace_history_entry(position, clean_c_string(&line).as_ptr(), ptr::null_mut());
            if !old.is_null() {
                free_entry(old);
            }
        }
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn maybe_save_line() -> c_int {
    rl_maybe_save_line()
}
#[no_mangle]
pub unsafe extern "C" fn maybe_unsave_line() -> c_int {
    rl_maybe_unsave_line()
}
#[no_mangle]
pub unsafe extern "C" fn maybe_replace_line() -> c_int {
    rl_maybe_replace_line()
}

#[no_mangle]
pub extern "C" fn rl_completion_mode(_function: Option<rl_command_func_t>) -> c_int {
    unsafe { rl_completion_type }
}

#[no_mangle]
pub unsafe extern "C" fn rl_save_state(state: *mut c_void) -> c_int {
    let state = state.cast::<READLINE_STATE>();
    if state.is_null() {
        return -1;
    }
    let line_capacity = {
        readline_store()
            .lock()
            .expect("readline lock")
            .line_capacity
    };
    *state = READLINE_STATE {
        point: rl_point,
        end: rl_end,
        mark: rl_mark,
        buflen: line_capacity.min(c_int::MAX as usize) as c_int,
        buffer: rl_line_buffer,
        undo: rl_undo_list,
        prompt: rl_prompt,
        state: rl_readline_state as c_int,
        done: rl_done,
        keymap: rl_get_keymap(),
        last_function: rl_last_func,
        insert_mode: rl_insert_mode,
        editing_mode: rl_editing_mode,
        key_sequence: rl_executing_keyseq,
        key_sequence_length: rl_key_sequence_length,
        pending_input: rl_pending_input,
        input: rl_instream,
        output: rl_outstream,
        macro_text: rl_executing_macro,
        catch_signals: rl_catch_signals,
        catch_sigwinch: rl_catch_sigwinch,
        completion_entry: rl_completion_entry_function,
        menu_completion_entry: rl_menu_completion_entry_function,
        ignore_completions: rl_ignore_some_completions_function,
        attempted_completion: rl_attempted_completion_function,
        word_break_characters: rl_completer_word_break_characters,
        reserved: [0; 64],
    };
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_restore_state(state: *mut c_void) -> c_int {
    let state = state.cast::<READLINE_STATE>();
    if state.is_null() {
        return -1;
    }
    let state = &*state;
    rl_point = state.point;
    rl_end = state.end;
    rl_mark = state.mark;
    rl_line_buffer = state.buffer;
    rl_undo_list = state.undo;
    rl_prompt = state.prompt;
    rl_display_prompt = state.prompt;
    rl_readline_state = state.state as c_ulong;
    rl_done = state.done;
    rl_set_keymap(state.keymap);
    rl_last_func = state.last_function;
    rl_insert_mode = state.insert_mode;
    rl_editing_mode = state.editing_mode;
    rl_executing_keyseq = state.key_sequence;
    rl_key_sequence_length = state.key_sequence_length;
    rl_pending_input = state.pending_input;
    rl_instream = state.input;
    rl_outstream = state.output;
    rl_executing_macro = state.macro_text;
    rl_catch_signals = state.catch_signals;
    rl_catch_sigwinch = state.catch_sigwinch;
    rl_completion_entry_function = state.completion_entry;
    rl_menu_completion_entry_function = state.menu_completion_entry;
    rl_ignore_some_completions_function = state.ignore_completions;
    rl_attempted_completion_function = state.attempted_completion;
    rl_completer_word_break_characters = state.word_break_characters;
    let mut store = readline_store().lock().expect("readline lock");
    store.line_allocation = state.buffer as usize;
    store.line_capacity = state.buflen.max(0) as usize;
    store.prompt_allocation = state.prompt as usize;
    store.mark_active = false;
    0
}
