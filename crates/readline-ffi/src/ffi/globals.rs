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
