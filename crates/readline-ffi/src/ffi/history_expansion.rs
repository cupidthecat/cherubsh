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
