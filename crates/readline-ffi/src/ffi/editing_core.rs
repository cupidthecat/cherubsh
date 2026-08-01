#[no_mangle]
pub static mut rl_undo_list: *mut UNDO_LIST = ptr::null_mut();

#[no_mangle]
pub unsafe extern "C" fn rl_initialize() -> c_int {
    initialize_keymaps();
    rl_initialize_funmap();
    let _ = readline_store();
    let _ = history_store();
    if rl_line_buffer.is_null() {
        set_line_buffer("", 0);
    }
    if rl_instream.is_null() {
        rl_instream = C_STDIN;
    }
    if rl_outstream.is_null() {
        rl_outstream = C_STDOUT;
    }
    rl_readline_state |= RL_STATE_INITIALIZED;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_prompt(prompt: *const c_char) -> c_int {
    let text = c_text(prompt).unwrap_or_default();
    let allocation = malloc_string(&text);
    if allocation.is_null() {
        return -1;
    }
    let mut store = readline_store().lock().expect("readline lock");
    if store.prompt_allocation != 0 {
        libc::free(store.prompt_allocation as *mut c_void);
    }
    store.prompt_allocation = allocation as usize;
    rl_prompt = allocation;
    rl_display_prompt = allocation;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_expand_prompt(prompt: *mut c_char) -> c_int {
    let text = c_text(prompt).unwrap_or_default();
    let mut visible = 0usize;
    let mut ignored = false;
    for ch in text.chars() {
        match ch {
            '\u{1}' => ignored = true,
            '\u{2}' => ignored = false,
            _ if !ignored => visible += 1,
            _ => {}
        }
    }
    visible.min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_replace_line(text: *const c_char, clear_undo: c_int) {
    let text = c_text(text).unwrap_or_default();
    if clear_undo == 0 {
        save_undo();
    } else {
        readline_store().lock().expect("readline lock").undo.clear();
    }
    set_line_buffer(&text, text.len());
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert_text(text: *const c_char) -> c_int {
    let Some(text) = c_text(text) else { return 0 };
    save_undo();
    let (mut line, point) = line_snapshot();
    let point = clamp_boundary(&line, point);
    line.insert_str(point, &text);
    set_line_buffer(&line, point + text.len());
    text.len().min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_delete_text(start: c_int, end: c_int) -> c_int {
    let mut line = current_line();
    let start = clamp_boundary(&line, start.max(0) as usize);
    let end = clamp_boundary(&line, end.max(0) as usize).max(start);
    if start == end {
        return 0;
    }
    save_undo();
    line.replace_range(start..end, "");
    set_line_buffer(&line, start.min(line.len()));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_text(start: c_int, end: c_int) -> c_int {
    let line = current_line();
    let start = clamp_boundary(&line, start.max(0) as usize);
    let end = clamp_boundary(&line, end.max(0) as usize).max(start);
    if start < end {
        readline_store()
            .lock()
            .expect("readline lock")
            .kill_ring
            .push(line[start..end].to_string());
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert(count: c_int, key: c_int) -> c_int {
    let Some(ch) = char::from_u32(key as u32) else {
        return 1;
    };
    let text: String = std::iter::repeat_n(ch, count.max(1) as usize).collect();
    {
        let mut store = readline_store().lock().expect("readline lock");
        if store.macro_recording {
            store.current_macro.push_str(&text);
        }
    }
    if rl_insert_mode == 0 {
        let line = current_line();
        let start = clamp_boundary(&line, rl_point.max(0) as usize);
        let mut end = start;
        for _ in 0..count.max(1) {
            end = line[end..]
                .chars()
                .next()
                .map_or(end, |character| end + character.len_utf8());
        }
        if end > start {
            rl_delete_text(start as c_int, end as c_int);
        }
    }
    let text = clean_c_string(&text);
    rl_insert_text(text.as_ptr());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_forward_char(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    for _ in 0..count.max(0) {
        point = line[point..]
            .char_indices()
            .nth(1)
            .map_or(line.len(), |(offset, _)| point + offset);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_char(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    for _ in 0..count.max(0) {
        point = line[..point]
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_beg_of_line(_count: c_int, _key: c_int) -> c_int {
    rl_point = 0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_end_of_line(_count: c_int, _key: c_int) -> c_int {
    rl_point = rl_end;
    0
}

fn word_left(text: &str, point: usize) -> usize {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut index = chars.partition_point(|(offset, _)| *offset < point);
    while index > 0 && !chars[index - 1].1.is_alphanumeric() && chars[index - 1].1 != '_' {
        index -= 1;
    }
    while index > 0 && (chars[index - 1].1.is_alphanumeric() || chars[index - 1].1 == '_') {
        index -= 1;
    }
    chars
        .get(index)
        .map_or(text.len().min(point), |(offset, _)| *offset)
}

fn word_right(text: &str, point: usize) -> usize {
    let mut point = clamp_boundary(text, point);
    while point < text.len() {
        let ch = text[point..].chars().next().unwrap();
        if ch.is_alphanumeric() || ch == '_' {
            break;
        }
        point += ch.len_utf8();
    }
    while point < text.len() {
        let ch = text[point..].chars().next().unwrap();
        if !ch.is_alphanumeric() && ch != '_' {
            break;
        }
        point += ch.len_utf8();
    }
    point
}

#[no_mangle]
pub unsafe extern "C" fn rl_forward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = rl_point.max(0) as usize;
    for _ in 0..count.max(0) {
        point = word_right(&line, point);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = rl_point.max(0) as usize;
    for _ in 0..count.max(0) {
        point = word_left(&line, point);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_rubout(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let end = clamp_boundary(&line, rl_point.max(0) as usize);
    let mut start = end;
    for _ in 0..count.max(0) {
        start = line[..start]
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_delete(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = clamp_boundary(&line, rl_point.max(0) as usize);
    let mut end = start;
    for _ in 0..count.max(0) {
        if end >= line.len() {
            break;
        }
        end += line[end..].chars().next().unwrap().len_utf8();
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_line(count: c_int, _key: c_int) -> c_int {
    if count < 0 {
        rl_kill_text(0, rl_point)
    } else {
        rl_kill_text(rl_point, rl_end)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_kill_line(_count: c_int, _key: c_int) -> c_int {
    rl_kill_text(0, rl_point)
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = rl_point.max(0) as usize;
    let mut end = start;
    for _ in 0..count.max(0) {
        end = word_right(&line, end);
    }
    rl_kill_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_kill_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let end = rl_point.max(0) as usize;
    let mut start = end;
    for _ in 0..count.max(0) {
        start = word_left(&line, start);
    }
    rl_kill_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_region(_count: c_int, _key: c_int) -> c_int {
    rl_kill_text(rl_mark.min(rl_point), rl_mark.max(rl_point))
}

#[no_mangle]
pub unsafe extern "C" fn rl_yank(count: c_int, _key: c_int) -> c_int {
    let text = readline_store()
        .lock()
        .expect("readline lock")
        .kill_ring
        .last()
        .cloned();
    if let Some(text) = text {
        let repeated = text.repeat(count.max(1) as usize);
        let repeated = clean_c_string(&repeated);
        rl_insert_text(repeated.as_ptr());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_mark(_count: c_int, _key: c_int) -> c_int {
    rl_mark = rl_point;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_exchange_point_and_mark(_count: c_int, _key: c_int) -> c_int {
    ptr::swap(ptr::addr_of_mut!(rl_point), ptr::addr_of_mut!(rl_mark));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_do_undo() -> c_int {
    let previous = readline_store().lock().expect("readline lock").undo.pop();
    if let Some((line, point)) = previous {
        set_line_buffer(&line, point);
        1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_undo_command(_count: c_int, _key: c_int) -> c_int {
    (rl_do_undo() == 0) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_newline(_count: c_int, _key: c_int) -> c_int {
    rl_done = 1;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_abort(_count: c_int, _key: c_int) -> c_int {
    -1
}

#[no_mangle]
pub unsafe extern "C" fn rl_ding() -> c_int {
    let bell = [7u8];
    libc::write(libc::STDERR_FILENO, bell.as_ptr().cast(), 1);
    -1
}

#[no_mangle]
pub extern "C" fn rl_alphabetic(character: c_int) -> c_int {
    char::from_u32(character as u32).is_some_and(|ch| ch.is_alphabetic()) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_free(value: *mut c_void) {
    libc::free(value);
}
