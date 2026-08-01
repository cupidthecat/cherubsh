#[no_mangle]
pub unsafe extern "C" fn rl_extend_line_buffer(length: c_int) {
    if length < 0 {
        return;
    }
    let wanted = (length as usize).saturating_add(1);
    let mut store = readline_store().lock().expect("readline lock");
    if store.line_capacity >= wanted {
        return;
    }
    let mut capacity = store.line_capacity.max(256);
    while capacity < wanted {
        capacity = capacity.saturating_mul(2);
    }
    let allocation = libc::realloc(store.line_allocation as *mut c_void, capacity).cast::<c_char>();
    if allocation.is_null() {
        return;
    }
    store.line_allocation = allocation as usize;
    store.line_capacity = capacity;
    rl_line_buffer = allocation;
}

#[no_mangle]
pub unsafe extern "C" fn ding() -> c_int {
    rl_ding()
}

#[no_mangle]
pub extern "C" fn alphabetic(character: c_int) -> c_int {
    rl_alphabetic(character)
}
