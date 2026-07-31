#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use cherubsh_test_harness::{cherub_path, oracle_bash_path, oracle_version};

fn loadable_parity_enabled() -> bool {
    std::env::var_os("RUN_LOADABLE_PARITY").is_some()
}

fn skip_without_loadable_parity() -> bool {
    if loadable_parity_enabled() {
        false
    } else {
        eprintln!(
            "skip: set RUN_LOADABLE_PARITY=1 to run the Bash loadable ABI compatibility suite"
        );
        true
    }
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

fn compile_fixture(source: &Path, output: &Path) {
    let status = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .args([
            "-std=c11", "-Wall", "-Wextra", "-Werror", "-fPIC", "-shared",
        ])
        .arg(source)
        .arg("-o")
        .arg(output)
        .status()
        .expect("run C compiler");
    assert!(status.success(), "failed to compile {}", source.display());
}

fn run(shell: &Path, script: &str, loadable_path: &Path) -> Output {
    Command::new(shell)
        .args(["--norc", "--noprofile", "-c", script])
        .env_clear()
        .env("HOME", "/tmp")
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/bin")
        .env("BASH_LOADABLES_PATH", loadable_path)
        .output()
        .expect("run shell")
}

fn run_stdin(shell: &Path, script: &str, loadable_path: &Path) -> Output {
    let mut child = Command::new(shell)
        .args(["--norc", "--noprofile"])
        .env_clear()
        .env("HOME", "/tmp")
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/bin")
        .env("BASH_LOADABLES_PATH", loadable_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start shell");
    child
        .stdin
        .take()
        .expect("shell stdin")
        .write_all(script.as_bytes())
        .expect("write shell input");
    child.wait_with_output().expect("wait for shell")
}

fn loadable_catalog(directory: &Path) -> Vec<(PathBuf, String, String)> {
    let mut catalog = Vec::new();
    for entry in fs::read_dir(directory).expect("read Bash loadable directory") {
        let path = entry.expect("read directory entry").path();
        if !path.is_file() || path.extension().is_some() {
            continue;
        }
        let output = Command::new("nm")
            .args(["-D", "--defined-only", "--format=posix"])
            .arg(&path)
            .output()
            .expect("inspect Bash loadable module");
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Some(symbol) = line.split_whitespace().next() else {
                continue;
            };
            let Some(requested_name) = symbol.strip_suffix("_struct") else {
                continue;
            };
            let command_name = if requested_name == "necho" {
                "echo"
            } else {
                requested_name
            };
            catalog.push((
                path.clone(),
                requested_name.to_string(),
                command_name.to_string(),
            ));
        }
    }
    catalog.sort_by(|left, right| left.1.cmp(&right.1));
    catalog
}

#[test]
fn bash_53_loadable_builtin_abi_matches_the_oracle() {
    if skip_without_loadable_parity() {
        return;
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temporary = temporary_directory("loadable-abi");
    let module = temporary.join("abi_probe.so");
    compile_fixture(&workspace.join("tests/loadables/abi_probe.c"), &module);

    let script = r#"
enable -f abi_probe.so abi_probe
printf 'hook:%s\n' "$PROBE_LOAD"
abi_probe -s alpha -i 7 -k answer -v forty-two tail
printf 'values:%s,%s,%s\n' "$PROBE_SCALAR" "${PROBE_INDEXED[7]}" "${PROBE_ASSOC[answer]}"
enable -n abi_probe
enable abi_probe
abi_probe -s beta -i 9 -k second -v value
enable -d abi_probe
printf 'unload:%s,%s\n' "$PROBE_UNLOAD" "$(type -t abi_probe)"
"#;
    let bash = oracle_bash_path();
    assert!(
        bash.exists(),
        "Bash {} oracle missing at {}",
        oracle_version(),
        bash.display()
    );
    let expected = run(&bash, script, &temporary);
    let actual = run(
        &cherub_path().expect("CARGO_BIN_EXE_cherubsh"),
        script,
        &temporary,
    );
    let _ = fs::remove_dir_all(&temporary);

    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&expected.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&actual.stderr),
        String::from_utf8_lossy(&expected.stderr)
    );
}

#[test]
fn every_bash_53_example_loadable_can_load_run_help_and_unload() {
    if skip_without_loadable_parity() {
        return;
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = workspace.join("target/oracle/bash-5.3.15/examples/loadables");
    let catalog = loadable_catalog(&directory);
    assert!(
        catalog.len() >= 40,
        "expected the Bash 5.3.15 loadable catalog at {}; run oracle/build-bash-5.3.15-loadables.sh",
        directory.display()
    );
    let bash = oracle_bash_path();
    let cherub = cherub_path().expect("CARGO_BIN_EXE_cherubsh");

    for (module, requested_name, command_name) in catalog {
        let script = format!(
            "enable -f \"$1\" {requested_name}\n\
             load_status=$?\n\
             {command_name} --help >/dev/null 2>&1\n\
             help_status=$?\n\
             enable -d {command_name}\n\
             unload_status=$?\n\
             printf '%s,%s,%s\\n' \"$load_status\" \"$help_status\" \"$unload_status\"\n"
        );
        let run_module = |shell: &Path| {
            Command::new(shell)
                .args(["--norc", "--noprofile", "-c", &script, "loadable-test"])
                .arg(&module)
                .env_clear()
                .env("HOME", "/tmp")
                .env("LANG", "C")
                .env("PATH", "/usr/bin:/bin")
                .output()
                .expect("run loadable module probe")
        };
        let expected = run_module(&bash);
        let actual = run_module(&cherub);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "status mismatch for {requested_name}"
        );
        assert_eq!(
            actual.stdout, expected.stdout,
            "stdout mismatch for {requested_name}"
        );
        assert_eq!(
            actual.stderr, expected.stderr,
            "stderr mismatch for {requested_name}"
        );
    }
}

#[test]
fn representative_bash_53_loadables_match_the_oracle() {
    if skip_without_loadable_parity() {
        return;
    }

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = workspace.join("target/oracle/bash-5.3.15/examples/loadables");
    assert!(
        directory.join("fltexpr").is_file(),
        "Bash 5.3.15 loadables are missing; run oracle/build-bash-5.3.15-loadables.sh"
    );
    let script = r#"
enable -f printenv printenv
export ABI_PRINT='plain value'
printf 'printenv='; printenv ABI_PRINT; printf ' status=%s\n' "$?"

enable -f fltexpr fltexpr
float_value=1.25
fltexpr 'float_value += 2.5'
printf 'fltexpr=%s status=%s\n' "$float_value" "$?"
float_values=(1.25 2.5)
fltexpr 'float_values[1] += 0.5'
printf 'fltexpr_array=%s status=%s\n' "${float_values[1]}" "$?"
declare -A float_map=([pi]=3.0)
fltexpr 'float_map[pi] += 0.14'
printf 'fltexpr_assoc=%s status=%s\n' "${float_map[pi]}" "$?"
fltexpr '1 / 0' 2>/dev/null
printf 'fltexpr_error_status=%s\n' "$?"

enable -f kv kv
kv -A pairs -s = <<'KV_INPUT'
alpha=one
beta=two words
KV_INPUT
printf 'kv=%s,%s status=%s\n' "${pairs[alpha]}" "${pairs[beta]}" "$?"

enable -f push push
outer_pid=$BASHPID
outer_dollar=$$
sleep 5 &
outer_async=$!
push
if true; then
    [[ -z $! ]] && async_cleared=1 || async_cleared=0
    [[ -z $(jobs -p) ]] && jobs_cleared=1 || jobs_cleared=0
    printf 'push=%s,%s,%s,%s,%s,%s,%s\n' "$SHLVL" "$((PPID == outer_pid))" "$(( $$ != outer_dollar ))" "$((BASHPID == $$))" "$BASH_SUBSHELL" "$async_cleared" "$jobs_cleared"
fi
cat <<'PUSH_INPUT'
push_heredoc=ok
PUSH_INPUT
exit 7
push_status=$?
printf 'push_status=%s\n' "$push_status"
kill "$outer_async" 2>/dev/null
wait "$outer_async" 2>/dev/null
"#;
    let expected = run_stdin(&oracle_bash_path(), script, &directory);
    let actual = run_stdin(
        &cherub_path().expect("CARGO_BIN_EXE_cherubsh"),
        script,
        &directory,
    );

    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn native_compatibility_does_not_need_a_bash_executable() {
    let shell = cherub_path().expect("CARGO_BIN_EXE_cherubsh");
    let output = Command::new(shell)
        .args(["--pretty-print", "-c", "printf native"])
        .env_clear()
        .env("HOME", "/tmp")
        .env("LANG", "C")
        .env("PATH", "/no/bash/here")
        .env("BASH_ORACLE_PATH", "/missing/bash")
        .output()
        .expect("run cherubsh");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "native");
    assert!(output.stderr.is_empty());
}
