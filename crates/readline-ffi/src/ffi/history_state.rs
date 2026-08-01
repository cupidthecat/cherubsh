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
