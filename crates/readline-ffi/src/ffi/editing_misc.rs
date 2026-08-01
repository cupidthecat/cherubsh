#[no_mangle]
pub unsafe extern "C" fn rl_bracketed_paste_begin(_count: c_int, _key: c_int) -> c_int {
    const END: &[u8] = b"\x1b[201~";
    let mut input = Vec::new();
    loop {
        let key = rl_read_key();
        if key < 0 {
            return 1;
        }
        input.push(key as u8);
        if input.ends_with(END) {
            input.truncate(input.len() - END.len());
            break;
        }
    }
    let input = String::from_utf8_lossy(&input);
    let input = clean_c_string(&input);
    rl_mark = rl_point;
    let expected = input.as_bytes().len();
    let inserted = rl_insert_text(input.as_ptr());
    rl_activate_mark();
    (inserted != expected.min(c_int::MAX as usize) as c_int) as c_int
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
