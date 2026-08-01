#[no_mangle]
pub unsafe extern "C" fn rl_refresh_line(_count: c_int, _key: c_int) -> c_int {
    rl_redisplay();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_overwrite_mode(_count: c_int, _key: c_int) -> c_int {
    rl_insert_mode = (rl_insert_mode == 0) as c_int;
    if rl_insert_mode == 0 {
        rl_readline_state |= RL_STATE_OVERWRITE;
    } else {
        rl_readline_state &= !RL_STATE_OVERWRITE;
    }
    0
}

#[no_mangle]
pub extern "C" fn rl_noop_action(_count: c_int, _key: c_int) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_digit_argument(_count: c_int, key: c_int) -> c_int {
    if let Some(digit) = char::from_u32(key as u32).and_then(|ch| ch.to_digit(10)) {
        rl_numeric_arg = rl_numeric_arg
            .saturating_mul(10)
            .saturating_add(digit as c_int);
        rl_explicit_arg = 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_universal_argument(_count: c_int, _key: c_int) -> c_int {
    rl_numeric_arg = rl_numeric_arg.saturating_mul(4);
    rl_explicit_arg = 1;
    0
}

action_alias! {}

#[no_mangle]
pub unsafe extern "C" fn rl_skip_csi_sequence(_count: c_int, _key: c_int) -> c_int {
    loop {
        let key = rl_read_key();
        if key < 0 {
            return 1;
        }
        if !(0x20..0x40).contains(&key) {
            return 0;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_arrow_keys(count: c_int, _key: c_int) -> c_int {
    let key = rl_read_key();
    match (key as u8).to_ascii_uppercase() {
        b'A' => rl_get_previous_history(count, key),
        b'B' => rl_get_next_history(count, key),
        b'C' => rl_forward_char(count, key),
        b'D' => rl_backward_char(count, key),
        _ => (key < 0) as c_int,
    }
}
