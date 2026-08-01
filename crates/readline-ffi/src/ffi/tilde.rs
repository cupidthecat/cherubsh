pub type tilde_hook_func_t = unsafe extern "C" fn(*mut c_char) -> *mut c_char;

#[no_mangle]
pub static mut tilde_expansion_preexpansion_hook: Option<tilde_hook_func_t> = None;
#[no_mangle]
pub static mut tilde_expansion_failure_hook: Option<tilde_hook_func_t> = None;
#[no_mangle]
pub static mut tilde_additional_prefixes: *mut *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut tilde_additional_suffixes: *mut *mut c_char = ptr::null_mut();

fn home_for_user(user: &str) -> Option<String> {
    if user.is_empty() {
        return std::env::var("HOME").ok();
    }
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            (fields.first() == Some(&user) && fields.len() > 5).then(|| fields[5].to_string())
        })
}

unsafe fn expand_tilde_word(input: &str) -> Option<String> {
    let Some(rest) = input.strip_prefix('~') else {
        return Some(input.to_string());
    };
    let (user, suffix) = rest
        .split_once('/')
        .map_or((rest, ""), |(user, suffix)| (user, suffix));
    if let Some(hook) = tilde_expansion_preexpansion_hook {
        let user = clean_c_string(user);
        let expanded = hook(user.as_ptr() as *mut c_char);
        if !expanded.is_null() {
            let base = c_text(expanded).unwrap_or_default();
            libc::free(expanded.cast());
            return Some(if suffix.is_empty() {
                base
            } else {
                format!("{base}/{suffix}")
            });
        }
    }
    if let Some(base) = home_for_user(user) {
        return Some(if suffix.is_empty() {
            base
        } else {
            format!("{base}/{suffix}")
        });
    }
    if let Some(hook) = tilde_expansion_failure_hook {
        let user = clean_c_string(user);
        let expanded = hook(user.as_ptr() as *mut c_char);
        if !expanded.is_null() {
            let base = c_text(expanded).unwrap_or_default();
            libc::free(expanded.cast());
            return Some(if suffix.is_empty() {
                base
            } else {
                format!("{base}/{suffix}")
            });
        }
    }
    None
}

#[no_mangle]
pub unsafe extern "C" fn tilde_expand_word(input: *const c_char) -> *mut c_char {
    let input = c_text(input).unwrap_or_default();
    expand_tilde_word(&input).map_or_else(|| malloc_string(&input), |value| malloc_string(&value))
}

#[no_mangle]
pub unsafe extern "C" fn tilde_expand(input: *const c_char) -> *mut c_char {
    let input = c_text(input).unwrap_or_default();
    let mut output = String::with_capacity(input.len());
    for (index, part) in input.split('/').enumerate() {
        if index > 0 {
            output.push('/');
        }
        if part.starts_with('~') && (index == 0 || output.ends_with([':', '='])) {
            output.push_str(&expand_tilde_word(part).unwrap_or_else(|| part.to_string()));
        } else {
            output.push_str(part);
        }
    }
    malloc_string(&output)
}

#[no_mangle]
pub unsafe extern "C" fn tilde_find_word(
    input: *const c_char,
    start: c_int,
    length: *mut c_int,
) -> *mut c_char {
    let input = c_text(input).unwrap_or_default();
    let start = start.max(0) as usize;
    if start >= input.len() || input.as_bytes()[start] != b'~' {
        if !length.is_null() {
            *length = 0;
        }
        return ptr::null_mut();
    }
    let end = input[start..]
        .find(|ch: char| ch == '/' || ch.is_whitespace() || matches!(ch, ':' | '='))
        .map_or(input.len(), |offset| start + offset);
    if !length.is_null() {
        *length = (end - start).min(c_int::MAX as usize) as c_int;
    }
    malloc_string(&input[start..end])
}
