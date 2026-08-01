#![no_main]

use std::ffi::CString;
use std::ptr;

use libfuzzer_sys::fuzz_target;
use readline::{
    add_history, clear_history, copy_history_entry, current_history, free_history_entry,
    history_expand, history_get_history_state, history_set_pos, remove_history, using_history,
};

fuzz_target!(|data: &[u8]| {
    let data = &data[..data.len().min(64 * 1024)];
    clear_history();
    using_history();

    for chunk in data.split(|byte| *byte == 0).take(16) {
        let line = CString::new(chunk).expect("split input cannot contain NUL");
        unsafe { add_history(line.as_ptr()) };

        let entry = current_history();
        if !entry.is_null() {
            let copy = unsafe { copy_history_entry(entry) };
            if !copy.is_null() {
                let _ = unsafe { free_history_entry(copy) };
            }
        }

        let mut expanded = ptr::null_mut();
        unsafe {
            history_expand(line.as_ptr(), &mut expanded);
            libc::free(expanded.cast());
        }
    }

    let position = data.first().copied().map_or(0, i32::from);
    let _ = history_set_pos(position);
    let state = history_get_history_state();
    if !state.is_null() {
        unsafe { libc::free(state.cast()) };
    }
    loop {
        let entry = unsafe { remove_history(0) };
        if entry.is_null() {
            break;
        }
        let _ = unsafe { free_history_entry(entry) };
    }
    clear_history();
});
