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
