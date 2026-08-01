static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn readline_signal_handler(signal: c_int) {
    PENDING_SIGNAL.store(signal, Ordering::SeqCst);
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
