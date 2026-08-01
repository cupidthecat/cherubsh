use std::collections::BTreeSet;
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
    for path in [
        "man/cherubsh.1",
        "man/cherubsh-readline.3",
        "man/cherubshrc.5",
        "completions/cherubsh",
        "manifests/cherubsh.files",
        "tools/install-cherubsh.sh",
    ] {
        assert!(
            listing.contains(&format!("{package_name}/{path}")),
            "archive is missing {path}"
        );
    }

    let extracted = temporary.join("extracted");
    fs::create_dir_all(&extracted).expect("create extraction directory");
    let extract = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .args(["-C"])
        .arg(&extracted)
        .output()
        .expect("extract release archive");
    assert!(extract.status.success());
    let package = extracted.join(&package_name);
    let destdir = temporary.join("root");
    let installed = destdir.join("opt/cherub");
    let install = Command::new("bash")
        .arg(package.join("tools/install-cherubsh.sh"))
        .args(["install", "--prefix", "/opt/cherub", "--destdir"])
        .arg(&destdir)
        .output()
        .expect("install release archive");
    assert!(
        install.status.success(),
        "release install failed:\n{}",
        String::from_utf8_lossy(&install.stderr)
    );
    for path in [
        "bin/cherubsh",
        "share/man/man1/cherubsh.1",
        "share/man/man3/cherubsh-readline.3",
        "share/man/man5/cherubshrc.5",
        "share/bash-completion/completions/cherubsh",
    ] {
        assert!(installed.join(path).is_file(), "install is missing {path}");
    }

    let help = Command::new(installed.join("bin/cherubsh"))
        .arg("--help")
        .output()
        .expect("run installed shell help");
    assert!(help.status.success());
    let expected_long_options = String::from_utf8_lossy(&help.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("--"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let completion = Command::new("bash")
        .args([
            "--noprofile",
            "--norc",
            "-c",
            "source \"$CHERUB_COMPLETION\"; COMP_WORDS=(cherubsh --); COMP_CWORD=1; _cherubsh; printf '%s\\n' \"${COMPREPLY[@]}\"",
        ])
        .env(
            "CHERUB_COMPLETION",
            installed.join("share/bash-completion/completions/cherubsh"),
        )
        .output()
        .expect("run installed completion");
    assert!(
        completion.status.success(),
        "completion failed:\n{}",
        String::from_utf8_lossy(&completion.stderr)
    );
    let completed_options = String::from_utf8_lossy(&completion.stdout)
        .lines()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(completed_options, expected_long_options);

    for page in ["cherubsh", "cherubsh-readline", "cherubshrc"] {
        let rendered = Command::new("man")
            .args(["-M"])
            .arg(installed.join("share/man"))
            .args(["-P", "cat", page])
            .output()
            .unwrap_or_else(|error| panic!("render installed {page} manual: {error}"));
        assert!(
            rendered.status.success(),
            "installed {page} manual failed:\n{}",
            String::from_utf8_lossy(&rendered.stderr)
        );
        assert!(
            !rendered.stdout.is_empty(),
            "installed {page} manual is empty"
        );
    }

    fs::create_dir_all(installed.join("share/keep")).expect("create unrelated directory");
    fs::write(installed.join("share/keep/unrelated"), "keep\n").expect("write unrelated file");
    let uninstall = Command::new("bash")
        .arg(package.join("tools/install-cherubsh.sh"))
        .args(["uninstall", "--prefix", "/opt/cherub", "--destdir"])
        .arg(&destdir)
        .output()
        .expect("uninstall release archive");
    assert!(uninstall.status.success());
    assert!(!installed.join("bin/cherubsh").exists());
    assert!(!installed.join("share/man/man1/cherubsh.1").exists());
    assert!(installed.join("share/keep/unrelated").is_file());

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
