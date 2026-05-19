//! Coprocess parity tests.

use cherubsh_test_harness::{assert_parity, RunSpec};

fn parity(script: &str) {
    assert_parity(&RunSpec {
        script: Some(script),
        ..RunSpec::default()
    });
}

#[test]
fn unnamed_coproc_exposes_read_fd_and_pid() {
    parity(
        "coproc { printf 'ok\\n'; }; read -u ${COPROC[0]} line; echo \"$line\"; wait $COPROC_PID",
    );
}

#[test]
fn named_coproc_exposes_named_fd_array() {
    parity("coproc worker { printf 'named\\n'; }; read -u ${worker[0]} line; echo \"$line\"; wait $worker_PID");
}
