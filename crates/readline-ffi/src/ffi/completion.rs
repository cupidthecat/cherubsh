fn common_prefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut end = first.len();
    for value in &values[1..] {
        let matching = first
            .chars()
            .zip(value.chars())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch.len_utf8())
            .sum();
        end = end.min(matching);
    }
    first[..end].to_string()
}

unsafe fn completion_array(values: &[String], include_common: bool) -> *mut *mut c_char {
    let extra = usize::from(include_common);
    let output = libc::calloc(values.len() + extra + 1, std::mem::size_of::<*mut c_char>())
        as *mut *mut c_char;
    if output.is_null() {
        return ptr::null_mut();
    }
    let mut offset = 0usize;
    if include_common {
        *output = malloc_string(&common_prefix(values));
        offset = 1;
    }
    for (index, value) in values.iter().enumerate() {
        *output.add(index + offset) = malloc_string(value);
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn rl_completion_matches(
    text: *const c_char,
    generator: Option<rl_compentry_func_t>,
) -> *mut *mut c_char {
    let Some(generator) = generator else {
        return ptr::null_mut();
    };
    let matches = run_generator(generator, &c_text(text).unwrap_or_default());
    if matches.is_empty() {
        ptr::null_mut()
    } else {
        completion_array(&matches, true)
    }
}

#[no_mangle]
pub unsafe extern "C" fn completion_matches(
    text: *mut c_char,
    generator: Option<rl_compentry_func_t>,
) -> *mut *mut c_char {
    rl_completion_matches(text, generator)
}

#[no_mangle]
pub unsafe extern "C" fn rl_filename_completion_function(
    text: *const c_char,
    state: c_int,
) -> *mut c_char {
    let text = c_text(text).unwrap_or_default();
    let mut store = readline_store().lock().expect("readline lock");
    if state == 0
        || store
            .filename_generator
            .as_ref()
            .is_none_or(|(saved, _, _)| saved != &text)
    {
        store.filename_generator = Some((text.clone(), filename_matches(&text), 0));
    }
    let Some((_, values, index)) = store.filename_generator.as_mut() else {
        return ptr::null_mut();
    };
    let Some(value) = values.get(*index).cloned() else {
        return ptr::null_mut();
    };
    *index += 1;
    malloc_string(&value)
}

#[no_mangle]
pub unsafe extern "C" fn filename_completion_function(
    text: *const c_char,
    state: c_int,
) -> *mut c_char {
    rl_filename_completion_function(text, state)
}

#[no_mangle]
pub unsafe extern "C" fn rl_username_completion_function(
    text: *const c_char,
    state: c_int,
) -> *mut c_char {
    let text = c_text(text).unwrap_or_default();
    let prefix = text.strip_prefix('~').unwrap_or(&text);
    let users: Vec<String> = std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| name.starts_with(prefix))
        .map(|name| format!("~{name}"))
        .collect();
    users
        .get(state.max(0) as usize)
        .map_or(ptr::null_mut(), |value| malloc_string(value))
}

#[no_mangle]
pub unsafe extern "C" fn username_completion_function(
    text: *const c_char,
    state: c_int,
) -> *mut c_char {
    rl_username_completion_function(text, state)
}

#[no_mangle]
pub unsafe extern "C" fn rl_complete_internal(_what_to_do: c_int) -> c_int {
    let line = current_line();
    let completion = ffi_complete(&line, rl_point.max(0) as usize);
    if completion.matches.is_empty() {
        return rl_ding();
    }
    let chosen = if completion.matches.len() == 1 {
        completion.matches[0].clone()
    } else {
        common_prefix(&completion.matches)
    };
    let start = clamp_boundary(&line, completion.replace_start);
    let point = clamp_boundary(&line, rl_point.max(0) as usize);
    if chosen.is_empty() || chosen == line[start..point] {
        if _what_to_do == b'?' as c_int || _what_to_do == b'!' as c_int {
            let array = completion_array(&completion.matches, false);
            rl_display_match_list(array, completion.matches.len() as c_int, 0);
            free_string_array(array);
        }
        return 0;
    }
    save_undo();
    let mut updated = line;
    updated.replace_range(start..point, &chosen);
    let mut new_point = start + chosen.len();
    if completion.matches.len() == 1 && !completion.suppress_append {
        if let Some(ch) = completion.append_character {
            updated.insert(new_point, ch);
            new_point += ch.len_utf8();
        }
    }
    set_line_buffer(&updated, new_point);
    0
}

unsafe fn free_string_array(array: *mut *mut c_char) {
    if array.is_null() {
        return;
    }
    let mut index = 0usize;
    while !(*array.add(index)).is_null() {
        libc::free((*array.add(index)).cast());
        index += 1;
    }
    libc::free(array.cast());
}

#[no_mangle]
pub unsafe extern "C" fn rl_display_match_list(matches: *mut *mut c_char, len: c_int, max: c_int) {
    if let Some(hook) = rl_completion_display_matches_hook {
        hook(matches, len, max);
        return;
    }
    if matches.is_null() {
        return;
    }
    libc::write(libc::STDERR_FILENO, b"\n".as_ptr().cast(), 1);
    for index in 0..len.max(0) as usize {
        let value = *matches.add(index);
        if value.is_null() {
            break;
        }
        let bytes = CStr::from_ptr(value).to_bytes();
        libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len());
        libc::write(libc::STDERR_FILENO, b"\n".as_ptr().cast(), 1);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_complete(_count: c_int, _key: c_int) -> c_int {
    rl_complete_internal(b'\t' as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_possible_completions(_count: c_int, _key: c_int) -> c_int {
    rl_complete_internal(b'?' as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert_completions(_count: c_int, _key: c_int) -> c_int {
    rl_complete_internal(b'*' as c_int)
}
