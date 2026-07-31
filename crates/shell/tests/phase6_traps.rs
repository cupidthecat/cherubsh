//! Trap parity tests.

use cherubsh_test_harness::{assert_parity, RunSpec};

fn parity(script: &str) {
    assert_parity(&RunSpec {
        script: Some(script),
        ..RunSpec::default()
    });
}

#[test]
fn exit_trap_fires() {
    parity("trap 'echo BYE' EXIT");
}

#[test]
fn exit_trap_with_explicit_exit() {
    parity("trap 'echo BYE' EXIT; exit 7");
}

#[test]
fn ignore_trap() {
    parity("trap '' USR1; kill -USR1 $$; echo ok");
}

#[test]
fn trap_reset_to_default() {
    parity("trap 'echo INT' INT; trap - INT; echo done");
}

#[test]
fn trap_print() {
    parity("trap 'echo BYE' EXIT; trap -p");
}

#[test]
fn trap_print_uses_bash_signal_names_without_numeric_duplicates() {
    parity("trap 'echo EXIT' 0; trap 'echo HUP' 1; trap 'echo INT' INT; trap '' TERM; trap");
}

#[test]
fn trap_single_signal_argument_resets_not_prints() {
    parity("trap 'echo HUP' HUP; trap HUP; trap -p HUP; echo done");
}

#[test]
fn trap_l_lists_signals() {
    parity("trap -l | head -3");
}

#[test]
fn numeric_signal_trap_runs_before_next_command() {
    parity("trap 'echo USR1' USR1; kill -USR1 $$; echo after");
}

#[test]
fn err_trap_fires_for_simple_failure() {
    parity("trap 'echo ERR' ERR; false; echo after");
}

#[test]
fn err_trap_in_function_requires_errtrace() {
    parity("trap 'echo ERR' ERR; f(){ false; echo inner; }; f; echo after");
    parity("set -E; trap 'echo ERR' ERR; f(){ false; echo inner; }; f; echo after");
}

#[test]
fn debug_trap_fires_without_recursing() {
    parity("trap 'echo DEBUG' DEBUG; echo hi");
    parity("trap 'echo DEBUG' DEBUG; f(){ echo body; }; f; echo after");
}

#[test]
fn return_trap_requires_functrace_for_functions() {
    parity("trap 'echo RETURN' RETURN; f(){ echo body; }; f; echo after");
    parity("set -T; trap 'echo RETURN' RETURN; f(){ echo body; }; f; echo after");
}

#[test]
fn coproc_inherits_debug_trap_without_running_it_internally() {
    parity(
        r#"trap 'echo "[debug]"' DEBUG
coproc { trap -p DEBUG; echo "coproc"; read; }
read_fd=${COPROC[0]}
write_fd=${COPROC[1]}
exec {drain_fd}<&"$read_fd"
read -u "$read_fd" trap_line
read -u "$read_fd" coproc_line
printf '%s\n' "$trap_line" "$coproc_line"
printf '\n' >&$write_fd
cat <&$drain_fd
wait"#,
    );
}
