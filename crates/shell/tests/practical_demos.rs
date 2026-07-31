use std::path::PathBuf;
use std::process::Command;

use cherubsh_test_harness::cherub_path;

#[test]
fn practical_demo_smoke_check_accepts_the_documented_examples() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let checker = workspace.join("tools/check-examples.sh");
    let cherub = cherub_path().expect("CherubSH test binary");
    let output = Command::new("bash")
        .arg(checker)
        .env("CHERUBSH", cherub)
        .output()
        .expect("run practical demo smoke check");

    assert!(
        output.status.success(),
        "example smoke check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
