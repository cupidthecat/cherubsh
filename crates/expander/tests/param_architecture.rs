use std::fs;
use std::path::PathBuf;

use cherubsh_expander::param::{special_byte, ValueRepr};

#[test]
fn parameter_expansion_has_focused_module_homes() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/param");
    let expected = [
        "mod.rs",
        "constructs.rs",
        "braces.rs",
        "defaults.rs",
        "operators.rs",
        "transforms.rs",
        "prompts.rs",
        "values.rs",
        "scanning.rs",
        "public_api.rs",
        "tests.rs",
    ];

    for name in expected {
        let path = source.join(name);
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read parameter expansion module {name}: {error}"));
        assert!(
            contents.lines().count() < 1_600,
            "parameter expansion module {name} is still too large"
        );
    }
}

#[test]
fn parameter_expansion_public_exports_remain_available() {
    assert!(special_byte(b'?'));
    let value = ValueRepr::Scalar("kept".to_string());
    assert!(matches!(value, ValueRepr::Scalar(text) if text == "kept"));
}
