
unsafe fn yank_history_argument(argument: c_int, skip: usize) -> c_int {
    let line = {
        let store = history_store().lock().expect("history lock");
        let Some(index) = store.entries.len().checked_sub(skip + 1) else {
            rl_ding();
            return 1;
        };
        c_text((*(store.entries[index] as *mut HIST_ENTRY)).line).unwrap_or_default()
    };
    let line = clean_c_string(&line);
    let argument = history_arg_extract(argument, argument, line.as_ptr());
    if argument.is_null() || (*argument == 0) {
        libc::free(argument.cast());
        rl_ding();
        return 1;
    }
    rl_mark = rl_point;
    let result = rl_insert_text(argument);
    libc::free(argument.cast());
    (result < 0) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_yank_nth_arg(count: c_int, _key: c_int) -> c_int {
    yank_history_argument(count, 0)
}

#[no_mangle]
pub unsafe extern "C" fn rl_yank_last_arg(count: c_int, _key: c_int) -> c_int {
    let argument = if rl_explicit_arg != 0 {
        count
    } else {
        b'$' as c_int
    };
    yank_history_argument(argument, 0)
}

unsafe fn readline_history_search(direction: c_int, prefix: bool, again: bool) -> c_int {
    let needle = if again {
        readline_store()
            .lock()
            .expect("readline lock")
            .last_search
            .as_ref()
            .map(|(needle, _)| needle.clone())
            .unwrap_or_default()
    } else {
        let line = current_line();
        let point = clamp_boundary(&line, rl_point.max(0) as usize);
        line[..point].to_string()
    };
    if needle.is_empty() && !again {
        return 1;
    }
    let found = search_history(&needle, direction, prefix, None);
    if found < 0 {
        rl_ding();
        return 1;
    }
    readline_store().lock().expect("readline lock").last_search = Some((needle, prefix));
    let entry = current_history();
    if entry.is_null() {
        return 1;
    }
    let line = c_text((*entry).line).unwrap_or_default();
    set_line_buffer(&line, line.len());
    0
}

macro_rules! history_search_action {
    ($name:ident, $direction:expr, $prefix:expr, $again:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(_count: c_int, _key: c_int) -> c_int {
            readline_history_search($direction, $prefix, $again)
        }
    };
}

history_search_action!(rl_reverse_search_history, -1, false, false);
history_search_action!(rl_forward_search_history, 1, false, false);
history_search_action!(rl_history_search_backward, -1, true, false);
history_search_action!(rl_history_search_forward, 1, true, false);
history_search_action!(rl_history_substr_search_backward, -1, false, false);
history_search_action!(rl_history_substr_search_forward, 1, false, false);
history_search_action!(rl_noninc_reverse_search, -1, false, false);
history_search_action!(rl_noninc_forward_search, 1, false, false);
history_search_action!(rl_noninc_reverse_search_again, -1, false, true);
history_search_action!(rl_noninc_forward_search_again, 1, false, true);
