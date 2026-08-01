#[no_mangle]
pub unsafe extern "C" fn rl_free_line_state() {
    readline_store().lock().expect("readline lock").undo.clear();
    set_line_buffer("", 0);
    rl_done = 0;
}

#[no_mangle]
pub extern "C" fn rl_free_undo_list() {
    readline_store().lock().expect("readline lock").undo.clear();
}

#[no_mangle]
pub extern "C" fn free_undo_list() {
    rl_free_undo_list();
}

#[no_mangle]
pub unsafe extern "C" fn rl_add_undo(_what: c_int, _start: c_int, _end: c_int, _text: *mut c_char) {
    save_undo();
}

#[no_mangle]
pub unsafe extern "C" fn rl_begin_undo_group() -> c_int {
    save_undo();
    0
}

#[no_mangle]
pub extern "C" fn rl_end_undo_group() -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_modifying(_start: c_int, _end: c_int) -> c_int {
    save_undo();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_redisplay() {
    if let Some(callback) = rl_redisplay_function {
        callback();
    }
}

#[no_mangle]
pub extern "C" fn rl_on_new_line() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn rl_on_new_line_with_prompt() -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_forced_update_display() -> c_int {
    rl_redisplay();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_clear_visible_line() -> c_int {
    libc::write(libc::STDERR_FILENO, b"\r\x1b[2K".as_ptr().cast(), 5);
    0
}

#[no_mangle]
pub extern "C" fn rl_clear_message() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn rl_reset_line_state() -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_crlf() -> c_int {
    libc::write(libc::STDERR_FILENO, b"\n".as_ptr().cast(), 1);
    0
}

#[no_mangle]
pub unsafe extern "C" fn crlf() -> c_int {
    rl_crlf()
}

#[no_mangle]
pub extern "C" fn rl_keep_mark_active() {
    readline_store()
        .lock()
        .expect("readline lock")
        .keep_mark_active = true;
}

#[no_mangle]
pub extern "C" fn rl_activate_mark() {
    let mut store = readline_store().lock().expect("readline lock");
    store.mark_active = true;
    store.keep_mark_active = true;
}

#[no_mangle]
pub extern "C" fn rl_deactivate_mark() {
    readline_store().lock().expect("readline lock").mark_active = false;
}

#[no_mangle]
pub extern "C" fn rl_mark_active_p() -> c_int {
    readline_store().lock().expect("readline lock").mark_active as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_message(message: *const c_char) -> c_int {
    let bytes = if message.is_null() {
        &[][..]
    } else {
        CStr::from_ptr(message).to_bytes()
    };
    libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len());
    0
}

#[no_mangle]
pub extern "C" fn rl_show_char(character: c_int) -> c_int {
    character
}

#[no_mangle]
pub extern "C" fn rl_character_len(character: c_int, _point: c_int) -> c_int {
    char::from_u32(character as u32).map_or(1, |ch| ch.len_utf8() as c_int)
}

#[no_mangle]
pub extern "C" fn rl_redraw_prompt_last_line() {}
#[no_mangle]
pub unsafe extern "C" fn rl_save_prompt() {
    let prompt = c_text(rl_prompt).unwrap_or_default();
    readline_store().lock().expect("readline lock").saved_prompt = Some(prompt);
}
#[no_mangle]
pub unsafe extern "C" fn rl_restore_prompt() {
    let prompt = readline_store()
        .lock()
        .expect("readline lock")
        .saved_prompt
        .take();
    if let Some(prompt) = prompt {
        let prompt = clean_c_string(&prompt);
        rl_set_prompt(prompt.as_ptr());
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_prep_terminal(meta_flag: c_int) {
    if let Some(callback) = rl_prep_term_function {
        callback(meta_flag);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_deprep_terminal() {
    if let Some(callback) = rl_deprep_term_function {
        callback();
    }
}

#[no_mangle]
pub extern "C" fn rl_tty_set_default_bindings(_keymap: Keymap) {}
#[no_mangle]
pub extern "C" fn rl_tty_unset_default_bindings(_keymap: Keymap) {}
#[no_mangle]
pub extern "C" fn rl_tty_set_echoing(value: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    let previous = store.tty_echoing;
    store.tty_echoing = value;
    previous
}
#[no_mangle]
pub unsafe extern "C" fn rl_reset_terminal(name: *const c_char) -> c_int {
    if !name.is_null() {
        rl_terminal_name = name;
    }
    rl_deprep_terminal();
    rl_prep_terminal(1);
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_resize_terminal() {
    if let Some(redisplay) = rl_redisplay_function {
        redisplay();
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_screen_size(rows: c_int, columns: c_int) {
    let size = libc::winsize {
        ws_row: rows.max(0) as u16,
        ws_col: columns.max(0) as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    libc::ioctl(libc::STDOUT_FILENO, libc::TIOCSWINSZ, &size);
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_screen_size(rows: *mut c_int, columns: *mut c_int) {
    let mut size: libc::winsize = std::mem::zeroed();
    libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size);
    if !rows.is_null() {
        *rows = size.ws_row as c_int;
    }
    if !columns.is_null() {
        *columns = size.ws_col as c_int;
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_reset_screen_size() {
    rl_resize_terminal();
}
#[no_mangle]
pub extern "C" fn rl_reparse_colors() {}

#[no_mangle]
pub unsafe extern "C" fn rl_stuff_char(character: c_int) -> c_int {
    rl_pending_input = character;
    1
}

#[no_mangle]
pub unsafe extern "C" fn rl_execute_next(character: c_int) -> c_int {
    rl_pending_input = character;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_clear_pending_input() -> c_int {
    rl_pending_input = 0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_read_key() -> c_int {
    if rl_pending_input != 0 {
        let value = rl_pending_input;
        rl_pending_input = 0;
        return value;
    }
    let (timeout, input) = {
        let store = readline_store().lock().expect("readline lock");
        (store.keyboard_timeout_us, rl_instream)
    };
    let descriptor = if input.is_null() {
        libc::STDIN_FILENO
    } else {
        libc::fileno(input)
    };
    if timeout >= 0 {
        let milliseconds = ((timeout as i64 + 999) / 1000).min(c_int::MAX as i64) as c_int;
        let mut pollfd = libc::pollfd {
            fd: descriptor,
            events: libc::POLLIN,
            revents: 0,
        };
        if libc::poll(&mut pollfd, 1, milliseconds) <= 0 {
            return -1;
        }
    }
    if let Some(getc) = rl_getc_function {
        return getc(if input.is_null() { C_STDIN } else { input });
    }
    if input.is_null() {
        let mut byte = 0u8;
        if libc::read(descriptor, (&raw mut byte).cast(), 1) == 1 {
            byte as c_int
        } else {
            -1
        }
    } else {
        libc::fgetc(input)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_getc(stream: *mut libc::FILE) -> c_int {
    let mut byte = 0u8;
    if libc::read(libc::fileno(stream), (&raw mut byte).cast(), 1) == 1 {
        byte as c_int
    } else {
        libc::EOF
    }
}

#[no_mangle]
pub extern "C" fn rl_set_keyboard_input_timeout(microseconds: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    let previous = store.keyboard_timeout_us;
    if microseconds >= 0 {
        store.keyboard_timeout_us = microseconds;
    }
    previous
}

#[no_mangle]
pub extern "C" fn rl_set_timeout(seconds: u32, microseconds: u32) -> c_int {
    let seconds = u64::from(seconds) + u64::from(microseconds / 1_000_000);
    let microseconds = microseconds % 1_000_000;
    let mut store = readline_store().lock().expect("readline lock");
    store.timeout_duration = (seconds != 0 || microseconds != 0)
        .then(|| Duration::new(seconds, microseconds.saturating_mul(1_000)));
    store.timeout_deadline = None;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_timeout_remaining(seconds: *mut u32, microseconds: *mut u32) -> c_int {
    let deadline = readline_store()
        .lock()
        .expect("readline lock")
        .timeout_deadline;
    let Some(deadline) = deadline else {
        set_errno(0);
        return -1;
    };
    let now = Instant::now();
    if now >= deadline {
        return 0;
    }
    let remaining = deadline.duration_since(now);
    if !seconds.is_null() {
        *seconds = remaining.as_secs().min(u64::from(u32::MAX)) as u32;
    }
    if !microseconds.is_null() {
        *microseconds = remaining.subsec_micros();
    }
    1
}
