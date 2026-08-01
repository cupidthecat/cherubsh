#![allow(clippy::missing_safety_doc)]
#![allow(dead_code)]
#![allow(non_camel_case_types)]

use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::raw::{c_char, c_int, c_ulong, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cherubsh_common::{histexpand, EditAction, HistoryTable, Keymap as RustKeymap};
use cherubsh_lineedit::{
    set_input_deadline, Completion, CompletionProvider, EditError, HistoryProvider, LineEditor,
    RawMode,
};

unsafe extern "C" {
    #[link_name = "stdin"]
    static mut C_STDIN: *mut libc::FILE;
    #[link_name = "stdout"]
    static mut C_STDOUT: *mut libc::FILE;
}

pub type histdata_t = *mut c_void;
pub type rl_command_func_t = unsafe extern "C" fn(c_int, c_int) -> c_int;
pub type rl_vcpfunc_t = unsafe extern "C" fn(*mut c_char);
pub type rl_hook_func_t = unsafe extern "C" fn() -> c_int;
pub type rl_compentry_func_t = unsafe extern "C" fn(*const c_char, c_int) -> *mut c_char;
pub type rl_completion_func_t =
    unsafe extern "C" fn(*const c_char, c_int, c_int) -> *mut *mut c_char;
pub type rl_compignore_func_t = unsafe extern "C" fn(*mut *mut c_char) -> c_int;
pub type rl_cpvfunc_t = unsafe extern "C" fn() -> *mut c_char;
pub type rl_compdisp_func_t = unsafe extern "C" fn(*mut *mut c_char, c_int, c_int);
pub type rl_getc_func_t = unsafe extern "C" fn(*mut libc::FILE) -> c_int;
pub type Keymap = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KEYMAP_ENTRY {
    pub r#type: c_char,
    pub function: *mut c_void,
}

const EMPTY_KEYMAP_ENTRY: KEYMAP_ENTRY = KEYMAP_ENTRY {
    r#type: 0,
    function: ptr::null_mut(),
};

#[no_mangle]
pub static mut emacs_standard_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut emacs_meta_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut emacs_ctlx_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut vi_insertion_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];
#[no_mangle]
pub static mut vi_movement_keymap: [KEYMAP_ENTRY; 257] = [EMPTY_KEYMAP_ENTRY; 257];

pub type tilde_hook_func_t = unsafe extern "C" fn(*mut c_char) -> *mut c_char;

#[no_mangle]
pub static mut tilde_expansion_preexpansion_hook: Option<tilde_hook_func_t> = None;
#[no_mangle]
pub static mut tilde_expansion_failure_hook: Option<tilde_hook_func_t> = None;
#[no_mangle]
pub static mut tilde_additional_prefixes: *mut *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut tilde_additional_suffixes: *mut *mut c_char = ptr::null_mut();

#[repr(C)]
pub struct UNDO_LIST {
    pub next: *mut UNDO_LIST,
    pub start: c_int,
    pub end: c_int,
    pub text: *mut c_char,
    pub what: c_int,
}

#[no_mangle]
pub static mut rl_undo_list: *mut UNDO_LIST = ptr::null_mut();
#[no_mangle]
pub static mut funmap: *mut *mut FUNMAP = ptr::null_mut();

#[repr(C)]
pub struct FUNMAP {
    pub name: *const c_char,
    pub function: Option<rl_command_func_t>,
}

#[repr(C)]
pub struct HIST_ENTRY {
    pub line: *mut c_char,
    pub timestamp: *mut c_char,
    pub data: histdata_t,
}

#[repr(C)]
pub struct HISTORY_STATE {
    pub entries: *mut *mut HIST_ENTRY,
    pub offset: c_int,
    pub length: c_int,
    pub size: c_int,
    pub flags: c_int,
}

#[repr(C)]
pub struct READLINE_STATE {
    pub point: c_int,
    pub end: c_int,
    pub mark: c_int,
    pub buflen: c_int,
    pub buffer: *mut c_char,
    pub undo: *mut UNDO_LIST,
    pub prompt: *mut c_char,
    pub state: c_int,
    pub done: c_int,
    pub keymap: Keymap,
    pub last_function: Option<rl_command_func_t>,
    pub insert_mode: c_int,
    pub editing_mode: c_int,
    pub key_sequence: *mut c_char,
    pub key_sequence_length: c_int,
    pub pending_input: c_int,
    pub input: *mut libc::FILE,
    pub output: *mut libc::FILE,
    pub macro_text: *mut c_char,
    pub catch_signals: c_int,
    pub catch_sigwinch: c_int,
    pub completion_entry: Option<rl_compentry_func_t>,
    pub menu_completion_entry: Option<rl_compentry_func_t>,
    pub ignore_completions: Option<rl_compignore_func_t>,
    pub attempted_completion: Option<rl_completion_func_t>,
    pub word_break_characters: *const c_char,
    pub reserved: [c_char; 64],
}

const VERSION: &[u8] = b"8.3\0";
const WORD_BREAKS: &[u8] = b" \t\n\"\\'`@$><=;|&{(\0";
const QUOTES: &[u8] = b"\"'\0";
const FILENAME_QUOTES: &[u8] = b" \t\n\\\"'@<>=;|&()#$`?*[!:{~\0";
const HISTORY_DELIMITERS: &[u8] = b" \t\n;&()|<>\0";
const HISTORY_NO_EXPAND: &[u8] = b" \t\n=\0";

#[no_mangle]
pub static mut history_base: c_int = 1;
#[no_mangle]
pub static mut history_length: c_int = 0;
#[no_mangle]
pub static mut history_max_entries: c_int = 0;
#[no_mangle]
pub static mut max_input_history: c_int = 0;
#[no_mangle]
pub static mut history_offset: c_int = 0;
#[no_mangle]
pub static mut history_lines_read_from_file: c_int = 0;
#[no_mangle]
pub static mut history_lines_written_to_file: c_int = 0;
#[no_mangle]
pub static mut history_expansion_char: c_char = b'!' as c_char;
#[no_mangle]
pub static mut history_subst_char: c_char = b'^' as c_char;
#[no_mangle]
pub static mut history_comment_char: c_char = b'#' as c_char;
#[no_mangle]
pub static mut history_word_delimiters: *mut c_char = HISTORY_DELIMITERS.as_ptr() as *mut c_char;
#[no_mangle]
pub static mut history_no_expand_chars: *mut c_char = HISTORY_NO_EXPAND.as_ptr() as *mut c_char;
#[no_mangle]
pub static mut history_search_delimiter_chars: *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut history_quotes_inhibit_expansion: c_int = 0;
#[no_mangle]
pub static mut history_quoting_state: c_int = 0;
#[no_mangle]
pub static mut history_write_timestamps: c_int = 0;
#[no_mangle]
pub static mut history_multiline_entries: c_int = 0;
#[no_mangle]
pub static mut history_file_version: c_int = 2;
#[no_mangle]
pub static mut history_inhibit_expansion_function: Option<
    unsafe extern "C" fn(*mut c_char, c_int) -> c_int,
> = None;

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
        history_comment_char as u8 as char,
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
    let marker = history_comment_char as u8 as char;
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
    let comment = unsafe { history_comment_char as u8 };
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

#[no_mangle]
pub static mut rl_library_version: *const c_char = VERSION.as_ptr().cast();
#[no_mangle]
pub static mut rl_readline_version: c_int = 0x0803;
#[no_mangle]
pub static mut rl_gnu_readline_p: c_int = 1;
#[no_mangle]
pub static mut rl_readline_state: c_ulong = 0;
#[no_mangle]
pub static mut rl_editing_mode: c_int = 1;
#[no_mangle]
pub static mut rl_insert_mode: c_int = 1;
#[no_mangle]
pub static mut rl_readline_name: *const c_char = c"other".as_ptr();
#[no_mangle]
pub static mut rl_prompt: *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut rl_display_prompt: *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut rl_line_buffer: *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut rl_point: c_int = 0;
#[no_mangle]
pub static mut rl_end: c_int = 0;
#[no_mangle]
pub static mut rl_mark: c_int = 0;
#[no_mangle]
pub static mut rl_done: c_int = 0;
#[no_mangle]
pub static mut rl_eof_found: c_int = 0;
#[no_mangle]
pub static mut rl_pending_input: c_int = 0;
#[no_mangle]
pub static mut rl_dispatching: c_int = 0;
#[no_mangle]
pub static mut rl_explicit_arg: c_int = 0;
#[no_mangle]
pub static mut rl_numeric_arg: c_int = 1;
#[no_mangle]
pub static mut rl_last_func: Option<rl_command_func_t> = None;
#[no_mangle]
pub static mut rl_terminal_name: *const c_char = ptr::null();
#[no_mangle]
pub static mut rl_instream: *mut libc::FILE = ptr::null_mut();
#[no_mangle]
pub static mut rl_outstream: *mut libc::FILE = ptr::null_mut();
#[no_mangle]
pub static mut rl_prefer_env_winsize: c_int = 0;
#[no_mangle]
pub static mut rl_startup_hook: Option<rl_hook_func_t> = None;
#[no_mangle]
pub static mut rl_pre_input_hook: Option<rl_hook_func_t> = None;
#[no_mangle]
pub static mut rl_event_hook: Option<rl_hook_func_t> = None;
#[no_mangle]
pub static mut rl_signal_event_hook: Option<rl_hook_func_t> = None;
#[no_mangle]
pub static mut rl_timeout_event_hook: Option<rl_hook_func_t> = None;
#[no_mangle]
pub static mut rl_input_available_hook: Option<rl_hook_func_t> = None;
#[no_mangle]
pub static mut rl_getc_function: Option<rl_getc_func_t> = Some(rl_getc);
#[no_mangle]
pub static mut rl_redisplay_function: Option<unsafe extern "C" fn()> = None;
#[no_mangle]
pub static mut rl_prep_term_function: Option<unsafe extern "C" fn(c_int)> = None;
#[no_mangle]
pub static mut rl_deprep_term_function: Option<unsafe extern "C" fn()> = None;
#[no_mangle]
pub static mut rl_macro_display_hook: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_executing_keymap: Keymap = ptr::null_mut();
#[no_mangle]
pub static mut rl_binding_keymap: Keymap = ptr::null_mut();
#[no_mangle]
pub static mut rl_executing_key: c_int = 0;
#[no_mangle]
pub static mut rl_executing_keyseq: *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut rl_key_sequence_length: c_int = 0;
#[no_mangle]
pub static mut rl_erase_empty_line: c_int = 0;
#[no_mangle]
pub static mut rl_already_prompted: c_int = 0;
#[no_mangle]
pub static mut rl_num_chars_to_read: c_int = 0;
#[no_mangle]
pub static mut rl_executing_macro: *mut c_char = ptr::null_mut();
#[no_mangle]
pub static mut rl_catch_signals: c_int = 1;
#[no_mangle]
pub static mut rl_catch_sigwinch: c_int = 1;
#[no_mangle]
pub static mut rl_change_environment: c_int = 1;
#[no_mangle]
pub static mut rl_completion_entry_function: Option<rl_compentry_func_t> = None;
#[no_mangle]
pub static mut rl_menu_completion_entry_function: Option<rl_compentry_func_t> = None;
#[no_mangle]
pub static mut rl_ignore_some_completions_function: Option<rl_compignore_func_t> = None;
#[no_mangle]
pub static mut rl_attempted_completion_function: Option<rl_completion_func_t> = None;
#[no_mangle]
pub static mut rl_basic_word_break_characters: *const c_char = WORD_BREAKS.as_ptr().cast();
#[no_mangle]
pub static mut rl_completer_word_break_characters: *const c_char = ptr::null();
#[no_mangle]
pub static mut rl_completion_word_break_hook: Option<rl_cpvfunc_t> = None;
#[no_mangle]
pub static mut rl_completer_quote_characters: *const c_char = QUOTES.as_ptr().cast();
#[no_mangle]
pub static mut rl_basic_quote_characters: *const c_char = QUOTES.as_ptr().cast();
#[no_mangle]
pub static mut rl_filename_quote_characters: *const c_char = FILENAME_QUOTES.as_ptr().cast();
#[no_mangle]
pub static mut rl_special_prefixes: *const c_char = ptr::null();
#[no_mangle]
pub static mut rl_directory_completion_hook: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_directory_rewrite_hook: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_filename_stat_hook: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_filename_rewrite_hook: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_completion_rewrite_hook: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_completion_display_matches_hook: Option<rl_compdisp_func_t> = None;
#[no_mangle]
pub static mut rl_filename_completion_desired: c_int = 0;
#[no_mangle]
pub static mut rl_filename_quoting_desired: c_int = 1;
#[no_mangle]
pub static mut rl_full_quoting_desired: c_int = 0;
#[no_mangle]
pub static mut rl_filename_quoting_function: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_filename_dequoting_function: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_char_is_quoted_p: *mut c_void = ptr::null_mut();
#[no_mangle]
pub static mut rl_attempted_completion_over: c_int = 0;
#[no_mangle]
pub static mut rl_completion_type: c_int = b'\t' as c_int;
#[no_mangle]
pub static mut rl_completion_invoking_key: c_int = b'\t' as c_int;
#[no_mangle]
pub static mut rl_completion_query_items: c_int = 100;
#[no_mangle]
pub static mut rl_completion_append_character: c_int = b' ' as c_int;
#[no_mangle]
pub static mut rl_completion_suppress_append: c_int = 0;
#[no_mangle]
pub static mut rl_completion_quote_character: c_int = 0;
#[no_mangle]
pub static mut rl_completion_found_quote: c_int = 0;
#[no_mangle]
pub static mut rl_completion_suppress_quote: c_int = 0;
#[no_mangle]
pub static mut rl_sort_completion_matches: c_int = 1;
#[no_mangle]
pub static mut rl_completion_mark_symlink_dirs: c_int = 0;
#[no_mangle]
pub static mut rl_ignore_completion_duplicates: c_int = 1;
#[no_mangle]
pub static mut rl_inhibit_completion: c_int = 0;
#[no_mangle]
pub static mut rl_persistent_signal_handlers: c_int = 0;

const RL_STATE_INITIALIZED: c_ulong = 0x00000002;
const RL_STATE_DISPATCHING: c_ulong = 0x00000020;
const RL_STATE_OVERWRITE: c_ulong = 0x00002000;
const RL_STATE_CALLBACK: c_ulong = 0x00080000;
const RL_STATE_TIMEOUT: c_ulong = 0x04000000;
const RL_STATE_EOF: c_ulong = 0x08000000;

static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn readline_signal_handler(signal: c_int) {
    PENDING_SIGNAL.store(signal, Ordering::SeqCst);
}

struct ReadlineStore {
    editor: Option<LineEditor>,
    line_allocation: usize,
    line_capacity: usize,
    prompt_allocation: usize,
    callback: Option<rl_vcpfunc_t>,
    callback_prompt: String,
    callback_raw_mode: Option<RawMode>,
    callback_prepped: bool,
    callback_buffer: Vec<u8>,
    variables: std::collections::BTreeMap<String, CString>,
    kill_ring: Vec<String>,
    undo: Vec<(String, usize)>,
    filename_generator: Option<(String, Vec<String>, usize)>,
    current_keymap: usize,
    keymap_names: std::collections::BTreeMap<String, usize>,
    saved_line: Option<(String, usize, usize)>,
    mark_active: bool,
    keep_mark_active: bool,
    saved_prompt: Option<String>,
    keyboard_timeout_us: c_int,
    timeout_duration: Option<Duration>,
    timeout_deadline: Option<Instant>,
    macro_recording: bool,
    current_macro: String,
    last_macro: String,
    last_init_file: Option<PathBuf>,
    last_search: Option<(String, bool)>,
    last_vi_operator: Option<(c_int, c_int)>,
    funmap_entries: Vec<usize>,
    funmap_functions: std::collections::BTreeMap<String, usize>,
    funmap_initialized: bool,
    funmap_array: usize,
    signal_handlers: std::collections::BTreeMap<c_int, usize>,
    paren_blink_timeout_us: c_int,
    tty_echoing: c_int,
}

impl ReadlineStore {
    fn new() -> Self {
        let mut keymap = RustKeymap::new("emacs");
        keymap.install_emacs_defaults();
        Self {
            editor: Some(LineEditor::new(keymap)),
            line_allocation: 0,
            line_capacity: 0,
            prompt_allocation: 0,
            callback: None,
            callback_prompt: String::new(),
            callback_raw_mode: None,
            callback_prepped: false,
            callback_buffer: Vec::new(),
            variables: std::collections::BTreeMap::new(),
            kill_ring: Vec::new(),
            undo: Vec::new(),
            filename_generator: None,
            current_keymap: 0,
            keymap_names: std::collections::BTreeMap::new(),
            saved_line: None,
            mark_active: false,
            keep_mark_active: false,
            saved_prompt: None,
            keyboard_timeout_us: 100_000,
            timeout_duration: None,
            timeout_deadline: None,
            macro_recording: false,
            current_macro: String::new(),
            last_macro: String::new(),
            last_init_file: None,
            last_search: None,
            last_vi_operator: None,
            funmap_entries: Vec::new(),
            funmap_functions: std::collections::BTreeMap::new(),
            funmap_initialized: false,
            funmap_array: 0,
            signal_handlers: std::collections::BTreeMap::new(),
            paren_blink_timeout_us: 500_000,
            tty_echoing: 1,
        }
    }
}

fn readline_store() -> &'static Mutex<ReadlineStore> {
    static STORE: OnceLock<Mutex<ReadlineStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(ReadlineStore::new()))
}

unsafe fn set_line_buffer(line: &str, point: usize) {
    let mut store = readline_store().lock().expect("readline lock");
    if store.line_allocation != 0 {
        libc::free(store.line_allocation as *mut c_void);
    }
    let allocation = malloc_string(line);
    store.line_allocation = allocation as usize;
    store.line_capacity = line.len().saturating_add(1);
    rl_line_buffer = allocation;
    rl_end = line.len().min(c_int::MAX as usize) as c_int;
    rl_point = point.min(line.len()).min(c_int::MAX as usize) as c_int;
}

unsafe fn current_line() -> String {
    c_text(rl_line_buffer).unwrap_or_default()
}

unsafe fn line_snapshot() -> (String, usize) {
    (current_line(), rl_point.max(0) as usize)
}

unsafe fn save_undo() {
    let snapshot = line_snapshot();
    let mut store = readline_store().lock().expect("readline lock");
    store.undo.push(snapshot);
    if store.undo.len() > 256 {
        store.undo.remove(0);
    }
}

fn clamp_boundary(text: &str, mut point: usize) -> usize {
    point = point.min(text.len());
    while point > 0 && !text.is_char_boundary(point) {
        point -= 1;
    }
    point
}

struct FfiHistory {
    entries: Vec<String>,
}

impl FfiHistory {
    fn snapshot() -> Self {
        let store = history_store().lock().expect("history lock");
        Self {
            entries: store
                .entries
                .iter()
                .map(|entry| unsafe {
                    c_text((*entry as *mut HIST_ENTRY).as_ref().unwrap().line).unwrap_or_default()
                })
                .collect(),
        }
    }
}

impl HistoryProvider for FfiHistory {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get(&self, index: usize) -> Option<String> {
        self.entries.get(index).cloned()
    }
}

struct FfiCompleter;

impl CompletionProvider for FfiCompleter {
    fn complete(&mut self, line: &str, point: usize) -> Completion {
        unsafe { ffi_complete(line, point) }
    }

    fn run_shell_command(
        &mut self,
        command: &str,
        line: &str,
        point: usize,
    ) -> Option<(String, usize)> {
        let payload = command.strip_prefix("readline-function:")?;
        let (address, key) = payload.split_once(':')?;
        let address = address.parse::<usize>().ok()?;
        let key = key.parse::<c_int>().ok()?;
        unsafe {
            set_line_buffer(line, point);
            let function = std::mem::transmute::<usize, rl_command_func_t>(address);
            rl_dispatching = 1;
            rl_readline_state |= RL_STATE_DISPATCHING;
            let status = function(1, key);
            rl_readline_state &= !RL_STATE_DISPATCHING;
            rl_dispatching = 0;
            rl_last_func = Some(function);
            if status < 0 {
                return None;
            }
            Some((current_line(), rl_point.max(0) as usize))
        }
    }
}

unsafe fn ffi_complete(line: &str, point: usize) -> Completion {
    if rl_inhibit_completion != 0 {
        return Completion::default();
    }
    let breaks = c_text(if rl_completer_word_break_characters.is_null() {
        rl_basic_word_break_characters
    } else {
        rl_completer_word_break_characters
    })
    .unwrap_or_else(|| " \t\n".to_string());
    let point = point.min(line.len());
    let start = line[..point]
        .char_indices()
        .rev()
        .find_map(|(index, ch)| breaks.contains(ch).then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let text = &line[start..point];
    let mut matches = if let Some(callback) = rl_attempted_completion_function {
        let text_c = clean_c_string(text);
        let array = callback(text_c.as_ptr(), start as c_int, point as c_int);
        take_completion_array(array, true)
    } else if let Some(generator) = rl_completion_entry_function {
        run_generator(generator, text)
    } else {
        filename_matches(text)
    };
    if rl_sort_completion_matches != 0 {
        matches.sort_by(|left, right| {
            let left = clean_c_string(left);
            let right = clean_c_string(right);
            libc::strcoll(left.as_ptr(), right.as_ptr()).cmp(&0)
        });
    }
    if rl_ignore_completion_duplicates != 0 {
        matches.dedup();
    }
    Completion {
        matches,
        replace_start: start,
        suppress_append: rl_completion_suppress_append != 0,
        append_character: char::from_u32(rl_completion_append_character.max(0) as u32),
        filenames: rl_filename_completion_desired != 0,
    }
}

unsafe fn take_completion_array(array: *mut *mut c_char, skip_common: bool) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    let mut values = Vec::new();
    let mut index = 0usize;
    while !(*array.add(index)).is_null() {
        values.push(c_text(*array.add(index)).unwrap_or_default());
        libc::free((*array.add(index)).cast());
        index += 1;
    }
    libc::free(array.cast());
    if skip_common && values.len() > 1 {
        values.remove(0);
    }
    values
}

unsafe fn run_generator(generator: rl_compentry_func_t, text: &str) -> Vec<String> {
    let text = clean_c_string(text);
    let mut matches = Vec::new();
    for state in 0..100_000 {
        let value = generator(text.as_ptr(), state);
        if value.is_null() {
            break;
        }
        matches.push(c_text(value).unwrap_or_default());
        libc::free(value.cast());
    }
    matches
}

fn filename_matches(text: &str) -> Vec<String> {
    unsafe { rl_filename_completion_desired = 1 };
    let path = PathBuf::from(text);
    let (directory, prefix) = if text.ends_with('/') {
        (path, String::new())
    } else {
        (
            path.parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    let shown_directory = if text.ends_with('/') {
        text.to_string()
    } else {
        text.rsplit_once('/')
            .map_or(String::new(), |(head, _)| format!("{head}/"))
    };
    let mut matches = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && (prefix.starts_with('.') || !name.starts_with('.')) {
                let mut candidate = format!("{shown_directory}{name}");
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    candidate.push('/');
                }
                matches.push(candidate);
            }
        }
    }
    matches
}

fn command_pointer(function: rl_command_func_t) -> *mut c_void {
    function as *const () as *mut c_void
}

fn lineedit_sequence(sequence: &[u8]) -> String {
    let mut rendered = String::new();
    let mut index = 0;
    while index < sequence.len() {
        let key = sequence[index];
        if key == 0x1b && index + 1 < sequence.len() {
            let next = sequence[index + 1];
            if !matches!(next, b'[' | b'O' | 0x1b) && (0x20..=0x7e).contains(&next) {
                rendered.push_str("\\M-");
                rendered.push(next as char);
                index += 2;
                continue;
            }
        }
        rendered.push_str(
            match key {
                b'\t' => "\\t".to_string(),
                b'\n' => "\\C-j".to_string(),
                b'\r' => "\\C-m".to_string(),
                0x08 | 0x7f => "\\C-h".to_string(),
                0x1b => "\\e".to_string(),
                0..=0x1f => format!("\\C-{}", ((key | 0x40) as char).to_ascii_lowercase()),
                b'\\' => "\\\\".to_string(),
                _ => (key as char).to_string(),
            }
            .as_str(),
        );
        index += 1;
    }
    rendered
}

fn edit_action_for_function(function: rl_command_func_t) -> Option<EditAction> {
    let address = command_pointer(function);
    let mappings = [
        (command_pointer(rl_insert), EditAction::SelfInsert),
        (command_pointer(rl_beg_of_line), EditAction::BeginningOfLine),
        (command_pointer(rl_end_of_line), EditAction::EndOfLine),
        (command_pointer(rl_forward_char), EditAction::ForwardChar),
        (command_pointer(rl_backward_char), EditAction::BackwardChar),
        (command_pointer(rl_forward_word), EditAction::ForwardWord),
        (command_pointer(rl_backward_word), EditAction::BackwardWord),
        (command_pointer(rl_delete), EditAction::DeleteChar),
        (command_pointer(rl_rubout), EditAction::BackwardDeleteChar),
        (command_pointer(rl_kill_line), EditAction::KillLine),
        (
            command_pointer(rl_backward_kill_line),
            EditAction::BackwardKillLine,
        ),
        (command_pointer(rl_kill_word), EditAction::KillWord),
        (
            command_pointer(rl_backward_kill_word),
            EditAction::BackwardKillWord,
        ),
        (command_pointer(rl_kill_region), EditAction::KillRegion),
        (command_pointer(rl_yank), EditAction::Yank),
        (command_pointer(rl_yank_pop), EditAction::YankPop),
        (command_pointer(rl_yank_last_arg), EditAction::YankLastArg),
        (command_pointer(rl_yank_nth_arg), EditAction::YankNthArg),
        (
            command_pointer(rl_get_previous_history),
            EditAction::PreviousHistory,
        ),
        (
            command_pointer(rl_get_next_history),
            EditAction::NextHistory,
        ),
        (
            command_pointer(rl_beginning_of_history),
            EditAction::BeginningOfHistory,
        ),
        (command_pointer(rl_end_of_history), EditAction::EndOfHistory),
        (
            command_pointer(rl_reverse_search_history),
            EditAction::ReverseSearchHistory,
        ),
        (
            command_pointer(rl_forward_search_history),
            EditAction::ForwardSearchHistory,
        ),
        (command_pointer(rl_newline), EditAction::AcceptLine),
        (command_pointer(rl_complete), EditAction::Complete),
        (
            command_pointer(rl_possible_completions),
            EditAction::PossibleCompletions,
        ),
        (
            command_pointer(rl_insert_completions),
            EditAction::InsertCompletions,
        ),
        (command_pointer(rl_undo_command), EditAction::UndoCmd),
        (command_pointer(rl_clear_screen), EditAction::ClearScreen),
        (command_pointer(rl_refresh_line), EditAction::Redraw),
        (command_pointer(rl_abort), EditAction::Abort),
        (command_pointer(rl_tilde_expand), EditAction::Tilde),
        (
            command_pointer(rl_vi_movement_mode),
            EditAction::ViMovementMode,
        ),
        (
            command_pointer(rl_vi_insertion_mode),
            EditAction::ViInsertionMode,
        ),
        (command_pointer(rl_vi_append_mode), EditAction::ViAppendMode),
        (command_pointer(rl_vi_append_eol), EditAction::ViAppendEol),
    ];
    mappings
        .into_iter()
        .find_map(|(candidate, action)| (candidate == address).then_some(action))
}

fn editor_action_for_function(
    keymap: &mut RustKeymap,
    function: rl_command_func_t,
    key: u8,
) -> EditAction {
    edit_action_for_function(function).unwrap_or_else(|| {
        let index = keymap.shell_commands.len() as u32;
        keymap.shell_commands.push(format!(
            "readline-function:{}:{key}",
            command_pointer(function) as usize
        ));
        EditAction::ShellCommand(index)
    })
}

unsafe fn collect_editor_bindings(
    map: Keymap,
    prefix: &mut Vec<u8>,
    path: &mut Vec<usize>,
    output: &mut RustKeymap,
) {
    if map.is_null() || path.len() >= 64 || path.contains(&(map as usize)) {
        return;
    }
    path.push(map as usize);
    for key in 0u16..=255 {
        let entry = &*map.cast::<KEYMAP_ENTRY>().add(key as usize);
        if entry.function.is_null() {
            continue;
        }
        prefix.push(key as u8);
        match entry.r#type {
            0 => {
                let function =
                    std::mem::transmute::<*mut c_void, rl_command_func_t>(entry.function);
                let action = editor_action_for_function(output, function, key as u8);
                output.bind(lineedit_sequence(prefix), action);
            }
            1 => collect_editor_bindings(entry.function, prefix, path, output),
            2 => {
                let index = output.macros.len() as u32;
                output
                    .macros
                    .push(c_text(entry.function.cast()).unwrap_or_default());
                output.bind(lineedit_sequence(prefix), EditAction::Macro(index));
            }
            _ => {}
        }
        prefix.pop();
    }
    path.pop();
}

unsafe fn editor_keymap_from_c(keymap: Keymap) -> RustKeymap {
    let name = readline_store()
        .lock()
        .expect("readline lock")
        .keymap_names
        .iter()
        .find_map(|(name, address)| (*address == keymap as usize).then(|| name.clone()))
        .unwrap_or_else(|| "readline".to_string());
    let mut output = RustKeymap::new(name);
    let emacs = (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_insert = (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_command = (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    if keymap == emacs {
        output.install_emacs_defaults();
    } else if keymap == vi_insert {
        output.install_vi_insert_defaults();
    } else if keymap == vi_command {
        output.install_vi_movement_defaults();
    }
    collect_editor_bindings(keymap, &mut Vec::new(), &mut Vec::new(), &mut output);
    output
}

unsafe fn sync_editor_function_binding(
    keymap: Keymap,
    sequence: &[u8],
    function: Option<rl_command_func_t>,
) {
    let mut store = readline_store().lock().expect("readline lock");
    if store.current_keymap != keymap as usize {
        return;
    }
    let Some(editor) = store.editor.as_mut() else {
        return;
    };
    let sequence_text = lineedit_sequence(sequence);
    let Some(function) = function else {
        editor.keymap.unbind(&sequence_text);
        return;
    };
    let key = sequence.last().copied().unwrap_or_default();
    let action = editor_action_for_function(&mut editor.keymap, function, key);
    editor.keymap.bind(sequence_text, action);
}

unsafe fn sync_editor_macro_binding(keymap: Keymap, sequence: &[u8], text: &str) {
    let mut store = readline_store().lock().expect("readline lock");
    if store.current_keymap != keymap as usize {
        return;
    }
    let Some(editor) = store.editor.as_mut() else {
        return;
    };
    let index = editor.keymap.macros.len() as u32;
    editor.keymap.macros.push(text.to_string());
    editor
        .keymap
        .bind(lineedit_sequence(sequence), EditAction::Macro(index));
}

fn initialize_keymaps() {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    INITIALIZED.get_or_init(|| unsafe {
        for key in 32usize..=126 {
            emacs_standard_keymap[key] = KEYMAP_ENTRY {
                r#type: 0,
                function: command_pointer(rl_insert),
            };
            vi_insertion_keymap[key] = emacs_standard_keymap[key];
        }
        emacs_standard_keymap[1].function = command_pointer(rl_beg_of_line);
        emacs_standard_keymap[2].function = command_pointer(rl_backward_char);
        emacs_standard_keymap[4].function = command_pointer(rl_delete);
        emacs_standard_keymap[5].function = command_pointer(rl_end_of_line);
        emacs_standard_keymap[6].function = command_pointer(rl_forward_char);
        emacs_standard_keymap[8].function = command_pointer(rl_rubout);
        emacs_standard_keymap[9].function = command_pointer(rl_complete);
        emacs_standard_keymap[10].function = command_pointer(rl_newline);
        emacs_standard_keymap[11].function = command_pointer(rl_kill_line);
        emacs_standard_keymap[13].function = command_pointer(rl_newline);
        emacs_standard_keymap[21].function = command_pointer(rl_unix_line_discard);
        emacs_standard_keymap[23].function = command_pointer(rl_unix_word_rubout);
        emacs_standard_keymap[25].function = command_pointer(rl_yank);
        emacs_standard_keymap[24] = KEYMAP_ENTRY {
            r#type: 1,
            function: (&raw mut emacs_ctlx_keymap).cast::<KEYMAP_ENTRY>().cast(),
        };
        emacs_standard_keymap[27] = KEYMAP_ENTRY {
            r#type: 1,
            function: (&raw mut emacs_meta_keymap).cast::<KEYMAP_ENTRY>().cast(),
        };
        emacs_meta_keymap[b'b' as usize].function = command_pointer(rl_backward_word);
        emacs_meta_keymap[b'f' as usize].function = command_pointer(rl_forward_word);
        emacs_meta_keymap[b'd' as usize].function = command_pointer(rl_kill_word);
        vi_insertion_keymap[27].function = command_pointer(rl_vi_movement_mode);
        vi_movement_keymap[b'h' as usize].function = command_pointer(rl_backward_char);
        vi_movement_keymap[b'l' as usize].function = command_pointer(rl_forward_char);
        vi_movement_keymap[b'i' as usize].function = command_pointer(rl_vi_insertion_mode);
        vi_movement_keymap[b'x' as usize].function = command_pointer(rl_delete);

        let emacs = (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let emacs_meta = (&raw mut emacs_meta_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let emacs_ctlx = (&raw mut emacs_ctlx_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let vi_insert = (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let vi_command = (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
        let mut store = readline_store().lock().expect("readline lock");
        store.current_keymap = emacs as usize;
        for name in ["emacs", "emacs-standard"] {
            store.keymap_names.insert(name.to_string(), emacs as usize);
        }
        store
            .keymap_names
            .insert("emacs-meta".to_string(), emacs_meta as usize);
        store
            .keymap_names
            .insert("emacs-ctlx".to_string(), emacs_ctlx as usize);
        for name in ["vi", "vi-insertion", "vi-insert"] {
            store
                .keymap_names
                .insert(name.to_string(), vi_insert as usize);
        }
        for name in ["vi-move", "vi-command", "vi-movement"] {
            store
                .keymap_names
                .insert(name.to_string(), vi_command as usize);
        }
        rl_executing_keymap = emacs;
        rl_binding_keymap = emacs;
    });
}

#[no_mangle]
pub unsafe extern "C" fn rl_initialize() -> c_int {
    initialize_keymaps();
    rl_initialize_funmap();
    let _ = readline_store();
    let _ = history_store();
    if rl_line_buffer.is_null() {
        set_line_buffer("", 0);
    }
    if rl_instream.is_null() {
        rl_instream = C_STDIN;
    }
    if rl_outstream.is_null() {
        rl_outstream = C_STDOUT;
    }
    rl_readline_state |= RL_STATE_INITIALIZED;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_prompt(prompt: *const c_char) -> c_int {
    let text = c_text(prompt).unwrap_or_default();
    let allocation = malloc_string(&text);
    if allocation.is_null() {
        return -1;
    }
    let mut store = readline_store().lock().expect("readline lock");
    if store.prompt_allocation != 0 {
        libc::free(store.prompt_allocation as *mut c_void);
    }
    store.prompt_allocation = allocation as usize;
    rl_prompt = allocation;
    rl_display_prompt = allocation;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_expand_prompt(prompt: *mut c_char) -> c_int {
    let text = c_text(prompt).unwrap_or_default();
    let mut visible = 0usize;
    let mut ignored = false;
    for ch in text.chars() {
        match ch {
            '\u{1}' => ignored = true,
            '\u{2}' => ignored = false,
            _ if !ignored => visible += 1,
            _ => {}
        }
    }
    visible.min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_replace_line(text: *const c_char, clear_undo: c_int) {
    let text = c_text(text).unwrap_or_default();
    if clear_undo == 0 {
        save_undo();
    } else {
        readline_store().lock().expect("readline lock").undo.clear();
    }
    set_line_buffer(&text, text.len());
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert_text(text: *const c_char) -> c_int {
    let Some(text) = c_text(text) else { return 0 };
    save_undo();
    let (mut line, point) = line_snapshot();
    let point = clamp_boundary(&line, point);
    line.insert_str(point, &text);
    set_line_buffer(&line, point + text.len());
    text.len().min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_delete_text(start: c_int, end: c_int) -> c_int {
    let mut line = current_line();
    let start = clamp_boundary(&line, start.max(0) as usize);
    let end = clamp_boundary(&line, end.max(0) as usize).max(start);
    if start == end {
        return 0;
    }
    save_undo();
    line.replace_range(start..end, "");
    set_line_buffer(&line, start.min(line.len()));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_text(start: c_int, end: c_int) -> c_int {
    let line = current_line();
    let start = clamp_boundary(&line, start.max(0) as usize);
    let end = clamp_boundary(&line, end.max(0) as usize).max(start);
    if start < end {
        readline_store()
            .lock()
            .expect("readline lock")
            .kill_ring
            .push(line[start..end].to_string());
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert(count: c_int, key: c_int) -> c_int {
    let Some(ch) = char::from_u32(key as u32) else {
        return 1;
    };
    let text: String = std::iter::repeat_n(ch, count.max(1) as usize).collect();
    {
        let mut store = readline_store().lock().expect("readline lock");
        if store.macro_recording {
            store.current_macro.push_str(&text);
        }
    }
    if rl_insert_mode == 0 {
        let line = current_line();
        let start = clamp_boundary(&line, rl_point.max(0) as usize);
        let mut end = start;
        for _ in 0..count.max(1) {
            end = line[end..]
                .chars()
                .next()
                .map_or(end, |character| end + character.len_utf8());
        }
        if end > start {
            rl_delete_text(start as c_int, end as c_int);
        }
    }
    let text = clean_c_string(&text);
    rl_insert_text(text.as_ptr());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_forward_char(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    for _ in 0..count.max(0) {
        point = line[point..]
            .char_indices()
            .nth(1)
            .map_or(line.len(), |(offset, _)| point + offset);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_char(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    for _ in 0..count.max(0) {
        point = line[..point]
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_beg_of_line(_count: c_int, _key: c_int) -> c_int {
    rl_point = 0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_end_of_line(_count: c_int, _key: c_int) -> c_int {
    rl_point = rl_end;
    0
}

fn word_left(text: &str, point: usize) -> usize {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut index = chars.partition_point(|(offset, _)| *offset < point);
    while index > 0 && !chars[index - 1].1.is_alphanumeric() && chars[index - 1].1 != '_' {
        index -= 1;
    }
    while index > 0 && (chars[index - 1].1.is_alphanumeric() || chars[index - 1].1 == '_') {
        index -= 1;
    }
    chars
        .get(index)
        .map_or(text.len().min(point), |(offset, _)| *offset)
}

fn word_right(text: &str, point: usize) -> usize {
    let mut point = clamp_boundary(text, point);
    while point < text.len() {
        let ch = text[point..].chars().next().unwrap();
        if ch.is_alphanumeric() || ch == '_' {
            break;
        }
        point += ch.len_utf8();
    }
    while point < text.len() {
        let ch = text[point..].chars().next().unwrap();
        if !ch.is_alphanumeric() && ch != '_' {
            break;
        }
        point += ch.len_utf8();
    }
    point
}

#[no_mangle]
pub unsafe extern "C" fn rl_forward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = rl_point.max(0) as usize;
    for _ in 0..count.max(0) {
        point = word_right(&line, point);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut point = rl_point.max(0) as usize;
    for _ in 0..count.max(0) {
        point = word_left(&line, point);
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_rubout(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let end = clamp_boundary(&line, rl_point.max(0) as usize);
    let mut start = end;
    for _ in 0..count.max(0) {
        start = line[..start]
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_delete(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = clamp_boundary(&line, rl_point.max(0) as usize);
    let mut end = start;
    for _ in 0..count.max(0) {
        if end >= line.len() {
            break;
        }
        end += line[end..].chars().next().unwrap().len_utf8();
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_line(count: c_int, _key: c_int) -> c_int {
    if count < 0 {
        rl_kill_text(0, rl_point)
    } else {
        rl_kill_text(rl_point, rl_end)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_kill_line(_count: c_int, _key: c_int) -> c_int {
    rl_kill_text(0, rl_point)
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = rl_point.max(0) as usize;
    let mut end = start;
    for _ in 0..count.max(0) {
        end = word_right(&line, end);
    }
    rl_kill_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_kill_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let end = rl_point.max(0) as usize;
    let mut start = end;
    for _ in 0..count.max(0) {
        start = word_left(&line, start);
    }
    rl_kill_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_region(_count: c_int, _key: c_int) -> c_int {
    rl_kill_text(rl_mark.min(rl_point), rl_mark.max(rl_point))
}

#[no_mangle]
pub unsafe extern "C" fn rl_yank(count: c_int, _key: c_int) -> c_int {
    let text = readline_store()
        .lock()
        .expect("readline lock")
        .kill_ring
        .last()
        .cloned();
    if let Some(text) = text {
        let repeated = text.repeat(count.max(1) as usize);
        let repeated = clean_c_string(&repeated);
        rl_insert_text(repeated.as_ptr());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_mark(_count: c_int, _key: c_int) -> c_int {
    rl_mark = rl_point;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_exchange_point_and_mark(_count: c_int, _key: c_int) -> c_int {
    ptr::swap(ptr::addr_of_mut!(rl_point), ptr::addr_of_mut!(rl_mark));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_do_undo() -> c_int {
    let previous = readline_store().lock().expect("readline lock").undo.pop();
    if let Some((line, point)) = previous {
        set_line_buffer(&line, point);
        1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_undo_command(_count: c_int, _key: c_int) -> c_int {
    (rl_do_undo() == 0) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_newline(_count: c_int, _key: c_int) -> c_int {
    rl_done = 1;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_abort(_count: c_int, _key: c_int) -> c_int {
    -1
}

#[no_mangle]
pub unsafe extern "C" fn rl_ding() -> c_int {
    let bell = [7u8];
    libc::write(libc::STDERR_FILENO, bell.as_ptr().cast(), 1);
    -1
}

#[no_mangle]
pub extern "C" fn rl_alphabetic(character: c_int) -> c_int {
    char::from_u32(character as u32).is_some_and(|ch| ch.is_alphabetic()) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_free(value: *mut c_void) {
    libc::free(value);
}

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

#[no_mangle]
pub unsafe extern "C" fn rl_callback_handler_install(
    prompt: *const c_char,
    callback: Option<rl_vcpfunc_t>,
) {
    rl_initialize();
    rl_set_prompt(prompt);
    rl_readline_state |= RL_STATE_CALLBACK;
    rl_readline_state &= !RL_STATE_EOF;
    rl_eof_found = 0;
    let prompt = c_text(prompt).unwrap_or_default();
    let prep_function = rl_prep_term_function;
    if let Some(prep) = prep_function {
        prep(1);
    }
    let raw_mode = if prep_function.is_none() && libc::isatty(libc::STDIN_FILENO) != 0 {
        RawMode::enter().ok()
    } else {
        None
    };
    let mut store = readline_store().lock().expect("readline lock");
    store.callback = callback;
    store.callback_prompt = prompt.clone();
    store.callback_raw_mode = raw_mode;
    store.callback_prepped = true;
    store.callback_buffer.clear();
    drop(store);
    set_line_buffer("", 0);
    write_callback_prompt(&prompt);
}

unsafe fn write_callback_prompt(prompt: &str) {
    if let Some(redisplay) = rl_redisplay_function {
        redisplay();
    } else {
        write_callback_bytes(prompt.as_bytes());
    }
}

unsafe fn write_callback_bytes(bytes: &[u8]) {
    let descriptor = if rl_outstream.is_null() {
        libc::STDOUT_FILENO
    } else {
        libc::fileno(rl_outstream)
    };
    libc::write(descriptor, bytes.as_ptr().cast(), bytes.len());
}

#[no_mangle]
pub unsafe extern "C" fn rl_callback_read_char() {
    let (callback, prompt) = {
        let store = readline_store().lock().expect("readline lock");
        (store.callback, store.callback_prompt.clone())
    };
    let Some(callback) = callback else { return };
    let key = rl_read_key();
    let line = if key < 0 {
        if key != libc::EOF {
            return;
        }
        rl_eof_found = 1;
        rl_readline_state |= RL_STATE_EOF;
        ptr::null_mut()
    } else {
        let keymap = rl_get_keymap();
        let entry = if keymap.is_null() || key as usize >= 257 {
            None
        } else {
            Some(*keymap.cast::<KEYMAP_ENTRY>().add(key as usize))
        };
        let function = entry
            .filter(|entry| entry.r#type == 0 && !entry.function.is_null())
            .map(|entry| std::mem::transmute::<*mut c_void, rl_command_func_t>(entry.function));
        let previous_dispatching = rl_dispatching;
        rl_dispatching = 1;
        rl_readline_state |= RL_STATE_DISPATCHING;
        if let Some(function) = function {
            function(1, key);
            rl_last_func = Some(function);
        } else if (0x20..=0x7e).contains(&key) {
            rl_insert(1, key);
            rl_last_func = Some(rl_insert);
        }
        rl_readline_state &= !RL_STATE_DISPATCHING;
        rl_dispatching = previous_dispatching;

        let redisplay_function = rl_redisplay_function;
        if key != b'\n' as c_int && key != b'\r' as c_int {
            if let Some(redisplay) = redisplay_function {
                redisplay();
            } else if (0x20..=0x7e).contains(&key) {
                write_callback_bytes(&[key as u8]);
            }
        } else if redisplay_function.is_none() {
            write_callback_bytes(b"\n");
        }
        let current = current_line();
        readline_store()
            .lock()
            .expect("readline lock")
            .callback_buffer = current.as_bytes().to_vec();
        if rl_done == 0 {
            return;
        }
        rl_done = 0;
        malloc_string(&current)
    };
    let was_prepped = {
        let mut store = readline_store().lock().expect("readline lock");
        drop(store.callback_raw_mode.take());
        std::mem::replace(&mut store.callback_prepped, false)
    };
    if was_prepped {
        if let Some(deprep) = rl_deprep_term_function {
            deprep();
        }
    }
    callback(line);
    if !rl_line_buffer.is_null() && *rl_line_buffer != 0 {
        set_line_buffer("", 0);
        readline_store()
            .lock()
            .expect("readline lock")
            .callback_buffer
            .clear();
    }
    if readline_store()
        .lock()
        .expect("readline lock")
        .callback
        .is_some()
    {
        let prep_function = rl_prep_term_function;
        if let Some(prep) = prep_function {
            prep(1);
        }
        let raw_mode = if prep_function.is_none() && libc::isatty(libc::STDIN_FILENO) != 0 {
            RawMode::enter().ok()
        } else {
            None
        };
        {
            let mut store = readline_store().lock().expect("readline lock");
            store.callback_raw_mode = raw_mode;
            store.callback_prepped = true;
        }
        let prompt_value = clean_c_string(&prompt);
        rl_set_prompt(prompt_value.as_ptr());
        write_callback_prompt(&prompt);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_callback_handler_remove() {
    let (raw_mode, was_prepped) = {
        let mut store = readline_store().lock().expect("readline lock");
        store.callback = None;
        store.callback_prompt.clear();
        store.callback_buffer.clear();
        (
            store.callback_raw_mode.take(),
            std::mem::replace(&mut store.callback_prepped, false),
        )
    };
    drop(raw_mode);
    if was_prepped {
        if let Some(deprep) = rl_deprep_term_function {
            deprep();
        }
    }
    rl_readline_state &= !RL_STATE_CALLBACK;
}

#[no_mangle]
pub extern "C" fn rl_callback_sigcleanup() {
    unsafe { rl_free_line_state() };
}

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
pub unsafe extern "C" fn rl_make_bare_keymap() -> Keymap {
    libc::calloc(257, std::mem::size_of::<KEYMAP_ENTRY>())
}

#[no_mangle]
pub unsafe extern "C" fn rl_empty_keymap(keymap: Keymap) -> c_int {
    if keymap.is_null() {
        return 0;
    }
    (0..256)
        .all(|index| {
            let entry = &*keymap.cast::<KEYMAP_ENTRY>().add(index);
            entry.r#type == 0 && entry.function.is_null()
        })
        .into()
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_keymap(keymap: Keymap) -> Keymap {
    if keymap.is_null() {
        return ptr::null_mut();
    }
    let copy = rl_make_bare_keymap();
    if !copy.is_null() {
        ptr::copy_nonoverlapping(
            keymap.cast::<KEYMAP_ENTRY>(),
            copy.cast::<KEYMAP_ENTRY>(),
            257,
        );
    }
    copy
}

#[no_mangle]
pub unsafe extern "C" fn rl_make_keymap() -> Keymap {
    let keymap = rl_make_bare_keymap();
    if !keymap.is_null() {
        for key in 32usize..=126 {
            (*keymap.cast::<KEYMAP_ENTRY>().add(key)).function = command_pointer(rl_insert);
        }
        for key in 128usize..=255 {
            (*keymap.cast::<KEYMAP_ENTRY>().add(key)).function = command_pointer(rl_insert);
        }
        (*keymap.cast::<KEYMAP_ENTRY>().add(b'\t' as usize)).function = command_pointer(rl_insert);
        (*keymap.cast::<KEYMAP_ENTRY>().add(0x7f)).function = command_pointer(rl_rubout);
        (*keymap.cast::<KEYMAP_ENTRY>().add(0x08)).function = command_pointer(rl_rubout);
    }
    keymap
}

fn static_keymap(keymap: Keymap) -> bool {
    let addresses = [
        (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut emacs_meta_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut emacs_ctlx_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
        (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap,
    ];
    addresses.contains(&keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_discard_keymap(keymap: Keymap) {
    if keymap.is_null() || static_keymap(keymap) {
        return;
    }
    for index in 0..257 {
        let entry = &mut *keymap.cast::<KEYMAP_ENTRY>().add(index);
        match entry.r#type {
            1 if !entry.function.is_null() => {
                let child = entry.function as Keymap;
                rl_discard_keymap(child);
                libc::free(child);
            }
            2 if !entry.function.is_null() => libc::free(entry.function),
            _ => {}
        }
        entry.r#type = 0;
        entry.function = ptr::null_mut();
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_free_keymap(keymap: Keymap) {
    if !keymap.is_null() && !static_keymap(keymap) {
        rl_discard_keymap(keymap);
        libc::free(keymap);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_keymap_by_name(name: *const c_char) -> Keymap {
    initialize_keymaps();
    let name = c_text(name).unwrap_or_default().to_ascii_lowercase();
    readline_store()
        .lock()
        .expect("readline lock")
        .keymap_names
        .get(&name)
        .copied()
        .unwrap_or(0) as Keymap
}

#[no_mangle]
pub extern "C" fn rl_get_keymap() -> Keymap {
    initialize_keymaps();
    readline_store()
        .lock()
        .expect("readline lock")
        .current_keymap as Keymap
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_keymap(keymap: Keymap) {
    if keymap.is_null() {
        return;
    }
    initialize_keymaps();
    let editor_keymap = editor_keymap_from_c(keymap);
    let emacs = (&raw mut emacs_standard_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_insert = (&raw mut vi_insertion_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let vi_command = (&raw mut vi_movement_keymap).cast::<KEYMAP_ENTRY>() as Keymap;
    let mut store = readline_store().lock().expect("readline lock");
    store.current_keymap = keymap as usize;
    if let Some(editor) = store.editor.as_mut() {
        if keymap == vi_command {
            editor.set_vi_command_keymap(editor_keymap);
            editor.vi_mode = true;
        } else {
            editor.keymap = editor_keymap;
            editor.vi_mode = false;
            editor.set_self_insert_unbound(keymap == emacs || keymap == vi_insert);
        }
    }
    drop(store);
    rl_executing_keymap = keymap;
    rl_binding_keymap = keymap;
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_keymap_name(name: *const c_char, keymap: Keymap) -> c_int {
    let Some(name) = c_text(name) else { return -1 };
    if keymap.is_null() {
        return -1;
    }
    initialize_keymaps();
    let name = name.to_ascii_lowercase();
    const BUILTIN_NAMES: &[&str] = &[
        "emacs",
        "emacs-standard",
        "emacs-meta",
        "emacs-ctlx",
        "vi",
        "vi-move",
        "vi-command",
        "vi-insert",
        "vi-insertion",
    ];
    if static_keymap(keymap) || BUILTIN_NAMES.contains(&name.as_str()) {
        return -1;
    }
    let mut store = readline_store().lock().expect("readline lock");
    if let Some(old_name) = store
        .keymap_names
        .iter()
        .find_map(|(old_name, address)| (*address == keymap as usize).then(|| old_name.clone()))
    {
        store.keymap_names.remove(&old_name);
    }
    store.keymap_names.insert(name, keymap as usize);
    0
}

unsafe fn bind_in_map(key: c_int, function: Option<rl_command_func_t>, keymap: Keymap) -> c_int {
    if !(0..257).contains(&key) || keymap.is_null() {
        return -1;
    }
    let entry = keymap.cast::<KEYMAP_ENTRY>().add(key as usize);
    (*entry).r#type = 0;
    (*entry).function = function.map_or(ptr::null_mut(), command_pointer);
    sync_editor_function_binding(keymap, &[key as u8], function);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key(key: c_int, function: Option<rl_command_func_t>) -> c_int {
    bind_in_map(key, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key_in_map(
    key: c_int,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    bind_in_map(key, function, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_key(key: c_int) -> c_int {
    bind_in_map(key, None, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_key_in_map(key: c_int, keymap: Keymap) -> c_int {
    bind_in_map(key, None, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key_if_unbound(
    key: c_int,
    function: Option<rl_command_func_t>,
) -> c_int {
    rl_bind_key_if_unbound_in_map(key, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_key_if_unbound_in_map(
    key: c_int,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    if !(0..257).contains(&key) || keymap.is_null() {
        return -1;
    }
    if (*keymap.cast::<KEYMAP_ENTRY>().add(key as usize))
        .function
        .is_null()
    {
        bind_in_map(key, function, keymap)
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_function_in_map(
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    if keymap.is_null() {
        return -1;
    }
    let wanted = function.map_or(ptr::null_mut(), command_pointer);
    let mut removed = false;
    for index in 0..257 {
        let entry = keymap.cast::<KEYMAP_ENTRY>().add(index);
        if (*entry).r#type == 0 && (*entry).function == wanted {
            (*entry).function = ptr::null_mut();
            removed = true;
        } else if (*entry).r#type == 1
            && !(*entry).function.is_null()
            && rl_unbind_function_in_map(function, (*entry).function) == 1
        {
            removed = true;
        }
    }
    removed as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_unbind_command_in_map(command: *const c_char, keymap: Keymap) -> c_int {
    let function = rl_named_function(command);
    function.map_or(0, |function| {
        rl_unbind_function_in_map(Some(function), keymap)
    })
}

fn translate_key_sequence(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut output = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 >= bytes.len() {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        match bytes[index + 1] {
            b'e' | b'E' => {
                output.push(0x1b);
                index += 2;
            }
            b'C' | b'c' if bytes.get(index + 2) == Some(&b'-') && index + 3 < bytes.len() => {
                output.push(bytes[index + 3].to_ascii_uppercase() & 0x1f);
                index += 4;
            }
            b'M' | b'm' if bytes.get(index + 2) == Some(&b'-') && index + 3 < bytes.len() => {
                output.push(0x1b);
                output.push(bytes[index + 3]);
                index += 4;
            }
            b'n' => {
                output.push(b'\n');
                index += 2;
            }
            b'r' => {
                output.push(b'\r');
                index += 2;
            }
            b't' => {
                output.push(b'\t');
                index += 2;
            }
            value => {
                output.push(value);
                index += 2;
            }
        }
    }
    output
}

#[no_mangle]
pub unsafe extern "C" fn rl_translate_keyseq(
    input: *const c_char,
    output: *mut c_char,
    length: *mut c_int,
) -> c_int {
    if output.is_null() || length.is_null() {
        return 1;
    }
    let translated = translate_key_sequence(&c_text(input).unwrap_or_default());
    ptr::copy_nonoverlapping(translated.as_ptr(), output.cast(), translated.len());
    *length = translated.len().min(c_int::MAX as usize) as c_int;
    0
}

unsafe fn bind_keyseq_in_map(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
    let Some(entry) = keyseq_entry(keymap, &translated, true) else {
        return -1;
    };
    if (*entry).r#type == 2 && !(*entry).function.is_null() {
        libc::free((*entry).function);
    }
    (*entry).r#type = 0;
    (*entry).function = function.map_or(ptr::null_mut(), command_pointer);
    sync_editor_function_binding(keymap, &translated, function);
    0
}

unsafe fn keyseq_entry(
    mut keymap: Keymap,
    sequence: &[u8],
    create: bool,
) -> Option<*mut KEYMAP_ENTRY> {
    let (&last, prefix) = sequence.split_last()?;
    if keymap.is_null() {
        return None;
    }
    for key in prefix {
        let entry = keymap.cast::<KEYMAP_ENTRY>().add(*key as usize);
        if (*entry).r#type == 1 && !(*entry).function.is_null() {
            keymap = (*entry).function;
            continue;
        }
        if !create {
            return None;
        }
        if (*entry).r#type == 2 && !(*entry).function.is_null() {
            libc::free((*entry).function);
        }
        let child = rl_make_bare_keymap();
        if child.is_null() {
            return None;
        }
        (*entry).r#type = 1;
        (*entry).function = child;
        keymap = child;
    }
    Some(keymap.cast::<KEYMAP_ENTRY>().add(last as usize))
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
) -> c_int {
    bind_keyseq_in_map(sequence, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq_in_map(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    bind_keyseq_in_map(sequence, function, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq_if_unbound(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
) -> c_int {
    rl_bind_keyseq_if_unbound_in_map(sequence, function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_bind_keyseq_if_unbound_in_map(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
    let bound =
        keyseq_entry(keymap, &translated, false).is_some_and(|entry| !(*entry).function.is_null());
    if bound {
        1
    } else {
        bind_keyseq_in_map(sequence, function, keymap)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_key(
    sequence: *const c_char,
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> c_int {
    rl_bind_keyseq_in_map(sequence, function, keymap)
}

#[no_mangle]
pub unsafe extern "C" fn rl_macro_bind(
    sequence: *const c_char,
    macro_text: *const c_char,
    keymap: Keymap,
) -> c_int {
    let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
    let Some(entry) = keyseq_entry(keymap, &translated, true) else {
        return -1;
    };
    if (*entry).r#type == 2 && !(*entry).function.is_null() {
        libc::free((*entry).function);
    }
    let macro_text = translate_key_sequence(&c_text(macro_text).unwrap_or_default());
    (*entry).r#type = 2;
    (*entry).function = malloc_string(&String::from_utf8_lossy(&macro_text)).cast();
    sync_editor_macro_binding(keymap, &translated, &String::from_utf8_lossy(&macro_text));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_generic_bind(
    kind: c_int,
    sequence: *const c_char,
    data: *mut c_char,
    keymap: Keymap,
) -> c_int {
    match kind {
        0 => {
            let function = (!data.is_null())
                .then(|| std::mem::transmute::<*mut c_char, rl_command_func_t>(data));
            rl_bind_keyseq_in_map(sequence, function, keymap)
        }
        1 => {
            let translated = translate_key_sequence(&c_text(sequence).unwrap_or_default());
            let Some(entry) = keyseq_entry(keymap, &translated, true) else {
                return -1;
            };
            if (*entry).r#type == 2 && !(*entry).function.is_null() {
                libc::free((*entry).function);
            }
            (*entry).r#type = 1;
            (*entry).function = data.cast();
            0
        }
        2 => rl_macro_bind(sequence, data, keymap),
        _ => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_add_defun(
    name: *const c_char,
    function: Option<rl_command_func_t>,
    key: c_int,
) -> c_int {
    if name.is_null() || function.is_none() {
        return -1;
    }
    rl_add_funmap_entry(name, function);
    if key >= 0 {
        rl_bind_key(key, function)
    } else {
        0
    }
}

macro_rules! action_alias {
    ($($name:ident => $target:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub unsafe extern "C" fn $name(count: c_int, key: c_int) -> c_int {
                $target(count, key)
            }
        )*
    };
}

action_alias! {
    rl_forward_byte => rl_forward_char,
    rl_forward => rl_forward_char,
    rl_backward_byte => rl_backward_char,
    rl_backward => rl_backward_char,
    rl_previous_screen_line => rl_backward_char,
    rl_next_screen_line => rl_forward_char,
    rl_rubout_or_delete => rl_rubout,
    rl_delete_or_show_completions => rl_delete,
    rl_unix_word_rubout => rl_backward_kill_word,
    rl_unix_filename_rubout => rl_backward_kill_word,
    rl_unix_line_discard => rl_backward_kill_line,
    rl_revert_line => rl_undo_command,
    rl_old_menu_complete => rl_complete,
    rl_menu_complete => rl_complete,
    rl_backward_menu_complete => rl_complete,
    rl_vi_complete => rl_complete,
    rl_vi_tilde_expand => rl_tilde_expand,
    rl_vi_prev_word => rl_backward_word,
    rl_vi_next_word => rl_forward_word,
    rl_vi_end_word => rl_forward_word,
    rl_vi_fWord => rl_forward_word,
    rl_vi_bWord => rl_backward_word,
    rl_vi_eWord => rl_forward_word,
    rl_vi_fword => rl_forward_word,
    rl_vi_bword => rl_backward_word,
    rl_vi_eword => rl_forward_word,
    rl_vi_rubout => rl_rubout,
    rl_vi_delete => rl_delete,
    rl_vi_unix_word_rubout => rl_backward_kill_word,
    rl_vi_undo => rl_undo_command,
    rl_vi_eof_maybe => rl_delete,
}

#[no_mangle]
pub unsafe extern "C" fn rl_quoted_insert(count: c_int, key: c_int) -> c_int {
    rl_insert(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_tab_insert(count: c_int, _key: c_int) -> c_int {
    rl_insert(count, b'\t' as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_delete_horizontal_space(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let point = rl_point.max(0) as usize;
    let mut start = point.min(line.len());
    let mut end = start;
    while start > 0 && line.as_bytes()[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    while end < line.len() && line.as_bytes()[end].is_ascii_whitespace() {
        end += 1;
    }
    rl_delete_text(start as c_int, end as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_insert_comment(_count: c_int, _key: c_int) -> c_int {
    rl_point = 0;
    let marker = clean_c_string("#");
    rl_insert_text(marker.as_ptr());
    rl_done = 1;
    0
}

unsafe fn map_current_word(mode: c_int) -> c_int {
    let mut line = current_line();
    let start = rl_point.max(0) as usize;
    let end = word_right(&line, start);
    if start >= end {
        return 0;
    }
    save_undo();
    let word = &line[start..end];
    let replacement = match mode {
        0 => word.to_uppercase(),
        1 => word.to_lowercase(),
        _ => {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                })
                .unwrap_or_default()
        }
    };
    line.replace_range(start..end, &replacement);
    set_line_buffer(&line, start + replacement.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_upcase_word(_count: c_int, _key: c_int) -> c_int {
    map_current_word(0)
}

#[no_mangle]
pub unsafe extern "C" fn rl_downcase_word(_count: c_int, _key: c_int) -> c_int {
    map_current_word(1)
}

#[no_mangle]
pub unsafe extern "C" fn rl_capitalize_word(_count: c_int, _key: c_int) -> c_int {
    map_current_word(2)
}

#[no_mangle]
pub unsafe extern "C" fn rl_transpose_chars(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let point = clamp_boundary(&line, rl_point.max(0) as usize);
    let mut chars: Vec<char> = line.chars().collect();
    if chars.len() < 2 {
        return 0;
    }
    let char_point = line[..point].chars().count();
    let right = if char_point >= chars.len() {
        chars.len() - 1
    } else {
        char_point.max(1)
    };
    save_undo();
    chars.swap(right - 1, right);
    let updated: String = chars.into_iter().collect();
    set_line_buffer(&updated, updated.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_transpose_words(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let words = history_words(&line);
    if words.len() < 2 {
        return 0;
    }
    let mut updated = words;
    let len = updated.len();
    updated.swap(len - 2, len - 1);
    let updated = updated.join(" ");
    save_undo();
    set_line_buffer(&updated, updated.len());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_kill_full_line(_count: c_int, _key: c_int) -> c_int {
    rl_kill_text(0, rl_end)
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_region_to_kill(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = rl_mark.min(rl_point).max(0) as usize;
    let end = rl_mark.max(rl_point).max(0) as usize;
    if start < end && end <= line.len() {
        readline_store()
            .lock()
            .expect("readline lock")
            .kill_ring
            .push(line[start..end].to_string());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_forward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let start = rl_point.max(0) as usize;
    let mut end = start;
    for _ in 0..count.max(0) {
        end = word_right(&line, end);
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .kill_ring
        .push(line[start..end].to_string());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_backward_word(count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let end = rl_point.max(0) as usize;
    let mut start = end;
    for _ in 0..count.max(0) {
        start = word_left(&line, start);
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .kill_ring
        .push(line[start..end].to_string());
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_beginning_of_history(_count: c_int, _key: c_int) -> c_int {
    history_set_pos(0);
    let entry = current_history();
    if !entry.is_null() {
        let line = c_text((*entry).line).unwrap_or_default();
        set_line_buffer(&line, line.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_end_of_history(_count: c_int, _key: c_int) -> c_int {
    let len = history_store().lock().expect("history lock").entries.len();
    history_set_pos(len as c_int);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_previous_history(count: c_int, _key: c_int) -> c_int {
    let mut entry = ptr::null_mut();
    for _ in 0..count.max(1) {
        let value = previous_history();
        if value.is_null() {
            break;
        }
        entry = value;
    }
    if !entry.is_null() {
        let line = c_text((*entry).line).unwrap_or_default();
        set_line_buffer(&line, line.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_next_history(count: c_int, _key: c_int) -> c_int {
    let mut entry = ptr::null_mut();
    for _ in 0..count.max(1) {
        entry = next_history();
    }
    if !entry.is_null() {
        let line = c_text((*entry).line).unwrap_or_default();
        set_line_buffer(&line, line.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_editing_mode(_count: c_int, _key: c_int) -> c_int {
    let name = clean_c_string("editing-mode");
    let value = clean_c_string("vi");
    rl_variable_bind(name.as_ptr(), value.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn rl_emacs_editing_mode(_count: c_int, _key: c_int) -> c_int {
    let name = clean_c_string("editing-mode");
    let value = clean_c_string("emacs");
    rl_variable_bind(name.as_ptr(), value.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn rl_tilde_expand(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let point = rl_point.max(0) as usize;
    let start = line[..point]
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    let word = line[start..point].to_string();
    if (word == "~" || word.starts_with("~/")) && std::env::var_os("HOME").is_some() {
        let home = std::env::var_os("HOME").unwrap();
        let mut updated = line;
        updated.replace_range(
            start..point,
            &format!("{}{}", home.to_string_lossy(), &word[1..]),
        );
        set_line_buffer(&updated, updated.len());
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_clear_screen(_count: c_int, _key: c_int) -> c_int {
    libc::write(libc::STDERR_FILENO, b"\x1b[2J\x1b[H".as_ptr().cast(), 7);
    0
}

action_alias! {
    rl_clear_display => rl_clear_screen,
    rl_operate_and_get_next => rl_newline,
    rl_fetch_history => rl_get_previous_history,
    rl_export_completions => rl_possible_completions,
    rl_yank_pop => rl_yank,
    rl_insert_close => rl_insert,
    rl_vi_fetch_history => rl_get_previous_history,
    rl_vi_arg_digit => rl_digit_argument,
    rl_vi_put => rl_yank,
    rl_vi_column => rl_beg_of_line,
    rl_vi_yank_pop => rl_yank_pop,
    rl_vi_back_to_indent => rl_beg_of_line,
    rl_vi_first_print => rl_beg_of_line,
    rl_vi_subst => rl_delete,
    rl_vi_overstrike_delete => rl_rubout,
    rl_vi_set_mark => rl_set_mark,
}

#[no_mangle]
pub unsafe extern "C" fn rl_execute_named_command(count: c_int, key: c_int) -> c_int {
    let mut command = Vec::new();
    loop {
        let input = rl_read_key();
        match input {
            value if value < 0 => return 1,
            10 | 13 => break,
            0x08 | 0x7f => {
                command.pop();
            }
            0x1b => return 1,
            value if (0x20..=0x7e).contains(&value) => command.push(value as u8),
            _ => {}
        }
    }
    if command.is_empty() {
        return 1;
    }
    let command = clean_c_string(&String::from_utf8_lossy(&command));
    let Some(function) = rl_named_function(command.as_ptr()) else {
        rl_ding();
        return 1;
    };
    let previous_dispatching = rl_dispatching;
    let was_dispatching = rl_readline_state & RL_STATE_DISPATCHING != 0;
    rl_dispatching = 1;
    rl_readline_state |= RL_STATE_DISPATCHING;
    let status = function(count, key);
    if !was_dispatching {
        rl_readline_state &= !RL_STATE_DISPATCHING;
    }
    rl_dispatching = previous_dispatching;
    rl_last_func = Some(function);
    status
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_yank_arg(count: c_int, key: c_int) -> c_int {
    if rl_explicit_arg != 0 {
        rl_yank_nth_arg(count - 1, key);
    } else {
        rl_yank_nth_arg(b'$' as c_int, key);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_search_again(count: c_int, key: c_int) -> c_int {
    match key as u8 {
        b'n' => {
            rl_noninc_reverse_search_again(count, key);
        }
        b'N' => {
            rl_noninc_forward_search_again(count, key);
        }
        _ => {}
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_search(count: c_int, key: c_int) -> c_int {
    match key as u8 {
        b'?' => {
            rl_noninc_forward_search(count, key);
        }
        b'/' => {
            rl_noninc_reverse_search(count, key);
        }
        _ => {
            rl_ding();
        }
    }
    0
}

unsafe fn vi_operator_range(count: c_int, key: c_int) -> Option<(usize, usize)> {
    let line = current_line();
    let start = clamp_boundary(&line, rl_point.max(0) as usize);
    let uppercase = (key as u8).is_ascii_uppercase();
    let motion = if uppercase {
        b'$' as c_int
    } else {
        rl_read_key()
    };
    if motion < 0 {
        return None;
    }
    let repetitions = count.max(1) as usize;
    let target = match motion as u8 {
        value if value == key as u8 && matches!(value, b'c' | b'd' | b'y') => {
            return Some((0, line.len()));
        }
        b'$' => line.len(),
        b'0' => 0,
        b'^' => line
            .char_indices()
            .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
            .unwrap_or(line.len()),
        b'h' => {
            let mut target = start;
            for _ in 0..repetitions {
                target = line[..target]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index);
            }
            target
        }
        b'l' | b' ' => {
            let mut target = start;
            for _ in 0..repetitions {
                if target < line.len() {
                    target += line[target..].chars().next().map_or(0, char::len_utf8);
                }
            }
            target
        }
        b'w' | b'W' | b'e' | b'E' => {
            let mut target = start;
            for _ in 0..repetitions {
                target = word_right(&line, target);
            }
            target
        }
        b'b' | b'B' => {
            let mut target = start;
            for _ in 0..repetitions {
                target = word_left(&line, target);
            }
            target
        }
        _ => {
            rl_ding();
            return None;
        }
    };
    Some((start.min(target), start.max(target)))
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_delete_to(count: c_int, key: c_int) -> c_int {
    let Some((start, end)) = vi_operator_range(count, key) else {
        return -1;
    };
    rl_kill_text(start as c_int, end as c_int);
    let line = current_line();
    if rl_point as usize == line.len() && rl_point > 0 {
        rl_point = line[..rl_point as usize]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index as c_int);
    }
    readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator = Some((count, key));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_change_to(count: c_int, key: c_int) -> c_int {
    let Some((start, end)) = vi_operator_range(count, key) else {
        return -1;
    };
    rl_kill_text(start as c_int, end as c_int);
    rl_vi_start_inserting(key, count, 1);
    readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator = Some((count, key));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_yank_to(count: c_int, key: c_int) -> c_int {
    let Some((start, end)) = vi_operator_range(count, key) else {
        return -1;
    };
    let line = current_line();
    if start < end {
        readline_store()
            .lock()
            .expect("readline lock")
            .kill_ring
            .push(line[start..end].to_string());
    }
    rl_point = start.min(line.len()) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_redo(_count: c_int, _key: c_int) -> c_int {
    let command = readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator;
    match command {
        Some((count, key)) if key == b'D' as c_int => rl_vi_delete_to(count, key),
        Some((count, key)) if key == b'C' as c_int => rl_vi_change_to(count, key),
        _ => {
            rl_ding();
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_do_lowercase_version(count: c_int, key: c_int) -> c_int {
    let lower = (key as u8).to_ascii_lowercase();
    if lower == key as u8 {
        rl_ding();
        return 0;
    }
    let map = if rl_executing_keymap.is_null() {
        rl_get_keymap()
    } else {
        rl_executing_keymap
    };
    let Some(entry) = keyseq_entry(map, &[lower], false) else {
        return 1;
    };
    if (*entry).r#type != 0 || (*entry).function.is_null() {
        return 1;
    }
    let function = std::mem::transmute::<*mut c_void, rl_command_func_t>((*entry).function);
    function(count, lower as c_int)
}

#[no_mangle]
pub unsafe extern "C" fn rl_bracketed_paste_begin(_count: c_int, _key: c_int) -> c_int {
    const END: &[u8] = b"\x1b[201~";
    let mut input = Vec::new();
    loop {
        let key = rl_read_key();
        if key < 0 {
            return 1;
        }
        input.push(key as u8);
        if input.ends_with(END) {
            input.truncate(input.len() - END.len());
            break;
        }
    }
    let input = String::from_utf8_lossy(&input);
    let input = clean_c_string(&input);
    rl_mark = rl_point;
    let expected = input.as_bytes().len();
    let inserted = rl_insert_text(input.as_ptr());
    rl_activate_mark();
    (inserted != expected.min(c_int::MAX as usize) as c_int) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_restart_output(_count: c_int, _key: c_int) -> c_int {
    let output = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    libc::tcflow(libc::fileno(output), libc::TCOON);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_stop_output(_count: c_int, _key: c_int) -> c_int {
    let input = if rl_instream.is_null() {
        C_STDIN
    } else {
        rl_instream
    };
    libc::tcflow(libc::fileno(input), libc::TCOOFF);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_tty_status(_count: c_int, _key: c_int) -> c_int {
    rl_ding();
    0
}

unsafe fn search_line_character(count: c_int, direction: c_int) -> c_int {
    let target = rl_read_key();
    if target < 0 {
        return 1;
    }
    let Some(target) = char::from_u32(target as u32) else {
        return 1;
    };
    let line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    for _ in 0..count.unsigned_abs().max(1) {
        let effective_direction = direction * if count < 0 { -1 } else { 1 };
        let found = if effective_direction < 0 {
            line[..point].rfind(target)
        } else {
            let start = line[point..]
                .chars()
                .next()
                .map_or(point, |character| point + character.len_utf8());
            line[start..].find(target).map(|offset| start + offset)
        };
        let Some(found) = found else {
            rl_ding();
            return 1;
        };
        point = found;
    }
    rl_point = point.min(c_int::MAX as usize) as c_int;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_char_search(count: c_int, _key: c_int) -> c_int {
    search_line_character(count, 1)
}

#[no_mangle]
pub unsafe extern "C" fn rl_backward_char_search(count: c_int, _key: c_int) -> c_int {
    search_line_character(count, -1)
}

unsafe fn set_vi_keymap(name: &str) {
    let name = clean_c_string(name);
    let keymap = rl_get_keymap_by_name(name.as_ptr());
    if !keymap.is_null() {
        rl_set_keymap(keymap);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_insertion_mode(_count: c_int, _key: c_int) -> c_int {
    rl_insert_mode = 1;
    rl_readline_state &= !RL_STATE_OVERWRITE;
    set_vi_keymap("vi-insertion");
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_insert_mode(count: c_int, key: c_int) -> c_int {
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_movement_mode(_count: c_int, key: c_int) -> c_int {
    if rl_point > 0 {
        rl_backward_char(1, key);
    }
    rl_insert_mode = 1;
    rl_readline_state &= !RL_STATE_OVERWRITE;
    set_vi_keymap("vi-movement");
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_insert_beg(count: c_int, key: c_int) -> c_int {
    rl_beg_of_line(count, key);
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_append_mode(count: c_int, key: c_int) -> c_int {
    rl_forward_char(1, key);
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_append_eol(count: c_int, key: c_int) -> c_int {
    rl_end_of_line(1, key);
    rl_vi_insertion_mode(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_change_case(count: c_int, _key: c_int) -> c_int {
    let mut line = current_line();
    let mut point = clamp_boundary(&line, rl_point.max(0) as usize);
    save_undo();
    for _ in 0..count.max(1) {
        let Some(character) = line[point..].chars().next() else {
            break;
        };
        let replacement = if character.is_lowercase() {
            character.to_uppercase().collect::<String>()
        } else if character.is_uppercase() {
            character.to_lowercase().collect::<String>()
        } else {
            character.to_string()
        };
        let end = point + character.len_utf8();
        line.replace_range(point..end, &replacement);
        point += replacement.len();
    }
    set_line_buffer(&line, point.min(line.len()));
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_char_search(count: c_int, key: c_int) -> c_int {
    let direction = if matches!(key as u8, b'F' | b'T') {
        -1
    } else {
        1
    };
    search_line_character(count, direction)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_match(_count: c_int, _key: c_int) -> c_int {
    let line = current_line();
    let mut start = clamp_boundary(&line, rl_point.max(0) as usize);
    while start < line.len()
        && !matches!(
            line.as_bytes()[start],
            b'(' | b')' | b'[' | b']' | b'{' | b'}'
        )
    {
        start += line[start..].chars().next().unwrap().len_utf8();
    }
    if start >= line.len() {
        rl_ding();
        return 1;
    }
    let current = line.as_bytes()[start];
    let (other, direction) = match current {
        b'(' => (b')', 1),
        b'[' => (b']', 1),
        b'{' => (b'}', 1),
        b')' => (b'(', -1),
        b']' => (b'[', -1),
        b'}' => (b'{', -1),
        _ => unreachable!(),
    };
    let mut depth = 1usize;
    let mut index = start as isize + direction;
    while index >= 0 && (index as usize) < line.len() {
        let byte = line.as_bytes()[index as usize];
        if byte == current {
            depth += 1;
        } else if byte == other {
            depth -= 1;
            if depth == 0 {
                rl_point = index as c_int;
                return 0;
            }
        }
        index += direction;
    }
    rl_ding();
    1
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_change_char(count: c_int, _key: c_int) -> c_int {
    let replacement = rl_read_key();
    if replacement < 0 {
        return 1;
    }
    rl_delete(count.max(1), 0);
    rl_insert(count.max(1), replacement)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_overstrike(_count: c_int, _key: c_int) -> c_int {
    rl_insert_mode = 0;
    rl_readline_state |= RL_STATE_OVERWRITE;
    set_vi_keymap("vi-insertion");
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_replace(count: c_int, key: c_int) -> c_int {
    rl_vi_overstrike(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_goto_mark(_count: c_int, _key: c_int) -> c_int {
    rl_point = rl_mark.clamp(0, rl_end);
    0
}

#[no_mangle]
pub extern "C" fn rl_start_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    store.current_macro.clear();
    store.macro_recording = true;
    0
}

#[no_mangle]
pub extern "C" fn rl_end_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    if !store.macro_recording {
        return 1;
    }
    store.macro_recording = false;
    store.last_macro = std::mem::take(&mut store.current_macro);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_call_last_kbd_macro(count: c_int, _key: c_int) -> c_int {
    let macro_text = readline_store()
        .lock()
        .expect("readline lock")
        .last_macro
        .clone();
    if macro_text.is_empty() {
        rl_ding();
        return 1;
    }
    let executing = malloc_string(&macro_text);
    rl_executing_macro = executing;
    for _ in 0..count.max(1) {
        for key in macro_text.bytes() {
            let entry = keyseq_entry(rl_get_keymap(), &[key], false);
            if let Some(entry) = entry.filter(|entry| (**entry).r#type == 0) {
                let function = (*entry).function;
                if !function.is_null() {
                    let function = std::mem::transmute::<*mut c_void, rl_command_func_t>(function);
                    function(1, key as c_int);
                    continue;
                }
            }
            rl_insert(1, key as c_int);
        }
    }
    libc::free(executing.cast());
    rl_executing_macro = ptr::null_mut();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_print_last_kbd_macro(_count: c_int, _key: c_int) -> c_int {
    let macro_text = readline_store()
        .lock()
        .expect("readline lock")
        .last_macro
        .clone();
    let stream = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    let macro_text = clean_c_string(&format!("{macro_text}\n"));
    libc::fputs(macro_text.as_ptr(), stream);
    libc::fflush(stream);
    0
}

unsafe fn yank_history_argument(argument: c_int, skip: usize) -> c_int {
    let line = {
        let store = history_store().lock().expect("history lock");
        let Some(index) = store.entries.len().checked_sub(skip + 1) else {
            rl_ding();
            return 1;
        };
        c_text((*(store.entries[index] as *mut HIST_ENTRY)).line).unwrap_or_default()
    };
    let line = clean_c_string(&line);
    let argument = history_arg_extract(argument, argument, line.as_ptr());
    if argument.is_null() || (*argument == 0) {
        libc::free(argument.cast());
        rl_ding();
        return 1;
    }
    rl_mark = rl_point;
    let result = rl_insert_text(argument);
    libc::free(argument.cast());
    (result < 0) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_yank_nth_arg(count: c_int, _key: c_int) -> c_int {
    yank_history_argument(count, 0)
}

#[no_mangle]
pub unsafe extern "C" fn rl_yank_last_arg(count: c_int, _key: c_int) -> c_int {
    let argument = if rl_explicit_arg != 0 {
        count
    } else {
        b'$' as c_int
    };
    yank_history_argument(argument, 0)
}

unsafe fn readline_history_search(direction: c_int, prefix: bool, again: bool) -> c_int {
    let needle = if again {
        readline_store()
            .lock()
            .expect("readline lock")
            .last_search
            .as_ref()
            .map(|(needle, _)| needle.clone())
            .unwrap_or_default()
    } else {
        let line = current_line();
        let point = clamp_boundary(&line, rl_point.max(0) as usize);
        line[..point].to_string()
    };
    if needle.is_empty() && !again {
        return 1;
    }
    let found = search_history(&needle, direction, prefix, None);
    if found < 0 {
        rl_ding();
        return 1;
    }
    readline_store().lock().expect("readline lock").last_search = Some((needle, prefix));
    let entry = current_history();
    if entry.is_null() {
        return 1;
    }
    let line = c_text((*entry).line).unwrap_or_default();
    set_line_buffer(&line, line.len());
    0
}

macro_rules! history_search_action {
    ($name:ident, $direction:expr, $prefix:expr, $again:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(_count: c_int, _key: c_int) -> c_int {
            readline_history_search($direction, $prefix, $again)
        }
    };
}

history_search_action!(rl_reverse_search_history, -1, false, false);
history_search_action!(rl_forward_search_history, 1, false, false);
history_search_action!(rl_history_search_backward, -1, true, false);
history_search_action!(rl_history_search_forward, 1, true, false);
history_search_action!(rl_history_substr_search_backward, -1, false, false);
history_search_action!(rl_history_substr_search_forward, 1, false, false);
history_search_action!(rl_noninc_reverse_search, -1, false, false);
history_search_action!(rl_noninc_forward_search, 1, false, false);
history_search_action!(rl_noninc_reverse_search_again, -1, false, true);
history_search_action!(rl_noninc_forward_search_again, 1, false, true);

#[no_mangle]
pub unsafe extern "C" fn rl_refresh_line(_count: c_int, _key: c_int) -> c_int {
    rl_redisplay();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_overwrite_mode(_count: c_int, _key: c_int) -> c_int {
    rl_insert_mode = (rl_insert_mode == 0) as c_int;
    if rl_insert_mode == 0 {
        rl_readline_state |= RL_STATE_OVERWRITE;
    } else {
        rl_readline_state &= !RL_STATE_OVERWRITE;
    }
    0
}

#[no_mangle]
pub extern "C" fn rl_noop_action(_count: c_int, _key: c_int) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_digit_argument(_count: c_int, key: c_int) -> c_int {
    if let Some(digit) = char::from_u32(key as u32).and_then(|ch| ch.to_digit(10)) {
        rl_numeric_arg = rl_numeric_arg
            .saturating_mul(10)
            .saturating_add(digit as c_int);
        rl_explicit_arg = 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_universal_argument(_count: c_int, _key: c_int) -> c_int {
    rl_numeric_arg = rl_numeric_arg.saturating_mul(4);
    rl_explicit_arg = 1;
    0
}

action_alias! {}

#[no_mangle]
pub unsafe extern "C" fn rl_skip_csi_sequence(_count: c_int, _key: c_int) -> c_int {
    loop {
        let key = rl_read_key();
        if key < 0 {
            return 1;
        }
        if !(0x20..0x40).contains(&key) {
            return 0;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_arrow_keys(count: c_int, _key: c_int) -> c_int {
    let key = rl_read_key();
    match (key as u8).to_ascii_uppercase() {
        b'A' => rl_get_previous_history(count, key),
        b'B' => rl_get_next_history(count, key),
        b'C' => rl_forward_char(count, key),
        b'D' => rl_backward_char(count, key),
        _ => (key < 0) as c_int,
    }
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

#[no_mangle]
pub extern "C" fn rl_dump_functions(_count: c_int, _key: c_int) -> c_int {
    rl_function_dumper(1);
    0
}

#[no_mangle]
pub extern "C" fn rl_dump_macros(_count: c_int, _key: c_int) -> c_int {
    rl_macro_dumper(1);
    0
}

#[no_mangle]
pub extern "C" fn rl_dump_variables(_count: c_int, _key: c_int) -> c_int {
    rl_variable_dumper(1);
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_free_line_state() {
    readline_store().lock().expect("readline lock").undo.clear();
    set_line_buffer("", 0);
    rl_done = 0;
}

#[no_mangle]
pub extern "C" fn rl_free_undo_list() {
    readline_store().lock().expect("readline lock").undo.clear();
}

#[no_mangle]
pub extern "C" fn free_undo_list() {
    rl_free_undo_list();
}

#[no_mangle]
pub unsafe extern "C" fn rl_add_undo(_what: c_int, _start: c_int, _end: c_int, _text: *mut c_char) {
    save_undo();
}

#[no_mangle]
pub unsafe extern "C" fn rl_begin_undo_group() -> c_int {
    save_undo();
    0
}

#[no_mangle]
pub extern "C" fn rl_end_undo_group() -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_modifying(_start: c_int, _end: c_int) -> c_int {
    save_undo();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_redisplay() {
    if let Some(callback) = rl_redisplay_function {
        callback();
    }
}

#[no_mangle]
pub extern "C" fn rl_on_new_line() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn rl_on_new_line_with_prompt() -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_forced_update_display() -> c_int {
    rl_redisplay();
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_clear_visible_line() -> c_int {
    libc::write(libc::STDERR_FILENO, b"\r\x1b[2K".as_ptr().cast(), 5);
    0
}

#[no_mangle]
pub extern "C" fn rl_clear_message() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn rl_reset_line_state() -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_crlf() -> c_int {
    libc::write(libc::STDERR_FILENO, b"\n".as_ptr().cast(), 1);
    0
}

#[no_mangle]
pub unsafe extern "C" fn crlf() -> c_int {
    rl_crlf()
}

#[no_mangle]
pub extern "C" fn rl_keep_mark_active() {
    readline_store()
        .lock()
        .expect("readline lock")
        .keep_mark_active = true;
}

#[no_mangle]
pub extern "C" fn rl_activate_mark() {
    let mut store = readline_store().lock().expect("readline lock");
    store.mark_active = true;
    store.keep_mark_active = true;
}

#[no_mangle]
pub extern "C" fn rl_deactivate_mark() {
    readline_store().lock().expect("readline lock").mark_active = false;
}

#[no_mangle]
pub extern "C" fn rl_mark_active_p() -> c_int {
    readline_store().lock().expect("readline lock").mark_active as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_message(message: *const c_char) -> c_int {
    let bytes = if message.is_null() {
        &[][..]
    } else {
        CStr::from_ptr(message).to_bytes()
    };
    libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len());
    0
}

#[no_mangle]
pub extern "C" fn rl_show_char(character: c_int) -> c_int {
    character
}

#[no_mangle]
pub extern "C" fn rl_character_len(character: c_int, _point: c_int) -> c_int {
    char::from_u32(character as u32).map_or(1, |ch| ch.len_utf8() as c_int)
}

#[no_mangle]
pub extern "C" fn rl_redraw_prompt_last_line() {}
#[no_mangle]
pub unsafe extern "C" fn rl_save_prompt() {
    let prompt = c_text(rl_prompt).unwrap_or_default();
    readline_store().lock().expect("readline lock").saved_prompt = Some(prompt);
}
#[no_mangle]
pub unsafe extern "C" fn rl_restore_prompt() {
    let prompt = readline_store()
        .lock()
        .expect("readline lock")
        .saved_prompt
        .take();
    if let Some(prompt) = prompt {
        let prompt = clean_c_string(&prompt);
        rl_set_prompt(prompt.as_ptr());
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_prep_terminal(meta_flag: c_int) {
    if let Some(callback) = rl_prep_term_function {
        callback(meta_flag);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_deprep_terminal() {
    if let Some(callback) = rl_deprep_term_function {
        callback();
    }
}

#[no_mangle]
pub extern "C" fn rl_tty_set_default_bindings(_keymap: Keymap) {}
#[no_mangle]
pub extern "C" fn rl_tty_unset_default_bindings(_keymap: Keymap) {}
#[no_mangle]
pub extern "C" fn rl_tty_set_echoing(value: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    let previous = store.tty_echoing;
    store.tty_echoing = value;
    previous
}
#[no_mangle]
pub unsafe extern "C" fn rl_reset_terminal(name: *const c_char) -> c_int {
    if !name.is_null() {
        rl_terminal_name = name;
    }
    rl_deprep_terminal();
    rl_prep_terminal(1);
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_resize_terminal() {
    if let Some(redisplay) = rl_redisplay_function {
        redisplay();
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_screen_size(rows: c_int, columns: c_int) {
    let size = libc::winsize {
        ws_row: rows.max(0) as u16,
        ws_col: columns.max(0) as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    libc::ioctl(libc::STDOUT_FILENO, libc::TIOCSWINSZ, &size);
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_screen_size(rows: *mut c_int, columns: *mut c_int) {
    let mut size: libc::winsize = std::mem::zeroed();
    libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size);
    if !rows.is_null() {
        *rows = size.ws_row as c_int;
    }
    if !columns.is_null() {
        *columns = size.ws_col as c_int;
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_reset_screen_size() {
    rl_resize_terminal();
}
#[no_mangle]
pub extern "C" fn rl_reparse_colors() {}

#[no_mangle]
pub unsafe extern "C" fn rl_stuff_char(character: c_int) -> c_int {
    rl_pending_input = character;
    1
}

#[no_mangle]
pub unsafe extern "C" fn rl_execute_next(character: c_int) -> c_int {
    rl_pending_input = character;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_clear_pending_input() -> c_int {
    rl_pending_input = 0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_read_key() -> c_int {
    if rl_pending_input != 0 {
        let value = rl_pending_input;
        rl_pending_input = 0;
        return value;
    }
    let (timeout, input) = {
        let store = readline_store().lock().expect("readline lock");
        (store.keyboard_timeout_us, rl_instream)
    };
    let descriptor = if input.is_null() {
        libc::STDIN_FILENO
    } else {
        libc::fileno(input)
    };
    if timeout >= 0 {
        let milliseconds = ((timeout as i64 + 999) / 1000).min(c_int::MAX as i64) as c_int;
        let mut pollfd = libc::pollfd {
            fd: descriptor,
            events: libc::POLLIN,
            revents: 0,
        };
        if libc::poll(&mut pollfd, 1, milliseconds) <= 0 {
            return -1;
        }
    }
    if let Some(getc) = rl_getc_function {
        return getc(if input.is_null() { C_STDIN } else { input });
    }
    if input.is_null() {
        let mut byte = 0u8;
        if libc::read(descriptor, (&raw mut byte).cast(), 1) == 1 {
            byte as c_int
        } else {
            -1
        }
    } else {
        libc::fgetc(input)
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_getc(stream: *mut libc::FILE) -> c_int {
    let mut byte = 0u8;
    if libc::read(libc::fileno(stream), (&raw mut byte).cast(), 1) == 1 {
        byte as c_int
    } else {
        libc::EOF
    }
}

#[no_mangle]
pub extern "C" fn rl_set_keyboard_input_timeout(microseconds: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    let previous = store.keyboard_timeout_us;
    if microseconds >= 0 {
        store.keyboard_timeout_us = microseconds;
    }
    previous
}

#[no_mangle]
pub extern "C" fn rl_set_timeout(seconds: u32, microseconds: u32) -> c_int {
    let seconds = u64::from(seconds) + u64::from(microseconds / 1_000_000);
    let microseconds = microseconds % 1_000_000;
    let mut store = readline_store().lock().expect("readline lock");
    store.timeout_duration = (seconds != 0 || microseconds != 0)
        .then(|| Duration::new(seconds, microseconds.saturating_mul(1_000)));
    store.timeout_deadline = None;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_timeout_remaining(seconds: *mut u32, microseconds: *mut u32) -> c_int {
    let deadline = readline_store()
        .lock()
        .expect("readline lock")
        .timeout_deadline;
    let Some(deadline) = deadline else {
        set_errno(0);
        return -1;
    };
    let now = Instant::now();
    if now >= deadline {
        return 0;
    }
    let remaining = deadline.duration_since(now);
    if !seconds.is_null() {
        *seconds = remaining.as_secs().min(u64::from(u32::MAX)) as u32;
    }
    if !microseconds.is_null() {
        *microseconds = remaining.subsec_micros();
    }
    1
}

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

#[no_mangle]
pub unsafe extern "C" fn rl_set_signals() -> c_int {
    let signals = [
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGQUIT,
        libc::SIGALRM,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGWINCH,
    ];
    let mut store = readline_store().lock().expect("readline lock");
    for signal in signals {
        if store.signal_handlers.contains_key(&signal) {
            continue;
        }
        let previous = libc::signal(
            signal,
            readline_signal_handler as *const () as libc::sighandler_t,
        );
        if previous == libc::SIG_ERR {
            return -1;
        }
        store.signal_handlers.insert(signal, previous);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_clear_signals() -> c_int {
    let handlers = std::mem::take(
        &mut readline_store()
            .lock()
            .expect("readline lock")
            .signal_handlers,
    );
    for (signal, handler) in handlers {
        libc::signal(signal, handler as libc::sighandler_t);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_cleanup_after_signal() {
    rl_deprep_terminal();
    rl_clear_signals();
}
#[no_mangle]
pub unsafe extern "C" fn rl_reset_after_signal() {
    rl_prep_terminal(1);
    if rl_catch_signals != 0 {
        rl_set_signals();
    }
}
#[no_mangle]
pub extern "C" fn rl_pending_signal() -> c_int {
    PENDING_SIGNAL.load(Ordering::SeqCst)
}
#[no_mangle]
pub unsafe extern "C" fn rl_check_signals() {
    let signal = PENDING_SIGNAL.swap(0, Ordering::SeqCst);
    if signal == 0 {
        return;
    }
    if let Some(hook) = rl_signal_event_hook {
        hook();
    }
    if signal == libc::SIGWINCH && rl_catch_sigwinch != 0 {
        rl_resize_terminal();
    }
}
#[no_mangle]
pub unsafe extern "C" fn rl_echo_signal_char(signal: c_int) {
    let character = match signal {
        libc::SIGINT => Some('C'),
        libc::SIGQUIT => Some('\\'),
        libc::SIGTSTP => Some('Z'),
        _ => None,
    };
    if let Some(character) = character {
        let stream = if rl_outstream.is_null() {
            C_STDOUT
        } else {
            rl_outstream
        };
        let text = clean_c_string(&format!("^{character}"));
        libc::fputs(text.as_ptr(), stream);
        libc::fflush(stream);
    }
}
#[no_mangle]
pub extern "C" fn rl_set_paren_blink_timeout(microseconds: c_int) -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    let previous = store.paren_blink_timeout_us;
    if microseconds >= 0 {
        store.paren_blink_timeout_us = microseconds;
    }
    previous
}

#[no_mangle]
pub extern "C" fn rl_clear_history() {
    clear_history();
}

#[no_mangle]
pub unsafe extern "C" fn rl_maybe_save_line() -> c_int {
    let mut store = readline_store().lock().expect("readline lock");
    if store.saved_line.is_none() {
        store.saved_line = Some((
            current_line(),
            rl_point.max(0) as usize,
            rl_mark.max(0) as usize,
        ));
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_maybe_unsave_line() -> c_int {
    let saved = readline_store()
        .lock()
        .expect("readline lock")
        .saved_line
        .take();
    if let Some((line, point, mark)) = saved {
        set_line_buffer(&line, point);
        rl_mark = mark.min(line.len()).min(c_int::MAX as usize) as c_int;
    } else {
        rl_ding();
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_maybe_replace_line() -> c_int {
    let position = where_history();
    let entry = current_history();
    if !entry.is_null() {
        let line = current_line();
        if c_text((*entry).line).as_deref() != Some(line.as_str()) {
            let old =
                replace_history_entry(position, clean_c_string(&line).as_ptr(), ptr::null_mut());
            if !old.is_null() {
                free_entry(old);
            }
        }
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn maybe_save_line() -> c_int {
    rl_maybe_save_line()
}
#[no_mangle]
pub unsafe extern "C" fn maybe_unsave_line() -> c_int {
    rl_maybe_unsave_line()
}
#[no_mangle]
pub unsafe extern "C" fn maybe_replace_line() -> c_int {
    rl_maybe_replace_line()
}

#[no_mangle]
pub extern "C" fn rl_completion_mode(_function: Option<rl_command_func_t>) -> c_int {
    unsafe { rl_completion_type }
}

#[no_mangle]
pub unsafe extern "C" fn rl_save_state(state: *mut c_void) -> c_int {
    let state = state.cast::<READLINE_STATE>();
    if state.is_null() {
        return -1;
    }
    let line_capacity = {
        readline_store()
            .lock()
            .expect("readline lock")
            .line_capacity
    };
    *state = READLINE_STATE {
        point: rl_point,
        end: rl_end,
        mark: rl_mark,
        buflen: line_capacity.min(c_int::MAX as usize) as c_int,
        buffer: rl_line_buffer,
        undo: rl_undo_list,
        prompt: rl_prompt,
        state: rl_readline_state as c_int,
        done: rl_done,
        keymap: rl_get_keymap(),
        last_function: rl_last_func,
        insert_mode: rl_insert_mode,
        editing_mode: rl_editing_mode,
        key_sequence: rl_executing_keyseq,
        key_sequence_length: rl_key_sequence_length,
        pending_input: rl_pending_input,
        input: rl_instream,
        output: rl_outstream,
        macro_text: rl_executing_macro,
        catch_signals: rl_catch_signals,
        catch_sigwinch: rl_catch_sigwinch,
        completion_entry: rl_completion_entry_function,
        menu_completion_entry: rl_menu_completion_entry_function,
        ignore_completions: rl_ignore_some_completions_function,
        attempted_completion: rl_attempted_completion_function,
        word_break_characters: rl_completer_word_break_characters,
        reserved: [0; 64],
    };
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_restore_state(state: *mut c_void) -> c_int {
    let state = state.cast::<READLINE_STATE>();
    if state.is_null() {
        return -1;
    }
    let state = &*state;
    rl_point = state.point;
    rl_end = state.end;
    rl_mark = state.mark;
    rl_line_buffer = state.buffer;
    rl_undo_list = state.undo;
    rl_prompt = state.prompt;
    rl_display_prompt = state.prompt;
    rl_readline_state = state.state as c_ulong;
    rl_done = state.done;
    rl_set_keymap(state.keymap);
    rl_last_func = state.last_function;
    rl_insert_mode = state.insert_mode;
    rl_editing_mode = state.editing_mode;
    rl_executing_keyseq = state.key_sequence;
    rl_key_sequence_length = state.key_sequence_length;
    rl_pending_input = state.pending_input;
    rl_instream = state.input;
    rl_outstream = state.output;
    rl_executing_macro = state.macro_text;
    rl_catch_signals = state.catch_signals;
    rl_catch_sigwinch = state.catch_sigwinch;
    rl_completion_entry_function = state.completion_entry;
    rl_menu_completion_entry_function = state.menu_completion_entry;
    rl_ignore_some_completions_function = state.ignore_completions;
    rl_attempted_completion_function = state.attempted_completion;
    rl_completer_word_break_characters = state.word_break_characters;
    let mut store = readline_store().lock().expect("readline lock");
    store.line_allocation = state.buffer as usize;
    store.line_capacity = state.buflen.max(0) as usize;
    store.prompt_allocation = state.prompt as usize;
    store.mark_active = false;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_vi_check() -> c_int {
    if rl_point > 0 && rl_point == rl_end {
        rl_backward_char(1, 0);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn rl_vi_domove(key: c_int, next: *mut c_int) -> c_int {
    let motion = rl_read_key();
    if !next.is_null() {
        *next = motion;
    }
    if motion < 0 {
        return -1;
    }
    if motion == key && matches!(key as u8, b'c' | b'd' | b'y') {
        rl_mark = rl_end;
        rl_point = 0;
        return 0;
    }
    let start = rl_point;
    let status = match motion as u8 {
        b'0' | b'^' => rl_beg_of_line(1, motion),
        b'$' => rl_end_of_line(1, motion),
        b'h' => rl_backward_char(rl_numeric_arg.max(1), motion),
        b'l' | b' ' => rl_forward_char(rl_numeric_arg.max(1), motion),
        b'b' | b'B' => rl_backward_word(rl_numeric_arg.max(1), motion),
        b'w' | b'W' | b'e' | b'E' => rl_forward_word(rl_numeric_arg.max(1), motion),
        _ => return 1,
    };
    rl_mark = start;
    status
}
#[no_mangle]
pub extern "C" fn rl_vi_bracktype(key: c_int) -> c_int {
    match key as u8 {
        b'(' => 1,
        b')' => -1,
        b'[' => 2,
        b']' => -2,
        b'{' => 3,
        b'}' => -3,
        _ => 0,
    }
}
#[no_mangle]
pub unsafe extern "C" fn rl_vi_start_inserting(key: c_int, count: c_int, _sign: c_int) {
    rl_begin_undo_group();
    readline_store()
        .lock()
        .expect("readline lock")
        .last_vi_operator = Some((count.max(1), key));
    rl_vi_insertion_mode(1, key);
}

#[no_mangle]
pub unsafe extern "C" fn rl_paste_from_clipboard(count: c_int, key: c_int) -> c_int {
    rl_noop_action(count, key)
}

#[no_mangle]
pub unsafe extern "C" fn rl_discard_argument() -> c_int {
    rl_numeric_arg = 1;
    rl_explicit_arg = 0;
    0
}

#[no_mangle]
pub unsafe extern "C" fn rl_variable_value(name: *const c_char) -> *mut c_char {
    let name = c_text(name).unwrap_or_default().to_ascii_lowercase();
    readline_store()
        .lock()
        .expect("readline lock")
        .variables
        .get(&name)
        .map_or(ptr::null_mut(), |value| value.as_ptr() as *mut c_char)
}

#[no_mangle]
pub extern "C" fn rl_untranslate_keyseq(key: c_int) -> *mut c_char {
    let value = if key == 0x1b {
        "\\e".to_string()
    } else if (0..32).contains(&key) {
        format!("\\C-{}", ((key as u8) | 0x40) as char)
    } else {
        char::from_u32(key as u32).unwrap_or('\u{fffd}').to_string()
    };
    unsafe { malloc_string(&value) }
}

const BUILTIN_FUNMAP: &[(&str, rl_command_func_t)] = &[
    ("abort", rl_abort),
    ("accept-line", rl_newline),
    ("arrow-key-prefix", rl_arrow_keys),
    ("backward-byte", rl_backward_byte),
    ("backward-char", rl_backward_char),
    ("backward-delete-char", rl_rubout),
    ("backward-kill-line", rl_backward_kill_line),
    ("backward-kill-word", rl_backward_kill_word),
    ("backward-word", rl_backward_word),
    ("beginning-of-history", rl_beginning_of_history),
    ("beginning-of-line", rl_beg_of_line),
    ("bracketed-paste-begin", rl_bracketed_paste_begin),
    ("call-last-kbd-macro", rl_call_last_kbd_macro),
    ("capitalize-word", rl_capitalize_word),
    ("character-search", rl_char_search),
    ("character-search-backward", rl_backward_char_search),
    ("clear-display", rl_clear_display),
    ("clear-screen", rl_clear_screen),
    ("complete", rl_complete),
    ("copy-backward-word", rl_copy_backward_word),
    ("copy-forward-word", rl_copy_forward_word),
    ("copy-region-as-kill", rl_copy_region_to_kill),
    ("delete-char", rl_delete),
    ("delete-char-or-list", rl_delete_or_show_completions),
    ("delete-horizontal-space", rl_delete_horizontal_space),
    ("digit-argument", rl_digit_argument),
    ("do-lowercase-version", rl_do_lowercase_version),
    ("downcase-word", rl_downcase_word),
    ("dump-functions", rl_dump_functions),
    ("dump-macros", rl_dump_macros),
    ("dump-variables", rl_dump_variables),
    ("emacs-editing-mode", rl_emacs_editing_mode),
    ("end-kbd-macro", rl_end_kbd_macro),
    ("end-of-history", rl_end_of_history),
    ("end-of-line", rl_end_of_line),
    ("exchange-point-and-mark", rl_exchange_point_and_mark),
    ("execute-named-command", rl_execute_named_command),
    ("export-completions", rl_export_completions),
    ("fetch-history", rl_fetch_history),
    ("forward-backward-delete-char", rl_rubout_or_delete),
    ("forward-byte", rl_forward_byte),
    ("forward-char", rl_forward_char),
    ("forward-search-history", rl_forward_search_history),
    ("forward-word", rl_forward_word),
    ("history-search-backward", rl_history_search_backward),
    ("history-search-forward", rl_history_search_forward),
    (
        "history-substring-search-backward",
        rl_history_substr_search_backward,
    ),
    (
        "history-substring-search-forward",
        rl_history_substr_search_forward,
    ),
    ("insert-comment", rl_insert_comment),
    ("insert-completions", rl_insert_completions),
    ("kill-whole-line", rl_kill_full_line),
    ("kill-line", rl_kill_line),
    ("kill-region", rl_kill_region),
    ("kill-word", rl_kill_word),
    ("menu-complete", rl_menu_complete),
    ("menu-complete-backward", rl_backward_menu_complete),
    ("next-history", rl_get_next_history),
    ("next-screen-line", rl_next_screen_line),
    (
        "non-incremental-forward-search-history",
        rl_noninc_forward_search,
    ),
    (
        "non-incremental-reverse-search-history",
        rl_noninc_reverse_search,
    ),
    (
        "non-incremental-forward-search-history-again",
        rl_noninc_forward_search_again,
    ),
    (
        "non-incremental-reverse-search-history-again",
        rl_noninc_reverse_search_again,
    ),
    ("old-menu-complete", rl_old_menu_complete),
    ("operate-and-get-next", rl_operate_and_get_next),
    ("overwrite-mode", rl_overwrite_mode),
    ("possible-completions", rl_possible_completions),
    ("previous-history", rl_get_previous_history),
    ("previous-screen-line", rl_previous_screen_line),
    ("print-last-kbd-macro", rl_print_last_kbd_macro),
    ("quoted-insert", rl_quoted_insert),
    ("re-read-init-file", rl_re_read_init_file),
    ("redraw-current-line", rl_refresh_line),
    ("reverse-search-history", rl_reverse_search_history),
    ("revert-line", rl_revert_line),
    ("self-insert", rl_insert),
    ("set-mark", rl_set_mark),
    ("skip-csi-sequence", rl_skip_csi_sequence),
    ("start-kbd-macro", rl_start_kbd_macro),
    ("tab-insert", rl_tab_insert),
    ("tilde-expand", rl_tilde_expand),
    ("transpose-chars", rl_transpose_chars),
    ("transpose-words", rl_transpose_words),
    ("tty-status", rl_tty_status),
    ("undo", rl_undo_command),
    ("universal-argument", rl_universal_argument),
    ("unix-filename-rubout", rl_unix_filename_rubout),
    ("unix-line-discard", rl_unix_line_discard),
    ("unix-word-rubout", rl_unix_word_rubout),
    ("upcase-word", rl_upcase_word),
    ("yank", rl_yank),
    ("yank-last-arg", rl_yank_last_arg),
    ("yank-nth-arg", rl_yank_nth_arg),
    ("yank-pop", rl_yank_pop),
    ("vi-append-eol", rl_vi_append_eol),
    ("vi-append-mode", rl_vi_append_mode),
    ("vi-arg-digit", rl_vi_arg_digit),
    ("vi-back-to-indent", rl_vi_back_to_indent),
    ("vi-backward-bigword", rl_vi_bWord),
    ("vi-backward-word", rl_vi_bword),
    ("vi-bWord", rl_vi_bWord),
    ("vi-bword", rl_vi_bword),
    ("vi-change-case", rl_vi_change_case),
    ("vi-change-char", rl_vi_change_char),
    ("vi-change-to", rl_vi_change_to),
    ("vi-char-search", rl_vi_char_search),
    ("vi-column", rl_vi_column),
    ("vi-complete", rl_vi_complete),
    ("vi-delete", rl_vi_delete),
    ("vi-delete-to", rl_vi_delete_to),
    ("vi-eWord", rl_vi_eWord),
    ("vi-editing-mode", rl_vi_editing_mode),
    ("vi-end-bigword", rl_vi_eWord),
    ("vi-end-word", rl_vi_end_word),
    ("vi-eof-maybe", rl_vi_eof_maybe),
    ("vi-eword", rl_vi_eword),
    ("vi-fWord", rl_vi_fWord),
    ("vi-fetch-history", rl_vi_fetch_history),
    ("vi-first-print", rl_vi_first_print),
    ("vi-forward-bigword", rl_vi_fWord),
    ("vi-forward-word", rl_vi_fword),
    ("vi-fword", rl_vi_fword),
    ("vi-goto-mark", rl_vi_goto_mark),
    ("vi-insert-beg", rl_vi_insert_beg),
    ("vi-insertion-mode", rl_vi_insert_mode),
    ("vi-match", rl_vi_match),
    ("vi-movement-mode", rl_vi_movement_mode),
    ("vi-next-word", rl_vi_next_word),
    ("vi-overstrike", rl_vi_overstrike),
    ("vi-overstrike-delete", rl_vi_overstrike_delete),
    ("vi-prev-word", rl_vi_prev_word),
    ("vi-put", rl_vi_put),
    ("vi-redo", rl_vi_redo),
    ("vi-replace", rl_vi_replace),
    ("vi-rubout", rl_vi_rubout),
    ("vi-search", rl_vi_search),
    ("vi-search-again", rl_vi_search_again),
    ("vi-set-mark", rl_vi_set_mark),
    ("vi-subst", rl_vi_subst),
    ("vi-tilde-expand", rl_vi_tilde_expand),
    ("vi-undo", rl_vi_undo),
    ("vi-unix-word-rubout", rl_vi_unix_word_rubout),
    ("vi-yank-arg", rl_vi_yank_arg),
    ("vi-yank-pop", rl_vi_yank_pop),
    ("vi-yank-to", rl_vi_yank_to),
];

fn named_function(name: &str) -> Option<rl_command_func_t> {
    rl_initialize_funmap();
    let address = readline_store()
        .lock()
        .expect("readline lock")
        .funmap_functions
        .get(&name.to_ascii_lowercase())
        .copied()?;
    Some(unsafe { std::mem::transmute::<usize, rl_command_func_t>(address) })
}

#[no_mangle]
pub unsafe extern "C" fn rl_named_function(name: *const c_char) -> Option<rl_command_func_t> {
    c_text(name).and_then(|name| named_function(&name))
}

#[no_mangle]
pub unsafe extern "C" fn rl_function_of_keyseq(
    sequence: *const c_char,
    keymap: Keymap,
    kind: *mut c_int,
) -> Option<rl_command_func_t> {
    let length = if sequence.is_null() {
        0
    } else {
        CStr::from_ptr(sequence).to_bytes().len()
    };
    rl_function_of_keyseq_len(sequence, length, keymap, kind)
}

#[no_mangle]
pub unsafe extern "C" fn rl_function_of_keyseq_len(
    sequence: *const c_char,
    length: usize,
    keymap: Keymap,
    kind: *mut c_int,
) -> Option<rl_command_func_t> {
    if sequence.is_null() || length == 0 {
        return None;
    }
    let keymap = if keymap.is_null() {
        rl_get_keymap()
    } else {
        keymap
    };
    let sequence = std::slice::from_raw_parts(sequence.cast::<u8>(), length);
    let entry = keyseq_entry(keymap, sequence, false)?;
    if !kind.is_null() {
        *kind = (*entry).r#type as c_int;
    }
    if !(*entry).function.is_null() {
        Some(std::mem::transmute::<*mut c_void, rl_command_func_t>(
            (*entry).function,
        ))
    } else {
        None
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_trim_arg_from_keyseq(
    sequence: *const c_char,
    length: usize,
    keymap: Keymap,
) -> c_int {
    if sequence.is_null() {
        return -1;
    }
    let original_map = if keymap.is_null() {
        rl_get_keymap()
    } else {
        keymap
    };
    if original_map.is_null() {
        return -1;
    }
    let sequence = std::slice::from_raw_parts(sequence.cast::<u8>(), length);
    let mut map = original_map;
    let mut parsed = 0usize;
    let mut parsing_digits = 0u8;
    for (index, key) in sequence.iter().copied().enumerate() {
        if parsing_digits == 2 && key == b'-' {
            parsed = index + 1;
            continue;
        }
        if parsing_digits != 0 {
            if key.is_ascii_digit() {
                parsed = index + 1;
                continue;
            }
            parsing_digits = 0;
        }
        let mut entry = &*map.cast::<KEYMAP_ENTRY>().add(key as usize);
        if entry.r#type == 1 {
            if entry.function.is_null() {
                return parsed.min(c_int::MAX as usize) as c_int;
            }
            map = entry.function;
            if index + 1 < sequence.len() {
                continue;
            }
            entry = &*map.cast::<KEYMAP_ENTRY>().add(256);
        }
        if entry.r#type != 0 {
            return parsed.min(c_int::MAX as usize) as c_int;
        }
        let function = entry.function;
        let digit = command_pointer(rl_digit_argument);
        let universal = command_pointer(rl_universal_argument);
        let vi_digit = command_pointer(rl_vi_arg_digit);
        if function != digit && function != universal && function != vi_digit {
            return parsed.min(c_int::MAX as usize) as c_int;
        }
        if index + 1 == sequence.len() {
            return -1;
        }
        parsing_digits = if function == universal || (function == digit && key == b'-') {
            2
        } else {
            1
        };
        map = original_map;
        parsed = index + 1;
    }
    -1
}

#[no_mangle]
pub unsafe extern "C" fn rl_list_funmap_names() {
    let names = rl_funmap_names();
    if names.is_null() {
        return;
    }
    let mut index = 0usize;
    while !(*names.add(index)).is_null() {
        write_readline_output(&c_text(*names.add(index)).unwrap_or_default());
        index += 1;
    }
    libc::free(names.cast());
}

#[no_mangle]
pub unsafe extern "C" fn rl_invoking_keyseqs_in_map(
    function: Option<rl_command_func_t>,
    keymap: Keymap,
) -> *mut *mut c_char {
    if keymap.is_null() {
        return ptr::null_mut();
    }
    let wanted = function.map_or(ptr::null_mut(), command_pointer);
    let mut values = Vec::new();
    let mut visited = Vec::new();
    collect_invoking_keyseqs(keymap, wanted, String::new(), &mut visited, &mut values);
    completion_array(&values, false)
}

unsafe fn collect_invoking_keyseqs(
    keymap: Keymap,
    wanted: *mut c_void,
    prefix: String,
    visited: &mut Vec<usize>,
    output: &mut Vec<String>,
) {
    if keymap.is_null() || visited.contains(&(keymap as usize)) {
        return;
    }
    visited.push(keymap as usize);
    for key in 0usize..256 {
        let entry = &*keymap.cast::<KEYMAP_ENTRY>().add(key);
        let mut sequence = prefix.clone();
        sequence.push_str(&key_name(key as u8));
        match entry.r#type {
            0 | 2 if entry.function == wanted => output.push(sequence),
            1 if !entry.function.is_null() => {
                collect_invoking_keyseqs(entry.function, wanted, sequence, visited, output);
            }
            _ => {}
        }
    }
    visited.pop();
}

fn key_name(key: u8) -> String {
    match key {
        0x1b => "\\e".to_string(),
        0x7f => "\\C-?".to_string(),
        0..=0x1f => format!("\\C-{}", ((key | 0x40) as char).to_ascii_lowercase()),
        b'\\' | b'"' => format!("\\{}", key as char),
        0x80..=0xff => format!("\\{:03o}", key),
        _ => (key as char).to_string(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_invoking_keyseqs(
    function: Option<rl_command_func_t>,
) -> *mut *mut c_char {
    rl_invoking_keyseqs_in_map(function, rl_get_keymap())
}

#[no_mangle]
pub unsafe extern "C" fn rl_print_keybinding(name: *const c_char, keymap: Keymap, readable: c_int) {
    let Some(name_text) = c_text(name) else {
        return;
    };
    let function = rl_named_function(name);
    let keymap = if keymap.is_null() {
        rl_get_keymap()
    } else {
        keymap
    };
    let sequences = function.map_or(ptr::null_mut(), |function| {
        rl_invoking_keyseqs_in_map(Some(function), keymap)
    });
    let mut values = Vec::new();
    if !sequences.is_null() {
        let mut index = 0usize;
        while !(*sequences.add(index)).is_null() {
            values.push(c_text(*sequences.add(index)).unwrap_or_default());
            libc::free((*sequences.add(index)).cast());
            index += 1;
        }
        libc::free(sequences.cast());
    }
    if readable != 0 {
        if values.is_empty() {
            write_readline_output(&format!("# {name_text} (not bound)"));
        } else {
            for sequence in values {
                write_readline_output(&format!("\"{sequence}\": {name_text}"));
            }
        }
    } else if values.is_empty() {
        write_readline_output(&format!("{name_text} is not bound to any keys"));
    } else {
        let shown = values
            .iter()
            .take(5)
            .map(|sequence| format!("\"{sequence}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let ending = if values.len() > 5 { ", ..." } else { "." };
        write_readline_output(&format!("{name_text} can be found on {shown}{ending}"));
    }
}

#[no_mangle]
pub extern "C" fn rl_function_dumper(readable: c_int) {
    unsafe {
        let names = rl_funmap_names();
        if names.is_null() {
            return;
        }
        write_readline_output("");
        let mut index = 0usize;
        while !(*names.add(index)).is_null() {
            rl_print_keybinding(*names.add(index), rl_get_keymap(), readable);
            index += 1;
        }
        libc::free(names.cast());
    }
}

#[no_mangle]
pub extern "C" fn rl_macro_dumper(_readable: c_int) {
    let mut macros = Vec::new();
    let mut visited = Vec::new();
    unsafe {
        collect_macros(rl_get_keymap(), String::new(), &mut visited, &mut macros);
    }
    for (sequence, macro_text) in macros {
        unsafe { write_readline_output(&format!("\"{sequence}\": \"{macro_text}\"")) };
    }
}

#[no_mangle]
pub extern "C" fn rl_variable_dumper(_readable: c_int) {
    for (name, value) in &readline_store().lock().expect("readline lock").variables {
        unsafe { write_readline_output(&format!("set {name} {}", value.to_string_lossy())) };
    }
}

unsafe fn write_readline_output(text: &str) {
    let stream = if rl_outstream.is_null() {
        C_STDOUT
    } else {
        rl_outstream
    };
    let text = clean_c_string(&format!("{text}\n"));
    libc::fputs(text.as_ptr(), stream);
    libc::fflush(stream);
}

unsafe fn collect_macros(
    keymap: Keymap,
    prefix: String,
    visited: &mut Vec<usize>,
    output: &mut Vec<(String, String)>,
) {
    if keymap.is_null() || visited.contains(&(keymap as usize)) {
        return;
    }
    visited.push(keymap as usize);
    for key in 0usize..256 {
        let entry = &*keymap.cast::<KEYMAP_ENTRY>().add(key);
        let mut sequence = prefix.clone();
        sequence.push_str(&key_name(key as u8));
        if entry.r#type == 1 && !entry.function.is_null() {
            collect_macros(entry.function, sequence, visited, output);
        } else if entry.r#type == 2 && !entry.function.is_null() {
            let macro_text = c_text(entry.function.cast()).unwrap_or_default();
            output.push((sequence, macro_text.replace('"', "\\\"")));
        }
    }
    visited.pop();
}

#[no_mangle]
pub extern "C" fn rl_get_keymap_name(keymap: Keymap) -> *mut c_char {
    let store = readline_store().lock().expect("readline lock");
    store
        .keymap_names
        .iter()
        .find_map(|(name, address)| {
            (*address == keymap as usize).then(|| unsafe { malloc_string(name) })
        })
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn rl_set_keymap_from_edit_mode() {
    let name = if rl_editing_mode == 0 {
        clean_c_string("vi-insertion")
    } else {
        clean_c_string("emacs-standard")
    };
    let keymap = rl_get_keymap_by_name(name.as_ptr());
    rl_set_keymap(keymap);
}

#[no_mangle]
pub extern "C" fn rl_get_keymap_name_from_edit_mode() -> *mut c_char {
    unsafe {
        malloc_string(if rl_editing_mode == 0 {
            "vi-insertion"
        } else {
            "emacs-standard"
        })
    }
}

#[no_mangle]
pub static mut funmap_program_specific_entry_start: c_int = 0;

#[no_mangle]
pub unsafe extern "C" fn rl_add_funmap_entry(
    name: *const c_char,
    function: Option<rl_command_func_t>,
) -> c_int {
    let Some(name_text) = c_text(name) else {
        return -1;
    };
    let name_pointer = malloc_string(&name_text);
    if name_pointer.is_null() {
        return -1;
    }
    let entry = Box::into_raw(Box::new(FUNMAP {
        name: name_pointer,
        function,
    }));
    let mut store = readline_store().lock().expect("readline lock");
    store.funmap_entries.push(entry as usize);
    if let Some(function) = function {
        store
            .funmap_functions
            .entry(name_text.to_ascii_lowercase())
            .or_insert(function as *const () as usize);
    }
    let array = libc::calloc(
        store.funmap_entries.len() + 1,
        std::mem::size_of::<*mut FUNMAP>(),
    ) as *mut *mut FUNMAP;
    if array.is_null() {
        return -1;
    }
    for (index, entry) in store.funmap_entries.iter().copied().enumerate() {
        *array.add(index) = entry as *mut FUNMAP;
    }
    if store.funmap_array != 0 {
        libc::free(store.funmap_array as *mut c_void);
    }
    store.funmap_array = array as usize;
    funmap = array;
    store.funmap_entries.len().min(c_int::MAX as usize) as c_int
}

#[no_mangle]
pub unsafe extern "C" fn rl_funmap_names() -> *mut *const c_char {
    rl_initialize_funmap();
    let mut names: Vec<*const c_char> = readline_store()
        .lock()
        .expect("readline lock")
        .funmap_entries
        .iter()
        .map(|entry| (*(*entry as *mut FUNMAP)).name)
        .collect();
    names.sort_by(|left, right| libc::strcoll(*left, *right).cmp(&0));
    let output =
        libc::calloc(names.len() + 1, std::mem::size_of::<*const c_char>()) as *mut *const c_char;
    if output.is_null() {
        return ptr::null_mut();
    }
    for (index, name) in names.into_iter().enumerate() {
        *output.add(index) = name;
    }
    output
}

#[no_mangle]
pub extern "C" fn rl_initialize_funmap() {
    {
        let mut store = readline_store().lock().expect("readline lock");
        if store.funmap_initialized {
            return;
        }
        store.funmap_initialized = true;
    }
    for &(name, function) in BUILTIN_FUNMAP {
        let name = clean_c_string(name);
        unsafe {
            rl_add_funmap_entry(name.as_ptr(), Some(function));
        }
    }
    unsafe {
        funmap_program_specific_entry_start =
            BUILTIN_FUNMAP.len().min(c_int::MAX as usize) as c_int;
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_push_macro_input(text: *mut c_char) {
    if !text.is_null() {
        rl_executing_macro = text;
    }
}

#[no_mangle]
pub unsafe extern "C" fn rl_copy_text(start: c_int, end: c_int) -> *mut c_char {
    let line = current_line();
    let start = clamp_boundary(&line, start.max(0) as usize);
    let end = clamp_boundary(&line, end.max(0) as usize).max(start);
    malloc_string(&line[start..end])
}

#[no_mangle]
pub unsafe extern "C" fn rl_get_termcap(capability: *const c_char) -> *mut c_char {
    match c_text(capability).as_deref() {
        Some("ce") => malloc_string("\x1b[K"),
        Some("cl") => malloc_string("\x1b[2J\x1b[H"),
        Some("cr") => malloc_string("\r"),
        Some("le") => malloc_string("\x1b[D"),
        Some("nd") => malloc_string("\x1b[C"),
        Some("up") => malloc_string("\x1b[A"),
        _ => ptr::null_mut(),
    }
}

struct DescriptorRedirect {
    target: c_int,
    saved: c_int,
}

impl DescriptorRedirect {
    unsafe fn new(source: c_int, target: c_int) -> Option<Self> {
        if source < 0 || source == target {
            return None;
        }
        let saved = libc::dup(target);
        if saved < 0 || libc::dup2(source, target) < 0 {
            if saved >= 0 {
                libc::close(saved);
            }
            return None;
        }
        Some(Self { target, saved })
    }
}

impl Drop for DescriptorRedirect {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved, self.target);
            libc::close(self.saved);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn readline(prompt: *const c_char) -> *mut c_char {
    rl_initialize();
    let prompt_text = c_text(prompt).unwrap_or_default();
    rl_set_prompt(prompt);
    rl_done = 0;
    rl_eof_found = 0;
    rl_readline_state &= !(RL_STATE_TIMEOUT | RL_STATE_EOF);
    set_line_buffer("", 0);
    if let Some(hook) = rl_startup_hook {
        hook();
    }
    let input_descriptor = if rl_instream.is_null() {
        libc::STDIN_FILENO
    } else {
        libc::fileno(rl_instream)
    };
    let output_descriptor = if rl_outstream.is_null() {
        libc::STDOUT_FILENO
    } else {
        libc::fileno(rl_outstream)
    };
    let _input_redirect = DescriptorRedirect::new(input_descriptor, libc::STDIN_FILENO);
    let _output_redirect = DescriptorRedirect::new(output_descriptor, libc::STDERR_FILENO);
    let (mut editor, timeout_deadline) = {
        let mut store = readline_store().lock().expect("readline lock");
        let deadline = store
            .timeout_duration
            .map(|duration| Instant::now() + duration);
        store.timeout_deadline = deadline;
        let editor = store.editor.take().unwrap_or_else(|| {
            let mut keymap = RustKeymap::new("emacs");
            keymap.install_emacs_defaults();
            LineEditor::new(keymap)
        });
        (editor, deadline)
    };
    set_input_deadline(timeout_deadline);
    let mut history = FfiHistory::snapshot();
    let mut completion = FfiCompleter;
    let input_is_tty = libc::isatty(libc::STDIN_FILENO) != 0;
    if !input_is_tty {
        set_errno(libc::ENOTTY);
    }
    let result = if input_is_tty {
        editor.readline(&prompt_text, &mut history, &mut completion)
    } else {
        editor.readline_scripted(&prompt_text, &mut history, &mut completion)
    };
    set_input_deadline(None);
    readline_store().lock().expect("readline lock").editor = Some(editor);
    match result {
        Ok(line) => {
            set_line_buffer(&line, line.len());
            malloc_string(&line)
        }
        Err(EditError::Eof) => {
            rl_eof_found = 1;
            rl_readline_state |= RL_STATE_EOF;
            ptr::null_mut()
        }
        Err(EditError::Interrupted) => {
            set_errno(libc::EINTR);
            ptr::null_mut()
        }
        Err(EditError::Io(error)) => {
            if error.kind() == std::io::ErrorKind::TimedOut {
                rl_readline_state |= RL_STATE_TIMEOUT;
                if let Some(hook) = rl_timeout_event_hook {
                    hook();
                }
            } else {
                set_errno(error.raw_os_error().unwrap_or(libc::EIO));
            }
            ptr::null_mut()
        }
    }
}

unsafe fn set_errno(value: c_int) {
    *libc::__errno_location() = value;
}

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
