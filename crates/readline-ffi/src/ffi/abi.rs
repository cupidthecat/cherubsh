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
