use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn temporary_directory(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cherubsh-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary directory");
    path
}

fn run(command: &mut Command, description: &str) {
    let output = command.output().expect(description);
    assert!(
        output.status.success(),
        "{description} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_failure(command: &mut Command, description: &str) {
    let output = command.output().expect(description);
    assert!(
        !output.status.success(),
        "{description} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn pkg_config_args(pkg_config_path: &Path, static_link: bool) -> Vec<String> {
    let mut command = Command::new("pkg-config");
    if static_link {
        command.arg("--static");
    }
    let output = command
        .args(["--cflags", "--libs", "readline", "history"])
        .env("PKG_CONFIG_PATH", pkg_config_path)
        .output()
        .expect("run pkg-config");
    assert!(
        output.status.success(),
        "pkg-config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8 pkg-config output")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn compile_client(source: &Path, prefix: &Path, output: &Path, static_link: bool) {
    let mut command = Command::new("cc");
    command.arg(source);
    if std::env::var("CHERUBSH_C_SANITIZER").as_deref() == Ok("address") {
        command.args(["-fsanitize=address", "-fno-omit-frame-pointer"]);
    }
    if static_link {
        for argument in pkg_config_args(&prefix.join("lib/pkgconfig"), true) {
            match argument.as_str() {
                "-lreadline" => {
                    command.arg(prefix.join("lib/libreadline.a"));
                }
                "-lhistory" => {
                    command.arg(prefix.join("lib/libhistory.a"));
                }
                _ => {
                    command.arg(argument);
                }
            }
        }
    } else {
        command.args(pkg_config_args(&prefix.join("lib/pkgconfig"), false));
        command.arg(format!("-Wl,-rpath,{}", prefix.join("lib").display()));
    }
    command.arg("-o").arg(output);
    run(&mut command, "compile installed readline client");
}

fn run_client(client: &Path, library_path: Option<&Path>) {
    let mut child = Command::new(client)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(library_path.map(|path| ("LD_LIBRARY_PATH", path)))
        .spawn()
        .expect("start installed readline client");
    child
        .stdin
        .as_mut()
        .expect("client stdin")
        .write_all(b"Ada\n")
        .expect("write client input");
    let output = child.wait_with_output().expect("wait for readline client");
    assert!(
        output.status.success(),
        "client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello Ada"));
}

#[test]
fn development_archive_installs_links_examples_and_uninstalls_owned_files() {
    let workspace = workspace_root();
    let temporary = temporary_directory("readline-install");
    let built = temporary.join("built");
    let dist = temporary.join("dist");
    let repeated_dist = temporary.join("dist-repeat");

    run(
        Command::new("bash")
            .arg(workspace.join("tools/build-readline.sh"))
            .env("READLINE_OUTPUT_ROOT", &built),
        "build readline development files",
    );
    fs::write(
        built.join("include/readline/undeclared-stale.h"),
        "must not ship\n",
    )
    .expect("write stale development input");
    run(
        Command::new("bash")
            .arg(workspace.join("tools/package-readline-dev.sh"))
            .args(["--version", "0.0.0-test", "--input"])
            .arg(&built)
            .args(["--output"])
            .arg(&dist)
            .env("SOURCE_DATE_EPOCH", "1700000000"),
        "package readline development files",
    );

    let platform = match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => panic!("unsupported development package test architecture: {other}"),
    };
    let package_name = format!("cherubsh-readline-dev-0.0.0-test-{platform}");
    let archive = dist.join(format!("{package_name}.tar.gz"));
    assert!(archive.is_file(), "development archive is missing");
    run(
        Command::new("bash")
            .arg(workspace.join("tools/package-readline-dev.sh"))
            .args(["--version", "0.0.0-test", "--input"])
            .arg(&built)
            .args(["--output"])
            .arg(&repeated_dist)
            .env("SOURCE_DATE_EPOCH", "1700000000"),
        "repeat readline development packaging",
    );
    run(
        Command::new("cmp")
            .arg("--silent")
            .arg(&archive)
            .arg(repeated_dist.join(format!("{package_name}.tar.gz"))),
        "compare repeated development archive",
    );
    run(
        Command::new("tar")
            .args(["-xzf"])
            .arg(&archive)
            .args(["-C"])
            .arg(&temporary),
        "extract readline development archive",
    );

    let package = temporary.join(package_name);
    for path in [
        "include/readline/readline.h",
        "include/readline/history.h",
        "lib/libreadline.a",
        "lib/libhistory.a",
        "lib/pkgconfig/readline.pc",
        "lib/pkgconfig/history.pc",
        "examples/readline-client.c",
        "tools/install-readline-dev.sh",
        "manifests/readline.files",
        "manifests/history.files",
        "LICENSE",
    ] {
        assert!(package.join(path).is_file(), "archive is missing {path}");
    }
    assert!(package.join("lib/libreadline.so").is_symlink());
    assert!(package.join("lib/libhistory.so.8").is_symlink());
    assert!(!package.join("include/readline/undeclared-stale.h").exists());

    fs::rename(
        built.join("include/readline/keymaps.h"),
        built.join("include/readline/keymaps.h.missing"),
    )
    .expect("hide manifested header");
    run_failure(
        Command::new("bash")
            .arg(workspace.join("tools/package-readline-dev.sh"))
            .args(["--version", "0.0.0-test", "--input"])
            .arg(&built)
            .args(["--output"])
            .arg(temporary.join("dist-missing"))
            .env("SOURCE_DATE_EPOCH", "1700000000"),
        "package development files with a missing manifest entry",
    );

    let destdir = temporary.join("root");
    let installed = destdir.join("opt/cherub");
    run_failure(
        Command::new("bash")
            .arg(package.join("tools/install-readline-dev.sh"))
            .args(["install", "--component", "all", "--prefix", "/usr"])
            .arg("--destdir")
            .arg(destdir.join("stage/../../..")),
        "install through a traversing DESTDIR",
    );
    fs::create_dir_all(installed.join("include")).expect("create unrelated file directory");
    fs::write(installed.join("include/unrelated.h"), "keep me\n").expect("write unrelated file");
    let installer = package.join("tools/install-readline-dev.sh");
    let history_collision = installed.join("include/readline/history.h");
    fs::create_dir_all(
        history_collision
            .parent()
            .expect("history collision parent"),
    )
    .expect("create history collision directory");
    fs::write(&history_collision, "foreign history header\n")
        .expect("write colliding history file");
    run_failure(
        Command::new("bash")
            .arg(&installer)
            .args(["install", "--component", "all", "--prefix", "/opt/cherub"])
            .env("DESTDIR", &destdir),
        "install all components with a History collision",
    );
    assert_eq!(
        fs::read_to_string(&history_collision).expect("read colliding history file"),
        "foreign history header\n"
    );
    assert!(!installed.join("include/readline/readline.h").exists());
    assert!(!installed
        .join("share/cherubsh/readline-dev/readline.manifest")
        .exists());
    fs::remove_file(&history_collision).expect("remove colliding history file");

    let collision = installed.join("include/readline/readline.h");
    fs::create_dir_all(collision.parent().expect("collision parent"))
        .expect("create collision directory");
    fs::write(&collision, "foreign header\n").expect("write colliding file");
    run_failure(
        Command::new("bash")
            .arg(&installer)
            .args([
                "install",
                "--component",
                "readline",
                "--prefix",
                "/opt/cherub",
            ])
            .env("DESTDIR", &destdir),
        "install over an unowned destination",
    );
    assert_eq!(
        fs::read_to_string(&collision).expect("read colliding file"),
        "foreign header\n"
    );
    assert!(!installed
        .join("share/licenses/cherubsh-readline/LICENSE")
        .exists());
    fs::remove_file(&collision).expect("remove colliding test file");
    run(
        Command::new("bash")
            .arg(&installer)
            .args(["install", "--component", "all", "--prefix", "/opt/cherub"])
            .env("DESTDIR", &destdir),
        "install readline development files through DESTDIR",
    );
    run(
        Command::new("bash")
            .arg(&installer)
            .args(["install", "--component", "all", "--prefix", "/opt/cherub"])
            .arg("--destdir")
            .arg(&destdir),
        "reinstall readline development files",
    );

    assert!(installed.join("lib/libreadline.so").is_symlink());
    assert!(installed.join("lib/libhistory.so.8").is_symlink());
    assert!(installed
        .join("share/cherubsh/readline-dev/readline.manifest")
        .is_file());
    assert!(installed
        .join("share/cherubsh/readline-dev/history.manifest")
        .is_file());

    let packaged_example = package.join("examples/readline-client.c");
    let shared_client = temporary.join("readline-client-shared");
    compile_client(&packaged_example, &installed, &shared_client, false);
    run_client(&shared_client, Some(&installed.join("lib")));

    let static_client = temporary.join("readline-client-static");
    compile_client(&packaged_example, &installed, &static_client, true);
    run_client(&static_client, None);

    run(
        Command::new("bash")
            .arg(&installer)
            .args([
                "uninstall",
                "--component",
                "readline",
                "--prefix",
                "/opt/cherub",
            ])
            .arg("--destdir")
            .arg(&destdir),
        "uninstall readline component",
    );
    assert!(!installed.join("lib/libreadline.a").exists());
    assert!(installed.join("lib/libhistory.a").is_file());
    assert!(installed.join("include/unrelated.h").is_file());

    run(
        Command::new("bash")
            .arg(&installer)
            .args([
                "uninstall",
                "--component",
                "history",
                "--prefix",
                "/opt/cherub",
            ])
            .arg("--destdir")
            .arg(&destdir),
        "uninstall history component",
    );
    assert!(!installed.join("lib/libhistory.a").exists());
    assert_eq!(
        fs::read_to_string(installed.join("include/unrelated.h")).expect("read unrelated file"),
        "keep me\n"
    );

    let _ = fs::remove_dir_all(&temporary);
}
