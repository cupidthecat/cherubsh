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
        ("ffi/globals.rs", "pub static mut rl_library_version"),
        ("ffi/editor_runtime.rs", "struct FfiHistory"),
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
            "ffi/vi_commands.rs",
            "pub unsafe extern \"C\" fn rl_vi_insertion_mode",
        ),
        (
            "ffi/redisplay_terminal.rs",
            "pub unsafe extern \"C\" fn rl_redisplay",
        ),
        (
            "ffi/signals_state.rs",
            "pub unsafe extern \"C\" fn rl_set_signals",
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
            "include!(\"ffi/globals.rs\");",
            "include!(\"ffi/editor_runtime.rs\");",
            "include!(\"ffi/editing_core.rs\");",
            "include!(\"ffi/completion.rs\");",
            "include!(\"ffi/callbacks.rs\");",
            "include!(\"ffi/inputrc.rs\");",
            "include!(\"ffi/keymaps.rs\");",
            "include!(\"ffi/editing_commands.rs\");",
            "include!(\"ffi/vi_commands.rs\");",
            "include!(\"ffi/redisplay_terminal.rs\");",
            "include!(\"ffi/signals_state.rs\");",
            "include!(\"ffi/function_maps.rs\");",
            "include!(\"ffi/streams.rs\");",
            "include!(\"ffi/tilde.rs\");",
            "include!(\"ffi/tests.rs\");",
        ]
    );
}
