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
