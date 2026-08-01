#[no_mangle]
pub unsafe extern "C" fn rl_restart_output(_count: c_int, _key: c_int) -> c_int {
    let output = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    libc::tcflow(libc::fileno(output), libc::TCOON);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_stop_output(_count: c_int, _key: c_int) -> c_int {
    let input = if rl_instream.is_null() {
        C_STDIN
    } else {
        rl_instream
    };
    libc::tcflow(libc::fileno(input), libc::TCOOFF);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_tty_status(_count: c_int, _key: c_int) -> c_int {
    rl_ding();
    0
}
