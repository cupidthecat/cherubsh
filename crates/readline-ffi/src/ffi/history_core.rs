struct HistoryStore {
    entries: Vec<usize>,
    list_cache: Vec<usize>,
    saved_entry_vectors: Vec<Vec<usize>>,
    entries_exposed: bool,
    offset: usize,
    stifled: Option<usize>,
    last_stifle_limit: usize,
    size: usize,
    real_size: usize,
}

impl HistoryStore {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            list_cache: Vec::new(),
            saved_entry_vectors: Vec::new(),
            entries_exposed: false,
            offset: 0,
            stifled: None,
            last_stifle_limit: 0,
            size: 0,
            real_size: 0,
        }
    }

    fn changed(&mut self) {
        self.list_cache.clear();
        self.offset = self.offset.min(self.entries.len());
        if self.entries.capacity() > self.entries.len() {
            unsafe { *self.entries.as_mut_ptr().add(self.entries.len()) = 0 };
        }
        unsafe {
            history_length = self.entries.len().min(c_int::MAX as usize) as c_int;
            history_offset = self.offset.min(c_int::MAX as usize) as c_int;
            history_max_entries = self.last_stifle_limit.min(c_int::MAX as usize) as c_int;
            max_input_history = history_max_entries;
        }
    }

    fn trim(&mut self) {
        if let Some(maximum) = self.stifled {
            while self.entries.len() > maximum {
                let entry = self.entries.remove(0) as *mut HIST_ENTRY;
                unsafe { free_entry(entry) };
                unsafe { history_base = history_base.saturating_add(1) };
            }
        }
        self.changed();
    }
}

fn history_store() -> &'static Mutex<HistoryStore> {
    static STORE: OnceLock<Mutex<HistoryStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HistoryStore::new()))
}

fn history_grow_size(length: usize) -> usize {
    if length < 1024 {
        return 256;
    }
    let shifted = length >> 10;
    let bit_length = usize::BITS as usize - shifted.leading_zeros() as usize;
    let width = 10 + bit_length;
    ((1usize << (width / 2)) + (1usize << ((width - 1) / 2))).max(256)
}

fn c_text(value: *const c_char) -> Option<String> {
    if value.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn c_char_byte(value: c_char) -> u8 {
    value.to_ne_bytes()[0]
}

fn clean_c_string(value: &str) -> CString {
    CString::new(value.replace('\0', "")).expect("interior nul removed")
}

unsafe fn malloc_string(value: &str) -> *mut c_char {
    let bytes = clean_c_string(value).into_bytes_with_nul();
    let output = libc::malloc(bytes.len()) as *mut u8;
    if output.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
    output.cast()
}

unsafe fn allocation_failure(name: &[u8]) -> ! {
    let _ = libc::write(libc::STDERR_FILENO, name.as_ptr().cast(), name.len());
    libc::exit(2)
}

#[no_mangle]
pub unsafe extern "C" fn xmalloc(bytes: usize) -> *mut c_void {
    let output = libc::malloc(bytes);
    if output.is_null() {
        allocation_failure(b"xmalloc: out of virtual memory\n");
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn xrealloc(pointer: *mut c_void, bytes: usize) -> *mut c_void {
    let output = if pointer.is_null() {
        libc::malloc(bytes)
    } else {
        libc::realloc(pointer, bytes)
    };
    if output.is_null() {
        allocation_failure(b"xrealloc: out of virtual memory\n");
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn xfree(pointer: *mut c_void) {
    if !pointer.is_null() {
        libc::free(pointer);
    }
}

unsafe fn alloc_entry(line: &str, timestamp: Option<&str>, data: histdata_t) -> *mut HIST_ENTRY {
    let entry = libc::calloc(1, std::mem::size_of::<HIST_ENTRY>()) as *mut HIST_ENTRY;
    if entry.is_null() {
        return ptr::null_mut();
    }
    (*entry).line = malloc_string(line);
    (*entry).timestamp = timestamp.map_or(ptr::null_mut(), |value| malloc_string(value));
    (*entry).data = data;
    entry
}

unsafe fn free_entry(entry: *mut HIST_ENTRY) -> histdata_t {
    if entry.is_null() {
        return ptr::null_mut();
    }
    let data = (*entry).data;
    libc::free((*entry).line.cast());
    libc::free((*entry).timestamp.cast());
    libc::free(entry.cast());
    data
}

unsafe fn clone_entry(entry: *mut HIST_ENTRY) -> *mut HIST_ENTRY {
    if entry.is_null() {
        return ptr::null_mut();
    }
    alloc_entry(
        &c_text((*entry).line).unwrap_or_default(),
        c_text((*entry).timestamp).as_deref(),
        (*entry).data,
    )
}

#[no_mangle]
pub extern "C" fn using_history() {
    let mut store = history_store().lock().expect("history lock");
    store.offset = store.entries.len();
    store.changed();
}

#[no_mangle]
pub unsafe extern "C" fn add_history(line: *const c_char) {
    let Some(line) = c_text(line) else { return };
    let timestamp = format!(
        "{}{}",
        char::from(c_char_byte(history_comment_char)),
        libc::time(ptr::null_mut())
    );
    let entry = alloc_entry(&line, Some(&timestamp), ptr::null_mut());
    if entry.is_null() {
        return;
    }
    let mut store = history_store().lock().expect("history lock");
    let mut advanced = false;
    if let Some(maximum) = store.stifled {
        if maximum == 0 {
            free_entry(entry);
            return;
        }
        if store.entries.len() == maximum {
            let removed = store.entries.remove(0) as *mut HIST_ENTRY;
            free_entry(removed);
            history_base = history_base.saturating_add(1);
            store.size = store.size.saturating_sub(1);
            advanced = true;
        }
    }
    if store.size == 0 {
        let initial = store
            .stifled
            .filter(|maximum| *maximum > 0)
            .map_or(502, |maximum| maximum.min(8192) + 2);
        store.size = initial;
        store.real_size = initial;
    } else if store.entries.len() == store.size.saturating_sub(1) {
        let entry_count = store.entries.len() + usize::from(advanced);
        let growth = history_grow_size(entry_count);
        store.real_size = if advanced {
            entry_count.saturating_add(growth)
        } else {
            store.real_size.saturating_add(growth)
        };
        store.size = store.real_size;
    }
    if store.entries.capacity() < store.size {
        let additional = store.size.saturating_sub(store.entries.len());
        store.entries.reserve_exact(additional);
    }
    store.entries.push(entry as usize);
    store.changed();
}

#[no_mangle]
pub unsafe extern "C" fn add_history_time(timestamp: *const c_char) {
    let Some(timestamp) = c_text(timestamp) else {
        return;
    };
    let store = history_store().lock().expect("history lock");
    let Some(entry) = store.entries.last().copied() else {
        return;
    };
    let entry = entry as *mut HIST_ENTRY;
    libc::free((*entry).timestamp.cast());
    (*entry).timestamp = malloc_string(&timestamp);
}

#[no_mangle]
pub unsafe extern "C" fn alloc_history_entry(
    line: *mut c_char,
    timestamp: *mut c_char,
) -> *mut HIST_ENTRY {
    let entry = libc::calloc(1, std::mem::size_of::<HIST_ENTRY>()) as *mut HIST_ENTRY;
    if entry.is_null() {
        return ptr::null_mut();
    }
    (*entry).line = malloc_string(&c_text(line).unwrap_or_default());
    if (*entry).line.is_null() {
        libc::free(entry.cast());
        return ptr::null_mut();
    }
    (*entry).timestamp = timestamp;
    (*entry).data = ptr::null_mut();
    entry
}

#[no_mangle]
pub unsafe extern "C" fn copy_history_entry(entry: *mut HIST_ENTRY) -> *mut HIST_ENTRY {
    clone_entry(entry)
}

#[no_mangle]
pub unsafe extern "C" fn free_history_entry(entry: *mut HIST_ENTRY) -> histdata_t {
    free_entry(entry)
}

#[no_mangle]
pub unsafe extern "C" fn remove_history(which: c_int) -> *mut HIST_ENTRY {
    if which < 0 {
        return ptr::null_mut();
    }
    let mut store = history_store().lock().expect("history lock");
    let index = which as usize;
    if index >= store.entries.len() {
        return ptr::null_mut();
    }
    let entry = store.entries.remove(index) as *mut HIST_ENTRY;
    store.changed();
    entry
}

#[no_mangle]
pub unsafe extern "C" fn remove_history_range(first: c_int, last: c_int) -> *mut *mut HIST_ENTRY {
    if first < 0 || last < first {
        return ptr::null_mut();
    }
    let mut store = history_store().lock().expect("history lock");
    let start = first as usize;
    if start >= store.entries.len() {
        return ptr::null_mut();
    }
    let end = (last as usize + 1).min(store.entries.len());
    let removed: Vec<usize> = store.entries.drain(start..end).collect();
    store.changed();
    let bytes = (removed.len() + 1) * std::mem::size_of::<*mut HIST_ENTRY>();
    let output = libc::calloc(1, bytes) as *mut *mut HIST_ENTRY;
    if output.is_null() {
        for entry in removed {
            free_entry(entry as *mut HIST_ENTRY);
        }
        return ptr::null_mut();
    }
    for (index, entry) in removed.into_iter().enumerate() {
        *output.add(index) = entry as *mut HIST_ENTRY;
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn replace_history_entry(
    which: c_int,
    line: *const c_char,
    data: histdata_t,
) -> *mut HIST_ENTRY {
    if which < 0 {
        return ptr::null_mut();
    }
    let Some(line) = c_text(line) else {
        return ptr::null_mut();
    };
    let mut store = history_store().lock().expect("history lock");
    let index = which as usize;
    if index >= store.entries.len() {
        return ptr::null_mut();
    }
    let timestamp = c_text((*(store.entries[index] as *mut HIST_ENTRY)).timestamp);
    let replacement = alloc_entry(&line, timestamp.as_deref(), data);
    if replacement.is_null() {
        return ptr::null_mut();
    }
    let previous = std::mem::replace(&mut store.entries[index], replacement as usize);
    store.changed();
    previous as *mut HIST_ENTRY
}

#[no_mangle]
pub extern "C" fn clear_history() {
    let mut store = history_store().lock().expect("history lock");
    for entry in store.entries.drain(..) {
        unsafe { free_entry(entry as *mut HIST_ENTRY) };
    }
    unsafe { history_base = 1 };
    store.offset = 0;
    store.changed();
}

#[no_mangle]
pub extern "C" fn stifle_history(maximum: c_int) {
    let mut store = history_store().lock().expect("history lock");
    let maximum = maximum.max(0) as usize;
    store.stifled = Some(maximum);
    store.last_stifle_limit = maximum;
    store.trim();
}

#[no_mangle]
pub extern "C" fn unstifle_history() -> c_int {
    let mut store = history_store().lock().expect("history lock");
    let previous = store.stifled.take();
    store.changed();
    previous.map_or_else(
        || -(store.last_stifle_limit.min(c_int::MAX as usize) as c_int),
        |value| value.min(c_int::MAX as usize) as c_int,
    )
}

#[no_mangle]
pub extern "C" fn history_is_stifled() -> c_int {
    history_store()
        .lock()
        .expect("history lock")
        .stifled
        .is_some() as c_int
}

#[no_mangle]
pub extern "C" fn history_list() -> *mut *mut HIST_ENTRY {
    let mut store = history_store().lock().expect("history lock");
    if store.entries.is_empty() {
        return ptr::null_mut();
    }
    store.list_cache = store.entries.clone();
    store.list_cache.push(0);
    store.list_cache.as_mut_ptr().cast()
}

#[no_mangle]
pub extern "C" fn where_history() -> c_int {
    history_store()
        .lock()
        .expect("history lock")
        .offset
        .min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub extern "C" fn current_history() -> *mut HIST_ENTRY {
    let store = history_store().lock().expect("history lock");
    store.entries.get(store.offset).copied().unwrap_or(0) as *mut HIST_ENTRY
}

#[no_mangle]
pub extern "C" fn history_get(offset: c_int) -> *mut HIST_ENTRY {
    let base = unsafe { history_base };
    if offset < base {
        return ptr::null_mut();
    }
    history_store()
        .lock()
        .expect("history lock")
        .entries
        .get((offset - base) as usize)
        .copied()
        .unwrap_or(0) as *mut HIST_ENTRY
}

#[no_mangle]
pub unsafe extern "C" fn history_get_time(entry: *mut HIST_ENTRY) -> libc::time_t {
    if entry.is_null() {
        return 0;
    }
    let marker = char::from(c_char_byte(history_comment_char));
    c_text((*entry).timestamp)
        .and_then(|value| value.strip_prefix(marker).map(str::to_owned))
        .and_then(|value| value.parse::<libc::time_t>().ok())
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn history_total_bytes() -> c_int {
    let store = history_store().lock().expect("history lock");
    store
        .entries
        .iter()
        .map(|entry| libc::strlen((*entry as *mut HIST_ENTRY).as_ref().unwrap().line))
        .sum::<usize>()
        .min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub extern "C" fn history_set_pos(position: c_int) -> c_int {
    if position < 0 {
        return 0;
    }
    let mut store = history_store().lock().expect("history lock");
    if position as usize > store.entries.len() {
        return 0;
    }
    store.offset = position as usize;
    store.changed();
    1
}

#[no_mangle]
pub extern "C" fn previous_history() -> *mut HIST_ENTRY {
    let mut store = history_store().lock().expect("history lock");
    if store.offset == 0 {
        return ptr::null_mut();
    }
    store.offset -= 1;
    let result = store.entries[store.offset] as *mut HIST_ENTRY;
    store.changed();
    result
}

#[no_mangle]
pub extern "C" fn next_history() -> *mut HIST_ENTRY {
    let mut store = history_store().lock().expect("history lock");
    if store.offset >= store.entries.len() {
        return ptr::null_mut();
    }
    store.offset += 1;
    let result = store.entries.get(store.offset).copied().unwrap_or(0) as *mut HIST_ENTRY;
    store.changed();
    result
}

fn search_history(needle: &str, direction: c_int, prefix: bool, position: Option<usize>) -> c_int {
    let mut store = history_store().lock().expect("history lock");
    let start = position.unwrap_or(store.offset).min(store.entries.len());
    let indices: Box<dyn Iterator<Item = usize>> = if direction < 0 {
        if store.entries.is_empty() {
            Box::new(std::iter::empty())
        } else {
            Box::new((0..=start.min(store.entries.len() - 1)).rev())
        }
    } else {
        Box::new(start..store.entries.len())
    };
    for index in indices {
        let entry = store.entries[index] as *mut HIST_ENTRY;
        let line = unsafe { c_text((*entry).line) }.unwrap_or_default();
        let found = if prefix {
            line.starts_with(needle).then_some(0)
        } else {
            line.find(needle)
        };
        if let Some(column) = found {
            store.offset = index;
            store.changed();
            return column.min(c_int::MAX as usize) as c_int;
        }
    }
    -1
}

#[no_mangle]
pub unsafe extern "C" fn history_search(needle: *const c_char, direction: c_int) -> c_int {
    c_text(needle).map_or(-1, |needle| search_history(&needle, direction, false, None))
}

#[no_mangle]
pub unsafe extern "C" fn history_search_prefix(needle: *const c_char, direction: c_int) -> c_int {
    c_text(needle).map_or(-1, |needle| search_history(&needle, direction, true, None))
}

#[no_mangle]
pub unsafe extern "C" fn history_search_pos(
    needle: *const c_char,
    direction: c_int,
    position: c_int,
) -> c_int {
    if position < 0 {
        return -1;
    }
    let Some(needle) = c_text(needle) else {
        return -1;
    };
    let result = search_history(&needle, direction, false, Some(position as usize));
    if result < 0 {
        -1
    } else {
        where_history()
    }
}
