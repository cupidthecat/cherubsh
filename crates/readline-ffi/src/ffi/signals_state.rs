#[no_mangle]
pub unsafe extern "C" fn rl_extend_line_buffer(length: c_int) {
    if length < 0 {
        return;
    }
    let wanted = (length as usize).saturating_add(1);
    let mut store = readline_store().lock().expect("readline lock");
    if store.line_capacity >= wanted {
        return;
    }
    let mut capacity = store.line_capacity.max(256);
    while capacity < wanted {
        capacity = capacity.saturating_mul(2);
    }
    let allocation = libc::realloc(store.line_allocation as *mut c_void, capacity).cast::<c_char>();
    if allocation.is_null() {
        return;
    }
    store.line_allocation = allocation as usize;
    store.line_capacity = capacity;
    rl_line_buffer = allocation;
}

#[no_mangle]
pub unsafe extern "C" fn ding() -> c_int {
    rl_ding()
}

#[no_mangle]
pub extern "C" fn alphabetic(character: c_int) -> c_int {
    rl_alphabetic(character)
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_signals() -> c_int {
    let signals = [
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGQUIT,
        libc::SIGALRM,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGWINCH,
    ];
    let mut store = readline_store().lock().expect("readline lock");
    for signal in signals {
        if store.signal_handlers.contains_key(&signal) {
            continue;
        }
        let previous = libc::signal(
            signal,
            readline_signal_handler as *const () as libc::sighandler_t,
        );
        if previous == libc::SIG_ERR {
            return -1;
        }
        store.signal_handlers.insert(signal, previous);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_clear_signals() -> c_int {
    let handlers = std::mem::take(
        &mut readline_store()
            .lock()
            .expect("readline lock")
            .signal_handlers,
    );
    for (signal, handler) in handlers {
        libc::signal(signal, handler as libc::sighandler_t);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_cleanup_after_signal() {
    rl_deprep_terminal();
    rl_clear_signals();
}
#[no_mangle]
pub unsafe extern "C" fn rl_reset_after_signal() {
    rl_prep_terminal(1);
    if rl_catch_signals != 0 {
        rl_set_signals();
    }
}
#[no_mangle]
pub extern "C" fn rl_pending_signal() -> c_int {
    PENDING_SIGNAL.load(Ordering::SeqCst)
}
#[no_mangle]
pub unsafe extern "C" fn rl_check_signals() {
    let signal = PENDING_SIGNAL.swap(0, Ordering::SeqCst);
    if signal == 0 {
        return;
    }
    if let Some(hook) = rl_signal_event_hook {
        hook();
    }
    if signal == libc::SIGWINCH && rl_catch_sigwinch != 0 {
        rl_resize_terminal();
    }
}
#[no_mangle]
pub unsafe extern "C" fn rl_echo_signal_char(signal: c_int) {
    let character = match signal {
        libc::SIGINT => Some('C'),
        libc::SIGQUIT => Some('\\'),
        libc::SIGTSTP => Some('Z'),
        _ => None,
    };
    if let Some(character) = character {
        let stream = if rl_outstream.is_null() {
            C_STDOUT
        } else {
            rl_outstream
        };
        let text = clean_c_string(&format!("^{character}"));
        libc::fputs(text.as_ptr(), stream);
        libc::fflush(stream);
    }
}
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

#[no_mangle]
pub unsafe extern "C" fn rl_vi_check() -> c_int {
    if rl_point > 0 && rl_point == rl_end {
        rl_backward_char(1, 0);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_vi_domove(key: c_int, next: *mut c_int) -> c_int {
    let motion = rl_read_key();
    if !next.is_null() {
        *next = motion;
    }
    if motion < 0 {
        return -1;
    }
    if motion == key && matches!(key as u8, b'c' | b'd' | b'y') {
        rl_mark = rl_end;
        rl_point = 0;
        return 0;
    }
    let start = rl_point;
    let status = match motion as u8 {
        b'0' | b'^' => rl_beg_of_line(1, motion),
        b'$' => rl_end_of_line(1, motion),
        b'h' => rl_backward_char(rl_numeric_arg.max(1), motion),
        b'l' | b' ' => rl_forward_char(rl_numeric_arg.max(1), motion),
        b'b' | b'B' => rl_backward_word(rl_numeric_arg.max(1), motion),
        b'w' | b'W' | b'e' | b'E' => rl_forward_word(rl_numeric_arg.max(1), motion),
        _ => return 1,
    };
    rl_mark = start;
    status
}
#[no_mangle]
pub extern "C" fn rl_vi_bracktype(key: c_int) -> c_int {
    match key as u8 {
        b'(' => 1,
        b')' => -1,
        b'[' => 2,
        b']' => -2,
        b'{' => 3,
        b'}' => -3,
        _ => 0,
    }
}
#[no_mangle]
pub unsafe extern "C" fn rl_vi_start_inserting(key: c_int, count: c_int, _sign: c_int) {
    rl_begin_undo_group();
    readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator = Some((count.max(1), key));
    rl_vi_insertion_mode(1, key);
}

#[no_mangle]
pub unsafe extern "C" fn rl_paste_from_clipboard(count: c_int, key: c_int) -> c_int {
    rl_noop_action(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_discard_argument() -> c_int {
    rl_numeric_arg = 1;
    rl_explicit_arg = 0;
    0
}
