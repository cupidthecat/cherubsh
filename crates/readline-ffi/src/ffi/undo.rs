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
