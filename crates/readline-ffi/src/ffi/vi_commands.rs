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

unsafe fn search_line_character(count: c_int, direction: c_int) -> c_int {
    let target = rl_read_key();
    if target < 0 {
        return 1;
    }
    let Some(target) = char::from_u32(target as u32) else {
        return 1;
    };
    let line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    for _ in 0..count.unsigned_abs().max(1) {
        let effective_direction = direction * if count < 0 { -1 } else { 1 };
        let found = if effective_direction < 0 {
            line[..point].rfind(target)
        } else {
            let start = line[point..]
                .chars()
                .next()
                .map_or(point, |character| point + character.len_utf8());
            line[start..].find(target).map(|offset| start + offset)
        };
        let Some(found) = found else {
            rl_ding();
            return 1;
        };
        point = found;
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_char_search(count: c_int, _key: c_int) -> c_int {
    search_line_character(count, 1)
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_char_search(count: c_int, _key: c_int) -> c_int {
    search_line_character(count, -1)
}

unsafe fn set_vi_keymap(name: &str) {
    let name = clean_c_string(name);
    let keymap = rl_get_keymap_by_name(name.as_ptr());
    if !keymap.is_null() {
        rl_set_keymap(keymap);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_insertion_mode(_count: c_int, _key: c_int) -> c_int {
    rl_insert_mode = 1;
    rl_readline_state &= !RL_STATE_OVERWRITE;
    set_vi_keymap("vi-insertion");
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_insert_mode(count: c_int, key: c_int) -> c_int {
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_movement_mode(_count: c_int, key: c_int) -> c_int {
    if rl_point > 0 {
        rl_backward_char(1, key);
    }
    rl_insert_mode = 1;
    rl_readline_state &= !RL_STATE_OVERWRITE;
    set_vi_keymap("vi-movement");
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_insert_beg(count: c_int, key: c_int) -> c_int {
    rl_beg_of_line(count, key);
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_append_mode(count: c_int, key: c_int) -> c_int {
    rl_forward_char(1, key);
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_append_eol(count: c_int, key: c_int) -> c_int {
    rl_end_of_line(1, key);
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_change_case(count: c_int, _key: c_int) -> c_int {
    let mut line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    save_undo();
    for _ in 0..count.max(1) {
        let Some(character) = line[point..].chars().next() else {
            break;
        };
        let replacement = if character.is_lowercase() {
            character.to_uppercase().collect::<String>()
        } else if character.is_uppercase() {
            character.to_lowercase().collect::<String>()
        } else {
            character.to_string()
        };
        let end = point + character.len_utf8();
        line.replace_range(point..end, &replacement);
        point += replacement.len();
    }
    set_line_buffer(&line, point.min(line.len()));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_char_search(count: c_int, key: c_int) -> c_int {
    let direction = if matches!(key as u8, b'F' | b'T') {
        -1
    } else {
        1
    };
    search_line_character(count, direction)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_match(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut start = clamp_boundary(&line, rl_point.max(0) as usize);
    while start < line.len()
        && !matches!(
            line.as_bytes()[start],
            b'(' | b')' | b'[' | b']' | b'{' | b'}'
        )
    {
        start += line[start..].chars().next().unwrap().len_utf8();
    }
    if start >= line.len() {
        rl_ding();
        return 1;
    }
    let current = line.as_bytes()[start];
    let (other, direction) = match current {
        b'(' => (b')', 1),
        b'[' => (b']', 1),
        b'{' => (b'}', 1),
        b')' => (b'(', -1),
        b']' => (b'[', -1),
        b'}' => (b'{', -1),
        _ => unreachable!(),
    };
    let mut depth = 1usize;
    let mut index = start as isize + direction;
    while index >= 0 && (index as usize) < line.len() {
        let byte = line.as_bytes()[index as usize];
        if byte == current {
            depth += 1;
        } else if byte == other {
            depth -= 1;
            if depth == 0 {
                rl_point = index as c_int;
                return 0;
            }
        }
        index += direction;
    }
    rl_ding();
    1
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_change_char(count: c_int, _key: c_int) -> c_int {
    let replacement = rl_read_key();
    if replacement < 0 {
        return 1;
    }
    rl_delete(count.max(1), 0);
    rl_insert(count.max(1), replacement)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_overstrike(_count: c_int, _key: c_int) -> c_int {
    rl_insert_mode = 0;
    rl_readline_state |= RL_STATE_OVERWRITE;
    set_vi_keymap("vi-insertion");
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_replace(count: c_int, key: c_int) -> c_int {
    rl_vi_overstrike(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_goto_mark(_count: c_int, _key: c_int) -> c_int {
    rl_point = rl_mark.clamp(0, rl_end);
    0
}

#[no_mangle]
pub extern "C" fn rl_start_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    store.current_macro.clear();
    store.macro_recording = true;
    0
}

#[no_mangle]
pub extern "C" fn rl_end_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    if !store.macro_recording {
        return 1;
    }
    store.macro_recording = false;
    store.last_macro = std::mem::take(&mut store.current_macro);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_call_last_kbd_macro(count: c_int, _key: c_int) -> c_int {
    let macro_text = readline_store()
        .lock()
        .expect("readline lock")
        .last_macro
        .clone();
    if macro_text.is_empty() {
        rl_ding();
        return 1;
    }
    let executing = malloc_string(&macro_text);
    rl_executing_macro = executing;
    for _ in 0..count.max(1) {
        for key in macro_text.bytes() {
            let entry = keyseq_entry(rl_get_keymap(), &[key], false);
            if let Some(entry) = entry.filter(|entry| (**entry).r#type == 0) {
                let function = (*entry).function;
                if !function.is_null() {
                    let function = std::mem::transmute::<*mut c_void, rl_command_func_t>(function);
                    function(1, key as c_int);
                    continue;
                }
            }
            rl_insert(1, key as c_int);
        }
    }
    libc::free(executing.cast());
    rl_executing_macro = ptr::null_mut();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_print_last_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let macro_text = readline_store()
        .lock()
        .expect("readline lock")
        .last_macro
        .clone();
    let stream = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    let macro_text = clean_c_string(&format!("{macro_text}\n"));
    libc::fputs(macro_text.as_ptr(), stream);
    libc::fflush(stream);
    0
}

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

#[no_mangle]
pub unsafe extern "C" fn rl_re_read_init_file(_count: c_int, _key: c_int) -> c_int {
    let path = readline_store()
        .lock()
        .expect("readline lock")
        .last_init_file
        .clone();
    path.map_or_else(
        || rl_read_init_file(ptr::null()),
        |path| {
            let path = clean_c_string(&path.to_string_lossy());
            rl_read_init_file(path.as_ptr())
        },
    )
}

#[no_mangle]
pub extern "C" fn rl_dump_functions(_count: c_int, _key: c_int) -> c_int {
    rl_function_dumper(1);
    0
}

#[no_mangle]
pub extern "C" fn rl_dump_macros(_count: c_int, _key: c_int) -> c_int {
    rl_macro_dumper(1);
    0
}

#[no_mangle]
pub extern "C" fn rl_dump_variables(_count: c_int, _key: c_int) -> c_int {
    rl_variable_dumper(1);
    0
}
