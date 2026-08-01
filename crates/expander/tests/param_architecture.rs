use std::fs;
use std::path::PathBuf;

use cherubsh_expander::param::{param_expand, process_subst_expand, special_byte, ValueRepr};

#[test]
fn parameter_expansion_has_focused_module_homes() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/param");
    let expected = [
        ("mod.rs", "pub fn param_expand"),
        ("constructs.rs", "fn dollar_paren"),
        ("braces.rs", "fn parameter_brace_expand"),
        ("defaults.rs", "fn handle_default_alt"),
        ("operators.rs", "fn handle_substring"),
        ("transforms.rs", "fn handle_transform"),
        ("prompts.rs", "const PROMPT_ESCAPED_DOLLAR"),
        ("values.rs", "pub enum ValueRepr"),
        ("scanning.rs", "fn extract_brace_body"),
        ("public_api.rs", "pub fn process_subst_expand"),
        ("tests.rs", "mod tests"),
    ];

    for (name, responsibility_anchor) in expected {
        let path = source.join(name);
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read parameter expansion module {name}: {error}"));
        assert!(
            contents.lines().count() < 1_600,
            "parameter expansion module {name} is still too large"
        );
        assert!(
            contents.contains(responsibility_anchor),
            "parameter expansion module {name} lost {responsibility_anchor}"
        );
    }

    let module = fs::read_to_string(source.join("mod.rs")).expect("read parameter module root");
    let includes: Vec<_> = module
        .lines()
        .filter(|line| line.starts_with("include!("))
        .collect();
    assert_eq!(
        includes,
        [
            "include!(\"constructs.rs\");",
            "include!(\"braces.rs\");",
            "include!(\"defaults.rs\");",
            "include!(\"operators.rs\");",
            "include!(\"transforms.rs\");",
            "include!(\"prompts.rs\");",
            "include!(\"values.rs\");",
            "include!(\"scanning.rs\");",
            "include!(\"public_api.rs\");",
            "include!(\"tests.rs\");",
        ]
    );
}

#[test]
fn parameter_expansion_public_exports_remain_available() {
    let _ = param_expand;
    let _ = process_subst_expand;
    assert!(special_byte(b'?'));
    let value = ValueRepr::Scalar("kept".to_string());
    assert!(matches!(value, ValueRepr::Scalar(text) if text == "kept"));
}
