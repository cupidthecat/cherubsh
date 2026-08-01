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
