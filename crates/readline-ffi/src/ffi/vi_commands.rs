
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
