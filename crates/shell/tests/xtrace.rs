use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cherubsh_test_harness::cherub_path;

#[test]
fn ps4_command_substitution_does_not_recurse() {
    let mut child = Command::new(cherub_path().expect("find cherubsh test binary"))
        .args([
            "--norc",
            "--noprofile",
            "-c",
            "set -x; PS4='+$(echo trace) '; echo ok",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start cherubsh");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().expect("poll cherubsh").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PS4 command substitution did not finish within three seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }

    let output = child.wait_with_output().expect("collect cherubsh output");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "+ PS4='+$(echo trace) '\n+trace echo ok\n"
    );
}
