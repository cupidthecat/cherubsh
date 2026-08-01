#[no_mangle]
pub unsafe extern "C" fn rl_variable_bind(name: *const c_char, value: *const c_char) -> c_int {
    let Some(name) = c_text(name) else { return 1 };
    let value = c_text(value).unwrap_or_default();
    let normalized = name.to_ascii_lowercase();
    if normalized == "editing-mode" {
        let vi = value.eq_ignore_ascii_case("vi");
        rl_editing_mode = if vi { 0 } else { 1 };
        let mut keymap = if rl_editing_mode == 0 {
            let mut keymap = RustKeymap::new("vi-insert");
            keymap.install_vi_insert_defaults();
            keymap
        } else {
            let mut keymap = RustKeymap::new("emacs");
            keymap.install_emacs_defaults();
            keymap
        };
        let mut store = readline_store().lock().expect("readline lock");
        store.editor = Some(LineEditor::new(std::mem::replace(
            &mut keymap,
            RustKeymap::new("unused"),
        )));
        drop(store);
        let map_name = clean_c_string(if vi { "vi-insertion" } else { "emacs-standard" });
        let map = rl_get_keymap_by_name(map_name.as_ptr());
        if !map.is_null() {
            rl_set_keymap(map);
        }
    } else if normalized == "keymap" {
        let value = clean_c_string(&value);
        let keymap = rl_get_keymap_by_name(value.as_ptr());
        if keymap.is_null() {
            return 1;
        }
        rl_binding_keymap = keymap;
    } else if normalized == "completion-query-items" {
        if let Ok(number) = value.parse::<c_int>() {
            rl_completion_query_items = number;
        }
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .variables
        .insert(normalized, clean_c_string(&value));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_parse_and_bind(line: *mut c_char) -> c_int {
    parse_binding_line(&c_text(line).unwrap_or_default())
}

unsafe fn parse_binding_line(line: &str) -> c_int {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return 0;
    }
    if let Some(rest) = line.strip_prefix("set ") {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = clean_c_string(parts.next().unwrap_or_default());
        let value = clean_c_string(parts.next().unwrap_or_default().trim());
        return rl_variable_bind(name.as_ptr(), value.as_ptr());
    }
    let Some(colon) = binding_colon(line) else {
        return 1;
    };
    let sequence = unquote_inputrc(line[..colon].trim());
    if sequence.is_empty() {
        return 1;
    }
    let binding = line[colon + 1..].trim();
    let keymap = if rl_binding_keymap.is_null() {
        rl_get_keymap()
    } else {
        rl_binding_keymap
    };
    let sequence = clean_c_string(&normalize_keyseq(&sequence));
    if binding.starts_with(['"', '\'']) {
        let macro_text = clean_c_string(&unquote_inputrc(binding));
        rl_macro_bind(sequence.as_ptr(), macro_text.as_ptr(), keymap)
    } else {
        let command = binding.split_whitespace().next().unwrap_or_default().trim();
        let command = clean_c_string(command);
        let Some(function) = rl_named_function(command.as_ptr()) else {
            return 1;
        };
        rl_bind_keyseq_in_map(sequence.as_ptr(), Some(function), keymap)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_read_init_file(filename: *const c_char) -> c_int {
    let path = c_text(filename)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("INPUTRC").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".inputrc")))
        .or_else(|| Some(PathBuf::from("/etc/inputrc")));
    let Some(path) = path else {
        return libc::ENOENT;
    };
    let result = read_init_path(&path, 0);
    if result == 0 {
        readline_store()
            .lock()
            .expect("readline lock")
            .last_init_file = Some(path);
    }
    result
}

unsafe fn read_init_path(path: &std::path::Path, depth: usize) -> c_int {
    if depth > 32 {
        return libc::ELOOP;
    }
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => return io_error_code(&error),
    };
    let mut active = true;
    let mut conditions: Vec<(bool, bool)> = Vec::new();
    let mut logical = String::new();
    for physical in contents.lines() {
        logical.push_str(physical.trim_end_matches('\r'));
        if logical.ends_with('\\') {
            logical.pop();
            continue;
        }
        let line = logical.trim().to_string();
        logical.clear();
        if let Some(expression) = line.strip_prefix("$if") {
            let parent = active;
            let matched = parent && inputrc_condition(expression.trim());
            conditions.push((parent, matched));
            active = matched;
            continue;
        }
        if line.starts_with("$else") {
            let Some((parent, matched)) = conditions.last_mut() else {
                return 1;
            };
            *matched = !*matched && *parent;
            active = *matched;
            continue;
        }
        if line.starts_with("$endif") {
            if conditions.pop().is_none() {
                return 1;
            }
            active = conditions.last().is_none_or(|(_, matched)| *matched);
            continue;
        }
        if !active {
            continue;
        }
        if let Some(include) = line.strip_prefix("$include") {
            let include = expand_inputrc_path(include.trim());
            let include = if include.is_absolute() {
                include
            } else {
                path.parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(include)
            };
            let result = read_init_path(&include, depth + 1);
            if result != 0 {
                return result;
            }
            continue;
        }
        let result = parse_binding_line(&line);
        if result != 0 {
            return result;
        }
    }
    if conditions.is_empty() {
        0
    } else {
        1
    }
}

fn binding_colon(line: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '"' | '\'') {
            quote = Some(character);
        } else if quote.is_none() && character == ':' {
            return Some(index);
        }
    }
    None
}

fn unquote_inputrc(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn normalize_keyseq(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "escape" | "esc" => "\\e".to_string(),
        "rubout" | "del" => "\\C-?".to_string(),
        "newline" | "lfd" => "\\C-j".to_string(),
        "return" | "ret" => "\\C-m".to_string(),
        "space" | "spc" => " ".to_string(),
        "tab" => "\\C-i".to_string(),
        _ => {
            let mut normalized = value.to_string();
            for (from, to) in [
                ("Control-", "\\C-"),
                ("control-", "\\C-"),
                ("Ctrl-", "\\C-"),
                ("ctrl-", "\\C-"),
                ("Meta-", "\\M-"),
                ("meta-", "\\M-"),
            ] {
                normalized = normalized.replace(from, to);
            }
            if normalized.starts_with("C-") || normalized.starts_with("M-") {
                normalized.insert(0, '\\');
            }
            normalized
        }
    }
}

fn inputrc_condition(expression: &str) -> bool {
    let expression = expression.trim();
    if let Some((name, value)) = expression.split_once('=') {
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches(['"', '\'']);
        return match name.as_str() {
            "mode" => unsafe {
                (rl_editing_mode == 0 && value.eq_ignore_ascii_case("vi"))
                    || (rl_editing_mode != 0 && value.eq_ignore_ascii_case("emacs"))
            },
            "term" => std::env::var("TERM").is_ok_and(|term| {
                term.eq_ignore_ascii_case(value)
                    || term
                        .split_once('-')
                        .is_some_and(|(base, _)| base.eq_ignore_ascii_case(value))
            }),
            "version" => value
                .parse::<f32>()
                .is_ok_and(|version| (8.3_f32 - version).abs() < f32::EPSILON),
            _ => readline_store()
                .lock()
                .expect("readline lock")
                .variables
                .get(&name)
                .is_some_and(|current| current.to_string_lossy().eq_ignore_ascii_case(value)),
        };
    }
    let application = unsafe { c_text(rl_readline_name).unwrap_or_else(|| "other".to_string()) };
    application.eq_ignore_ascii_case(expression)
}

fn expand_inputrc_path(value: &str) -> PathBuf {
    let value = unquote_inputrc(value);
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
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
