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

#[repr(C)]
pub struct UNDO_LIST {
    pub next: *mut UNDO_LIST,
    pub start: c_int,
    pub end: c_int,
    pub text: *mut c_char,
    pub what: c_int,
}

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
