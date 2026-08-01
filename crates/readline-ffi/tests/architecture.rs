use std::fs;
use std::path::PathBuf;

#[test]
fn readline_ffi_has_focused_responsibility_files() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let expected = [
        ("lib.rs", "include!(\"ffi/abi.rs\")"),
        ("ffi/abi.rs", "pub struct READLINE_STATE"),
        (
            "ffi/history_core.rs",
            "pub unsafe extern \"C\" fn add_history",
        ),
        (
            "ffi/history_io.rs",
            "pub unsafe extern \"C\" fn read_history",
        ),
        (
            "ffi/history_state.rs",
            "pub extern \"C\" fn history_get_history_state",
        ),
        (
            "ffi/history_expansion.rs",
            "pub unsafe extern \"C\" fn history_expand",
        ),
        ("ffi/history_navigation.rs", "fn readline_history_search"),
        ("ffi/globals.rs", "pub static mut rl_library_version"),
        ("ffi/editor_runtime.rs", "struct FfiHistory"),
        (
            "ffi/buffer_state.rs",
            "pub unsafe extern \"C\" fn rl_extend_line_buffer",
        ),
        (
            "ffi/editing_core.rs",
            "pub unsafe extern \"C\" fn rl_insert_text",
        ),
        (
            "ffi/completion.rs",
            "pub unsafe extern \"C\" fn rl_completion_matches",
        ),
        (
            "ffi/callbacks.rs",
            "pub unsafe extern \"C\" fn rl_callback_handler_install",
        ),
        (
            "ffi/inputrc.rs",
            "pub unsafe extern \"C\" fn rl_read_init_file",
        ),
        (
            "ffi/keymaps.rs",
            "pub unsafe extern \"C\" fn rl_make_bare_keymap",
        ),
        (
            "ffi/editing_commands.rs",
            "pub unsafe extern \"C\" fn rl_quoted_insert",
        ),
        (
            "ffi/editing_misc.rs",
            "pub unsafe extern \"C\" fn rl_bracketed_paste_begin",
        ),
        (
            "ffi/editing_dispatch.rs",
            "pub unsafe extern \"C\" fn rl_arrow_keys",
        ),
        (
            "ffi/editing_macros.rs",
            "pub extern \"C\" fn rl_start_kbd_macro",
        ),
        (
            "ffi/vi_support.rs",
            "pub unsafe extern \"C\" fn rl_vi_domove",
        ),
        (
            "ffi/vi_commands.rs",
            "pub unsafe extern \"C\" fn rl_vi_insertion_mode",
        ),
        ("ffi/undo.rs", "pub unsafe extern \"C\" fn rl_add_undo"),
        (
            "ffi/redisplay.rs",
            "pub unsafe extern \"C\" fn rl_redisplay",
        ),
        (
            "ffi/terminal.rs",
            "pub unsafe extern \"C\" fn rl_prep_terminal",
        ),
        ("ffi/input.rs", "pub unsafe extern \"C\" fn rl_read_key"),
        (
            "ffi/terminal_commands.rs",
            "pub unsafe extern \"C\" fn rl_restart_output",
        ),
        (
            "ffi/signals.rs",
            "pub unsafe extern \"C\" fn rl_set_signals",
        ),
        (
            "ffi/saved_state.rs",
            "pub unsafe extern \"C\" fn rl_save_state",
        ),
        (
            "ffi/function_maps.rs",
            "pub unsafe extern \"C\" fn rl_named_function",
        ),
        ("ffi/streams.rs", "pub unsafe extern \"C\" fn readline"),
        (
            "ffi/tilde.rs",
            "pub unsafe extern \"C\" fn tilde_expand_word",
        ),
        ("ffi/tests.rs", "mod tests"),
    ];

    for (name, responsibility_anchor) in expected {
        let contents = fs::read_to_string(source.join(name))
            .unwrap_or_else(|error| panic!("read Readline FFI file {name}: {error}"));
        assert!(
            contents.lines().count() < 900,
            "Readline FFI file {name} is still too large"
        );
        assert!(
            contents.contains(responsibility_anchor),
            "Readline FFI file {name} lost {responsibility_anchor}"
        );
        assert!(
            !contents.ends_with("\n\n"),
            "Readline FFI file {name} has a surplus blank line at EOF"
        );
    }

    let root = fs::read_to_string(source.join("lib.rs")).expect("read Readline FFI root");
    let includes: Vec<_> = root
        .lines()
        .filter(|line| line.starts_with("include!("))
        .collect();
    assert_eq!(
        includes,
        [
            "include!(\"ffi/abi.rs\");",
            "include!(\"ffi/history_core.rs\");",
            "include!(\"ffi/history_io.rs\");",
            "include!(\"ffi/history_state.rs\");",
            "include!(\"ffi/history_expansion.rs\");",
            "include!(\"ffi/history_navigation.rs\");",
            "include!(\"ffi/globals.rs\");",
            "include!(\"ffi/editor_runtime.rs\");",
            "include!(\"ffi/buffer_state.rs\");",
            "include!(\"ffi/editing_core.rs\");",
            "include!(\"ffi/completion.rs\");",
            "include!(\"ffi/callbacks.rs\");",
            "include!(\"ffi/inputrc.rs\");",
            "include!(\"ffi/keymaps.rs\");",
            "include!(\"ffi/editing_commands.rs\");",
            "include!(\"ffi/editing_misc.rs\");",
            "include!(\"ffi/editing_dispatch.rs\");",
            "include!(\"ffi/editing_macros.rs\");",
            "include!(\"ffi/vi_support.rs\");",
            "include!(\"ffi/vi_commands.rs\");",
            "include!(\"ffi/undo.rs\");",
            "include!(\"ffi/redisplay.rs\");",
            "include!(\"ffi/terminal.rs\");",
            "include!(\"ffi/input.rs\");",
            "include!(\"ffi/terminal_commands.rs\");",
            "include!(\"ffi/signals.rs\");",
            "include!(\"ffi/saved_state.rs\");",
            "include!(\"ffi/function_maps.rs\");",
            "include!(\"ffi/streams.rs\");",
            "include!(\"ffi/tilde.rs\");",
            "include!(\"ffi/tests.rs\");",
        ]
    );

    let abi = fs::read_to_string(source.join("ffi/abi.rs")).expect("read ABI types");
    for misplaced in [
        "emacs_standard_keymap",
        "tilde_expansion_preexpansion_hook",
        "rl_undo_list",
        "pub static mut funmap",
        "history_base",
    ] {
        assert!(
            !abi.contains(misplaced),
            "ffi/abi.rs still contains {misplaced}"
        );
    }

    let globals = fs::read_to_string(source.join("ffi/globals.rs")).expect("read globals");
    for misplaced in [
        "struct ReadlineStore",
        "PENDING_SIGNAL",
        "readline_signal_handler",
    ] {
        assert!(
            !globals.contains(misplaced),
            "ffi/globals.rs still contains {misplaced}"
        );
    }

    let vi = fs::read_to_string(source.join("ffi/vi_commands.rs")).expect("read Vi commands");
    for misplaced in [
        "rl_bracketed_paste_begin",
        "rl_start_kbd_macro",
        "history_search_action",
        "rl_re_read_init_file",
        "rl_dump_functions",
    ] {
        assert!(
            !vi.contains(misplaced),
            "ffi/vi_commands.rs still contains {misplaced}"
        );
    }

    let history_io =
        fs::read_to_string(source.join("ffi/history_io.rs")).expect("read history I/O");
    for misplaced in [
        "history_get_history_state",
        "history_expand",
        "history_tokenize",
    ] {
        assert!(
            !history_io.contains(misplaced),
            "ffi/history_io.rs still contains {misplaced}"
        );
    }

    let redisplay =
        fs::read_to_string(source.join("ffi/redisplay.rs")).expect("read redisplay functions");
    for misplaced in ["rl_add_undo", "rl_prep_terminal", "rl_read_key"] {
        assert!(
            !redisplay.contains(misplaced),
            "ffi/redisplay.rs still contains {misplaced}"
        );
    }
}
