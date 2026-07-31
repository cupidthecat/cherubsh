use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cherubsh-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn starter_config_installer_creates_but_never_overwrites_cherubrc() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let installer = workspace.join("tools/install-cherubrc.sh");
    let template = workspace.join("examples/cherubrc");
    let temporary = temporary_directory("starter-config");
    let target = temporary.join("home/.cherubrc");
    fs::create_dir_all(target.parent().expect("starter config parent"))
        .expect("create starter config home");

    let first = Command::new("bash")
        .arg(&installer)
        .args(["--path", target.to_str().expect("UTF-8 target path")])
        .output()
        .expect("run starter config installer");
    assert!(
        first.status.success(),
        "installer failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        fs::read(&target).expect("read installed config"),
        fs::read(&template).expect("read template")
    );

    fs::write(&target, "# user changes stay here\n").expect("replace installed config");
    let second = Command::new("bash")
        .arg(&installer)
        .args(["--path", target.to_str().expect("UTF-8 target path")])
        .output()
        .expect("rerun starter config installer");

    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already exists"));
    assert_eq!(
        fs::read(&target).expect("read preserved config"),
        b"# user changes stay here\n"
    );
    let _ = fs::remove_dir_all(&temporary);
}
