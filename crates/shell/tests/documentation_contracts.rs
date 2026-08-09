use std::collections::BTreeSet;
use std::fs;
use std::process::Command;

use cherubsh_test_harness::{cherub_path, workspace_root};

fn long_options(text: &str) -> BTreeSet<String> {
    let normalized = text.replace("\\-", "-");
    let bytes = normalized.as_bytes();
    let mut options = BTreeSet::new();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] != b'-' || bytes[index + 1] != b'-' {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        if !bytes[index].is_ascii_alphanumeric() {
            continue;
        }
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'-')
        {
            index += 1;
        }
        if index > start + 2 {
            options.insert(normalized[start..index].to_string());
        }
    }
    options
}

#[test]
fn built_in_long_options_match_the_manual_and_wiki() {
    let help = Command::new(cherub_path().expect("find cherubsh test binary"))
        .arg("--help")
        .output()
        .expect("run cherubsh --help");
    assert!(help.status.success());
    let help_options = long_options(&String::from_utf8_lossy(&help.stdout));
    assert_eq!(help_options.len(), 16, "unexpected built-in option count");

    let root = workspace_root();
    let manual = fs::read_to_string(root.join("man/cherubsh.1")).expect("read cherubsh manual");
    let wiki = fs::read_to_string(root.join("wiki/Command-line-reference.md"))
        .expect("read command-line reference");

    assert_eq!(
        long_options(&manual),
        help_options,
        "man/cherubsh.1 long options differ from cherubsh --help"
    );
    assert_eq!(
        long_options(&wiki),
        help_options,
        "wiki command-line long options differ from cherubsh --help"
    );
}

#[test]
fn roff_escaped_long_options_are_normalized() {
    assert_eq!(
        long_options(r".B \-\-pretty\-print"),
        BTreeSet::from(["--pretty-print".to_string()])
    );
}
