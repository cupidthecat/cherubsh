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
