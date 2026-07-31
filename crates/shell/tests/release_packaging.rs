use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_test_harness::cherub_path;

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
fn release_packager_creates_a_checksummed_portable_archive() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temporary = temporary_directory("release-package");
    let output = temporary.join("dist");
    let repeated_output = temporary.join("dist-repeat");
    let platform = match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => panic!("unsupported release test architecture: {other}"),
    };
    let package_name = format!("cherubsh-0.0.0-test-{platform}");
    let archive_name = format!("{package_name}.tar.gz");

    let packaged = Command::new("bash")
        .arg(workspace.join("tools/package-release.sh"))
        .args(["--version", "0.0.0-test"])
        .args([
            "--binary",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
        ])
        .args(["--output", "dist"])
        .current_dir(&temporary)
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .output()
        .expect("run release packager");
    assert!(
        packaged.status.success(),
        "release packager failed:\n{}",
        String::from_utf8_lossy(&packaged.stderr)
    );

    let archive = output.join(&archive_name);
    assert!(
        archive.is_file(),
        "archive missing at {}",
        archive.display()
    );
    let listing = Command::new("tar")
        .args(["-tzf", archive.to_str().expect("UTF-8 archive path")])
        .output()
        .expect("list release archive");
    assert!(listing.status.success());
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains(&format!("{package_name}/cherubsh")));
    assert!(listing.contains(&format!("{package_name}/examples/cherubrc")));
    assert!(listing.contains(&format!("{package_name}/tools/install-cherubrc.sh")));

    let repeated = Command::new("bash")
        .arg(workspace.join("tools/package-release.sh"))
        .args(["--version", "0.0.0-test"])
        .args([
            "--binary",
            cherub_path()
                .expect("CherubSH test binary")
                .to_str()
                .expect("UTF-8 binary path"),
        ])
        .args(["--output", "dist-repeat"])
        .current_dir(&temporary)
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .output()
        .expect("rerun release packager");
    assert!(
        repeated.status.success(),
        "second release packaging run failed:\n{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_archive = repeated_output.join(&archive_name);
    let identical = Command::new("cmp")
        .args([
            "--silent",
            archive.to_str().expect("UTF-8 archive path"),
            repeated_archive
                .to_str()
                .expect("UTF-8 repeated archive path"),
        ])
        .status()
        .expect("compare repeated release archive");
    assert!(identical.success(), "repeated release archive changed");

    let checksum = Command::new("sha256sum")
        .args(["--check", "SHA256SUMS"])
        .current_dir(&output)
        .output()
        .expect("verify package checksum");
    let _ = fs::remove_dir_all(&temporary);
    assert!(
        checksum.status.success(),
        "checksum verification failed:\n{}",
        String::from_utf8_lossy(&checksum.stderr)
    );
}
