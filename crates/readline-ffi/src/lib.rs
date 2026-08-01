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

include!("ffi/abi.rs");
include!("ffi/history_core.rs");
include!("ffi/history_io.rs");
include!("ffi/history_state.rs");
include!("ffi/history_expansion.rs");
include!("ffi/history_navigation.rs");
include!("ffi/globals.rs");
include!("ffi/editor_runtime.rs");
include!("ffi/buffer_state.rs");
include!("ffi/editing_core.rs");
include!("ffi/completion.rs");
include!("ffi/callbacks.rs");
include!("ffi/inputrc.rs");
include!("ffi/keymaps.rs");
include!("ffi/editing_commands.rs");
include!("ffi/editing_misc.rs");
include!("ffi/editing_dispatch.rs");
include!("ffi/editing_macros.rs");
include!("ffi/vi_support.rs");
include!("ffi/vi_commands.rs");
include!("ffi/undo.rs");
include!("ffi/redisplay.rs");
include!("ffi/terminal.rs");
include!("ffi/input.rs");
include!("ffi/terminal_commands.rs");
include!("ffi/signals.rs");
include!("ffi/saved_state.rs");
include!("ffi/function_maps.rs");
include!("ffi/streams.rs");
include!("ffi/tilde.rs");
include!("ffi/tests.rs");
