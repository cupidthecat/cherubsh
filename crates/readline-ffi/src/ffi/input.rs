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
