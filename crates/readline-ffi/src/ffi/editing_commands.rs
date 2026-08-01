#[no_mangle]
pub unsafe extern "C" fn rl_quoted_insert(count: c_int, key: c_int) -> c_int {
    rl_insert(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_tab_insert(count: c_int, _key: c_int) -> c_int {
    rl_insert(count, b'\t' as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_delete_horizontal_space(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let point = rl_point.max(0) as usize;
    let mut start = point.min(line.len());
    let mut end = start;
    while start > 0 && line.as_bytes()[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    while end < line.len() && line.as_bytes()[end].is_ascii_whitespace() {
        end += 1;
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert_comment(_count: c_int, _key: c_int) -> c_int {
    rl_point = 0;
    let marker = clean_c_string("#");
    rl_insert_text(marker.as_ptr());
    rl_done = 1;
    0
}

unsafe fn map_current_word(mode: c_int) -> c_int {
    let mut line = current_line();
    let start = rl_point.max(0) as usize;
    let end = word_right(&line, start);
    if start >= end {
        return 0;
    }
    save_undo();
    let word = &line[start..end];
    let replacement = match mode {
        0 => word.to_uppercase(),
        1 => word.to_lowercase(),
        _ => {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                })
                .unwrap_or_default()
        }
    };
    line.replace_range(start..end, &replacement);
    set_line_buffer(&line, start + replacement.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_upcase_word(_count: c_int, _key: c_int) -> c_int {
    map_current_word(0)
}

#[no_mangle]
pub unsafe extern "C" fn rl_downcase_word(_count: c_int, _key: c_int) -> c_int {
    map_current_word(1)
}

#[no_mangle]
pub unsafe extern "C" fn rl_capitalize_word(_count: c_int, _key: c_int) -> c_int {
    map_current_word(2)
}

#[no_mangle]
pub unsafe extern "C" fn rl_transpose_chars(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let point = clamp_boundary(&line, rl_point.max(0) as usize);
    let mut chars: Vec<char> = line.chars().collect();
    if chars.len() < 2 {
        return 0;
    }
    let char_point = line[..point].chars().count();
    let right = if char_point >= chars.len() {
        chars.len() - 1
    } else {
        char_point.max(1)
    };
    save_undo();
    chars.swap(right - 1, right);
    let updated: String = chars.into_iter().collect();
    set_line_buffer(&updated, updated.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_transpose_words(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let words = history_words(&line);
    if words.len() < 2 {
        return 0;
    }
    let mut updated = words;
    let len = updated.len();
    updated.swap(len - 2, len - 1);
    let updated = updated.join(" ");
    save_undo();
    set_line_buffer(&updated, updated.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_full_line(_count: c_int, _key: c_int) -> c_int {
    rl_kill_text(0, rl_end)
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_region_to_kill(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = rl_mark.min(rl_point).max(0) as usize;
    let end = rl_mark.max(rl_point).max(0) as usize;
    if start < end && end <= line.len() {
        readline_store()
            .lock()
            .expect("readline lock")
            .kill_ring
            .push(line[start..end].to_string());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_forward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = rl_point.max(0) as usize;
    let mut end = start;
    for _ in 0..count.max(0) {
        end = word_right(&line, end);
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .kill_ring
        .push(line[start..end].to_string());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_backward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let end = rl_point.max(0) as usize;
    let mut start = end;
    for _ in 0..count.max(0) {
        start = word_left(&line, start);
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .kill_ring
        .push(line[start..end].to_string());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_beginning_of_history(_count: c_int, _key: c_int) -> c_int {
    history_set_pos(0);
    let entry = current_history();
    if !entry.is_null() {
        let line = c_text((*entry).line).unwrap_or_default();
        set_line_buffer(&line, line.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_end_of_history(_count: c_int, _key: c_int) -> c_int {
    let len = history_store().lock().expect("history lock").entries.len();
    history_set_pos(len as c_int);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_previous_history(count: c_int, _key: c_int) -> c_int {
    let mut entry = ptr::null_mut();
    for _ in 0..count.max(1) {
        let value = previous_history();
        if value.is_null() {
            break;
        }
        entry = value;
    }
    if !entry.is_null() {
        let line = c_text((*entry).line).unwrap_or_default();
        set_line_buffer(&line, line.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_next_history(count: c_int, _key: c_int) -> c_int {
    let mut entry = ptr::null_mut();
    for _ in 0..count.max(1) {
        entry = next_history();
    }
    if !entry.is_null() {
        let line = c_text((*entry).line).unwrap_or_default();
        set_line_buffer(&line, line.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_editing_mode(_count: c_int, _key: c_int) -> c_int {
    let name = clean_c_string("editing-mode");
    let value = clean_c_string("vi");
    rl_variable_bind(name.as_ptr(), value.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn rl_emacs_editing_mode(_count: c_int, _key: c_int) -> c_int {
    let name = clean_c_string("editing-mode");
    let value = clean_c_string("emacs");
    rl_variable_bind(name.as_ptr(), value.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn rl_tilde_expand(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let point = rl_point.max(0) as usize;
    let start = line[..point]
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    let word = line[start..point].to_string();
    if (word == "~" || word.starts_with("~/")) && std::env::var_os("HOME").is_some() {
        let home = std::env::var_os("HOME").unwrap();
        let mut updated = line;
        updated.replace_range(
            start..point,
            &format!("{}{}", home.to_string_lossy(), &word[1..]),
        );
        set_line_buffer(&updated, updated.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_clear_screen(_count: c_int, _key: c_int) -> c_int {
    libc::write(libc::STDERR_FILENO, b"\x1b[2J\x1b[H".as_ptr().cast(), 7);
    0
}

action_alias! {
    rl_clear_display => rl_clear_screen,
    rl_operate_and_get_next => rl_newline,
    rl_fetch_history => rl_get_previous_history,
    rl_export_completions => rl_possible_completions,
    rl_yank_pop => rl_yank,
    rl_insert_close => rl_insert,
    rl_vi_fetch_history => rl_get_previous_history,
    rl_vi_arg_digit => rl_digit_argument,
    rl_vi_put => rl_yank,
    rl_vi_column => rl_beg_of_line,
    rl_vi_yank_pop => rl_yank_pop,
    rl_vi_back_to_indent => rl_beg_of_line,
    rl_vi_first_print => rl_beg_of_line,
    rl_vi_subst => rl_delete,
    rl_vi_overstrike_delete => rl_rubout,
    rl_vi_set_mark => rl_set_mark,
}

#[no_mangle]
pub unsafe extern "C" fn rl_execute_named_command(count: c_int, key: c_int) -> c_int {
    let mut command = Vec::new();
    loop {
        let input = rl_read_key();
        match input {
            value if value < 0 => return 1,
            10 | 13 => break,
            0x08 | 0x7f => {
                command.pop();
            }
            0x1b => return 1,
            value if (0x20..=0x7e).contains(&value) => command.push(value as u8),
            _ => {}
        }
    }
    if command.is_empty() {
        return 1;
    }
    let command = clean_c_string(&String::from_utf8_lossy(&command));
    let Some(function) = rl_named_function(command.as_ptr()) else {
        rl_ding();
        return 1;
    };
    let previous_dispatching = rl_dispatching;
    let was_dispatching = rl_readline_state & RL_STATE_DISPATCHING != 0;
    rl_dispatching = 1;
    rl_readline_state |= RL_STATE_DISPATCHING;
    let status = function(count, key);
    if !was_dispatching {
        rl_readline_state &= !RL_STATE_DISPATCHING;
    }
    rl_dispatching = previous_dispatching;
    rl_last_func = Some(function);
    status
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_yank_arg(count: c_int, key: c_int) -> c_int {
    if rl_explicit_arg != 0 {
        rl_yank_nth_arg(count - 1, key);
    } else {
        rl_yank_nth_arg(b'$' as c_int, key);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_search_again(count: c_int, key: c_int) -> c_int {
    match key as u8 {
        b'n' => {
            rl_noninc_reverse_search_again(count, key);
        }
        b'N' => {
            rl_noninc_forward_search_again(count, key);
        }
        _ => {}
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_search(count: c_int, key: c_int) -> c_int {
    match key as u8 {
        b'?' => {
            rl_noninc_forward_search(count, key);
        }
        b'/' => {
            rl_noninc_reverse_search(count, key);
        }
        _ => {
            rl_ding();
        }
    }
    0
}

unsafe fn vi_operator_range(count: c_int, key: c_int) -> Option<(usize, usize)> {
    let line = current_line();
    let start = clamp_boundary(&line, rl_point.max(0) as usize);
    let uppercase = (key as u8).is_ascii_uppercase();
    let motion = if uppercase {
        b'$' as c_int
    } else {
        rl_read_key()
    };
    if motion < 0 {
        return None;
    }
    let repetitions = count.max(1) as usize;
    let target = match motion as u8 {
        value if value == key as u8 && matches!(value, b'c' | b'd' | b'y') => {
            return Some((0, line.len()));
        }
        b'$' => line.len(),
        b'0' => 0,
        b'^' => line
            .char_indices()
            .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
            .unwrap_or(line.len()),
        b'h' => {
            let mut target = start;
            for _ in 0..repetitions {
                target = line[..target]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index);
            }
            target
        }
        b'l' | b' ' => {
            let mut target = start;
            for _ in 0..repetitions {
                if target < line.len() {
                    target += line[target..].chars().next().map_or(0, char::len_utf8);
                }
            }
            target
        }
        b'w' | b'W' | b'e' | b'E' => {
            let mut target = start;
            for _ in 0..repetitions {
                target = word_right(&line, target);
            }
            target
        }
        b'b' | b'B' => {
            let mut target = start;
            for _ in 0..repetitions {
                target = word_left(&line, target);
            }
            target
        }
        _ => {
            rl_ding();
            return None;
        }
    };
    Some((start.min(target), start.max(target)))
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_delete_to(count: c_int, key: c_int) -> c_int {
    let Some((start, end)) = vi_operator_range(count, key) else {
        return -1;
    };
    rl_kill_text(start as c_int, end as c_int);
    let line = current_line();
    if rl_point as usize == line.len() && rl_point > 0 {
        rl_point = line[..rl_point as usize]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index as c_int);
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator = Some((count, key));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_change_to(count: c_int, key: c_int) -> c_int {
    let Some((start, end)) = vi_operator_range(count, key) else {
        return -1;
    };
    rl_kill_text(start as c_int, end as c_int);
    rl_vi_start_inserting(key, count, 1);
    readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator = Some((count, key));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_yank_to(count: c_int, key: c_int) -> c_int {
    let Some((start, end)) = vi_operator_range(count, key) else {
        return -1;
    };
    let line = current_line();
    if start < end {
        readline_store()
            .lock()
            .expect("readline lock")
            .kill_ring
            .push(line[start..end].to_string());
    }
    rl_point = start.min(line.len()) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_redo(_count: c_int, _key: c_int) -> c_int {
    let command = readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator;
    match command {
        Some((count, key)) if key == b'D' as c_int => rl_vi_delete_to(count, key),
        Some((count, key)) if key == b'C' as c_int => rl_vi_change_to(count, key),
        _ => {
            rl_ding();
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_do_lowercase_version(count: c_int, key: c_int) -> c_int {
    let lower = (key as u8).to_ascii_lowercase();
    if lower == key as u8 {
        rl_ding();
        return 0;
    }
    let map = if rl_executing_keymap.is_null() {
        rl_get_keymap()
    } else {
        rl_executing_keymap
    };
    let Some(entry) = keyseq_entry(map, &[lower], false) else {
        return 1;
    };
    if (*entry).r#type != 0 || (*entry).function.is_null() {
        return 1;
    }
    let function = std::mem::transmute::<*mut c_void, rl_command_func_t>((*entry).function);
    function(count, lower as c_int)
}
