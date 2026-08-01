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
