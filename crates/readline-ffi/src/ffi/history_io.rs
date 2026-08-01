fn history_path(filename: *const c_char) -> Option<PathBuf> {
    if let Some(filename) = c_text(filename) {
        return Some(PathBuf::from(filename));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".history"))
}

fn io_error_code(error: &std::io::Error) -> c_int {
    error.raw_os_error().unwrap_or(libc::EIO)
}

unsafe fn read_history_impl(filename: *const c_char, from: c_int, to: c_int) -> c_int {
    let Some(path) = history_path(filename) else {
        return libc::ENOENT;
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return io_error_code(&error),
    };
    let mut timestamp: Option<String> = None;
    let mut accepted = 0usize;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(error) => return io_error_code(&error),
        };
        if index < from.max(0) as usize {
            continue;
        }
        if to >= from && index > to as usize {
            break;
        }
        if line.starts_with('#') && line[1..].bytes().all(|byte| byte.is_ascii_digit()) {
            timestamp = Some(line);
            continue;
        }
        let entry = alloc_entry(&line, timestamp.take().as_deref(), ptr::null_mut());
        if !entry.is_null() {
            let mut store = history_store().lock().expect("history lock");
            store.entries.push(entry as usize);
            store.trim();
            accepted += 1;
        }
    }
    history_lines_read_from_file = accepted.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn read_history(filename: *const c_char) -> c_int {
    read_history_impl(filename, 0, -1)
}

#[no_mangle]
pub unsafe extern "C" fn read_history_range(
    filename: *const c_char,
    from: c_int,
    to: c_int,
) -> c_int {
    read_history_impl(filename, from, to)
}

unsafe fn write_history_impl(filename: *const c_char, append: bool, count: Option<usize>) -> c_int {
    let Some(path) = history_path(filename) else {
        return libc::ENOENT;
    };
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) => return io_error_code(&error),
    };
    let store = history_store().lock().expect("history lock");
    let start = count
        .map(|count| store.entries.len().saturating_sub(count))
        .unwrap_or(0);
    let mut written = 0usize;
    for entry in &store.entries[start..] {
        let entry = *entry as *mut HIST_ENTRY;
        if history_write_timestamps != 0 {
            if let Some(timestamp) = c_text((*entry).timestamp) {
                if let Err(error) = writeln!(file, "{timestamp}") {
                    return io_error_code(&error);
                }
            }
        }
        let line = c_text((*entry).line).unwrap_or_default();
        if let Err(error) = writeln!(file, "{line}") {
            return io_error_code(&error);
        }
        written += 1;
    }
    history_lines_written_to_file = written.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn write_history(filename: *const c_char) -> c_int {
    write_history_impl(filename, false, None)
}

#[no_mangle]
pub unsafe extern "C" fn append_history(count: c_int, filename: *const c_char) -> c_int {
    write_history_impl(filename, true, Some(count.max(0) as usize))
}

#[no_mangle]
pub unsafe extern "C" fn history_truncate_file(filename: *const c_char, lines: c_int) -> c_int {
    let Some(path) = history_path(filename) else {
        return libc::ENOENT;
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => return io_error_code(&error),
    };
    let all: Vec<&str> = contents.lines().collect();
    let start = all.len().saturating_sub(lines.max(0) as usize);
    let output = if start < all.len() {
        format!("{}\n", all[start..].join("\n"))
    } else {
        String::new()
    };
    std::fs::write(path, output).map_or_else(|error| io_error_code(&error), |_| 0)
}

#[no_mangle]
pub extern "C" fn history_get_history_state() -> *mut HISTORY_STATE {
    let mut store = history_store().lock().expect("history lock");
    if store.entries.capacity() < store.size {
        let additional = store.size.saturating_sub(store.entries.len());
        store.entries.reserve_exact(additional);
    }
    let entries_pointer = if store.size == 0 {
        ptr::null_mut()
    } else {
        unsafe { *store.entries.as_mut_ptr().add(store.entries.len()) = 0 };
        store.entries_exposed = true;
        store.entries.as_mut_ptr().cast()
    };
    let state =
        unsafe { libc::calloc(1, std::mem::size_of::<HISTORY_STATE>()) } as *mut HISTORY_STATE;
    if state.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*state).entries = entries_pointer;
        (*state).offset = store.offset.min(c_int::MAX as usize) as c_int;
        (*state).length = store.entries.len().min(c_int::MAX as usize) as c_int;
        (*state).size = store.size.min(c_int::MAX as usize) as c_int;
        (*state).flags = if store.stifled.is_some() { 1 } else { 0 };
    }
    state
}

#[no_mangle]
pub unsafe extern "C" fn history_set_history_state(state: *mut HISTORY_STATE) {
    if state.is_null() {
        return;
    }
    let mut store = history_store().lock().expect("history lock");
    let length = (*state).length.max(0) as usize;
    let requested_values: Vec<usize> = if (*state).entries.is_null() {
        Vec::new()
    } else {
        (0..length)
            .map(|index| *(*state).entries.add(index) as usize)
            .collect()
    };
    let current_pointer = if store.entries.capacity() == 0 {
        ptr::null_mut()
    } else {
        store.entries.as_mut_ptr().cast::<*mut HIST_ENTRY>()
    };
    let requested_pointer = (*state).entries;
    if requested_pointer != current_pointer {
        if store.entries_exposed && store.entries.capacity() > 0 {
            let entries = std::mem::take(&mut store.entries);
            store.saved_entry_vectors.push(entries);
        } else {
            store.entries.clear();
        }
        let saved_index = store.saved_entry_vectors.iter_mut().position(|entries| {
            entries.capacity() > 0
                && entries.as_mut_ptr().cast::<*mut HIST_ENTRY>() == requested_pointer
        });
        store.entries = if let Some(index) = saved_index {
            store.saved_entry_vectors.swap_remove(index)
        } else if requested_pointer.is_null() {
            Vec::new()
        } else {
            requested_values.clone()
        };
    }
    if store.entries.len() != length {
        store.entries.clear();
        store.entries.extend_from_slice(&requested_values);
    }
    store.entries_exposed = requested_pointer == store.entries.as_mut_ptr().cast();
    store.offset = (*state).offset.max(0) as usize;
    store.stifled = ((*state).flags & 1 != 0).then_some(store.last_stifle_limit);
    store.size = (*state).size.max(0) as usize;
    if store.entries.capacity() < store.size {
        let additional = store.size.saturating_sub(store.entries.len());
        store.entries.reserve_exact(additional);
    }
    store.list_cache.clear();
    history_length = store.entries.len().min(c_int::MAX as usize) as c_int;
    history_offset = store.offset.min(c_int::MAX as usize) as c_int;
    history_max_entries = store.last_stifle_limit.min(c_int::MAX as usize) as c_int;
    max_input_history = history_max_entries;
}

fn history_table_snapshot() -> HistoryTable {
    let store = history_store().lock().expect("history lock");
    let mut table = HistoryTable::new(store.entries.len().max(1));
    for entry in &store.entries {
        let entry = *entry as *mut HIST_ENTRY;
        let line = unsafe { c_text((*entry).line) }.unwrap_or_default();
        let timestamp = unsafe { c_text((*entry).timestamp) }
            .and_then(|value| value.trim_start_matches('#').parse().ok());
        table.add_forced(&line, timestamp);
    }
    table
}

#[no_mangle]
pub unsafe extern "C" fn history_expand(input: *const c_char, output: *mut *mut c_char) -> c_int {
    if output.is_null() {
        return -1;
    }
    let input = c_text(input).unwrap_or_default();
    let result = histexpand::expand_with_options(&input, &history_table_snapshot(), false);
    if let Some(error) = result.error {
        *output = malloc_string(&error);
        -1
    } else {
        *output = malloc_string(&result.line);
        if result.print_only {
            2
        } else if result.changed {
            1
        } else {
            0
        }
    }
}

fn history_words(input: &str) -> Vec<String> {
    fn scan_word(input: &[u8], start: usize, delimiters: &[u8]) -> usize {
        let mut index = start;
        let mut delimiter = 0;
        let mut nesting = 0usize;
        let mut opening = 0;

        if matches!(input[index], b'(' | b')' | b'\n') {
            return index + 1;
        }

        if input[index].is_ascii_digit() {
            let mut end = index;
            while end < input.len() && input[end].is_ascii_digit() {
                end += 1;
            }
            if end == input.len() {
                return end;
            }
            index = end;
            if !matches!(input[end], b'<' | b'>') {
                // The digit sequence is part of an ordinary word.
            } else {
                // Let the redirection parser below include the descriptor.
            }
        }

        if index < input.len() && matches!(input[index], b'<' | b'>' | b';' | b'&' | b'|') {
            let current = input[index];
            let next = input.get(index + 1).copied().unwrap_or_default();
            if next == current {
                let mut end = index + 2;
                if current == b'<' && matches!(input.get(end), Some(b'-' | b'<')) {
                    end += 1;
                }
                return end;
            }
            if next == b'&' && matches!(current, b'<' | b'>') {
                let mut end = index + 2;
                while end < input.len() && input[end].is_ascii_digit() {
                    end += 1;
                }
                if input.get(end) == Some(&b'-') {
                    end += 1;
                }
                return end;
            }
            if matches!(current, b'&' | b'|') && next == b'>' {
                return index + 2;
            }
            if matches!(current, b'<' | b'>') && next == b'(' {
                index += 2;
                opening = b'(';
                delimiter = b')';
                nesting = 1;
            } else {
                return index + 1;
            }
        }

        if delimiter == 0 && matches!(input.get(index), Some(b'"' | b'\'' | b'`')) {
            delimiter = input[index];
            index += 1;
        }

        while index < input.len() {
            let current = input[index];
            if current == b'\\' && input.get(index + 1) == Some(&b'\n') {
                index += 2;
                continue;
            }
            if current == b'\\' && delimiter != b'\'' {
                index = (index + 2).min(input.len());
                continue;
            }
            if nesting > 0 && current == opening {
                nesting += 1;
                index += 1;
                continue;
            }
            if nesting > 0 && current == delimiter {
                nesting -= 1;
                if nesting == 0 {
                    delimiter = 0;
                }
                index += 1;
                continue;
            }
            if delimiter != 0 && current == delimiter {
                delimiter = 0;
                index += 1;
                continue;
            }
            if nesting == 0
                && delimiter == 0
                && matches!(
                    current,
                    b'<' | b'>' | b'$' | b'!' | b'@' | b'?' | b'+' | b'*'
                )
                && input.get(index + 1) == Some(&b'(')
            {
                index += 2;
                opening = b'(';
                delimiter = b')';
                nesting = 1;
                continue;
            }
            if delimiter == 0 && delimiters.contains(&current) {
                break;
            }
            if delimiter == 0 && matches!(current, b'"' | b'\'' | b'`') {
                delimiter = current;
            }
            index += 1;
        }
        index
    }

    let bytes = input.as_bytes();
    let delimiters = unsafe {
        c_text(history_word_delimiters)
            .unwrap_or_else(|| " \t\n;&()|<>".to_owned())
            .into_bytes()
    };
    let comment = unsafe { c_char_byte(history_comment_char) };
    let mut words = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\n') {
            index += 1;
        }
        if index == bytes.len() || bytes[index] == comment {
            break;
        }
        let start = index;
        index = scan_word(bytes, start, &delimiters);
        if index == start && delimiters.contains(&bytes[index]) {
            index += 1;
            while index < bytes.len() && delimiters.contains(&bytes[index]) {
                index += 1;
            }
        }
        words.push(input[start..index].to_owned());
    }
    words
}

#[no_mangle]
pub unsafe extern "C" fn history_arg_extract(
    first: c_int,
    last: c_int,
    input: *const c_char,
) -> *mut c_char {
    let words = history_words(&c_text(input).unwrap_or_default());
    if words.is_empty() {
        return ptr::null_mut();
    }
    let length = words.len() as isize;
    let first = if first < 0 {
        length + first as isize - 1
    } else if first == b'$' as c_int {
        length - 1
    } else {
        first as isize
    };
    let last = if last < 0 {
        length + last as isize - 1
    } else if last == b'$' as c_int {
        length - 1
    } else {
        last as isize
    };
    if first < 0 || last < 0 || first >= length || last >= length || first > last {
        return ptr::null_mut();
    }
    malloc_string(&words[first as usize..=last as usize].join(" "))
}

#[no_mangle]
pub unsafe extern "C" fn history_tokenize(input: *const c_char) -> *mut *mut c_char {
    let words = history_words(&c_text(input).unwrap_or_default());
    if words.is_empty() {
        return ptr::null_mut();
    }
    let output =
        libc::calloc(words.len() + 1, std::mem::size_of::<*mut c_char>()) as *mut *mut c_char;
    if output.is_null() {
        return ptr::null_mut();
    }
    for (index, word) in words.iter().enumerate() {
        *output.add(index) = malloc_string(word);
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn get_history_event(
    input: *const c_char,
    index: *mut c_int,
    _delimiting_quote: c_int,
) -> *mut c_char {
    if input.is_null() || index.is_null() {
        return ptr::null_mut();
    }
    let input = c_text(input).unwrap_or_default();
    let start = (*index).max(0) as usize;
    let event = input[start..]
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ':' | ';' | '&' | '|'))
        .next()
        .unwrap_or_default();
    *index = start.saturating_add(event.len()).min(c_int::MAX as usize) as c_int;
    malloc_string(event)
}
