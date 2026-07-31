//! `read` builtin parity tests.

use cherubsh_test_harness::{assert_parity_strict, RunSpec};

#[test]
fn read_ifs_backslash_and_remainder_parity() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"printf '%s\n' ' a  b\ ' | { read x y; printf '<%s>|<%s>\n' "$x" "$y"; }
printf '%s\n' '\ a  b\ ' | { read -r x y; printf '<%s>|<%s>\n' "$x" "$y"; }
printf 'ab\\\ncd\n' | { read; printf '<%s>\n' "$REPLY"; }
IFS=: read x y z <<'EOF'
:::
EOF
printf '<%s>|<%s>|<%s>\n' "$x" "$y" "$z""#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn read_timeout_and_invalid_number_parity() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"sleep 1 | { a=4; read -t 0.05 a; s=$?; case $s in 1??) echo timeout;; *) echo "$s";; esac; echo "${a:-unset}"; }
read -n -1 2>/dev/null; echo n=$?
read -t -3 foo 2>/dev/null; echo t=$?"#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn zero_timeout_checks_readiness_without_consuming_input() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"value=old
read -t 0 value <<< "ready"
printf 'scalar=%s value=<%s>\n' "$?" "$value"
REPLY=old-reply
read -t 0 <<< "ready"
printf 'reply=%s value=<%s>\n' "$?" "$REPLY"
items=(old values)
read -t 0 -a items <<< "ready"
printf 'array=%s value=<%s>\n' "$?" "${items[*]}"
coproc { read; }
fd=${COPROC[0]}
pid=$COPROC_PID
value=old
read -t 0 -u "$fd" value
printf 'pending=%s value=<%s>\n' "$?" "$value"
kill "$pid"
wait "$pid" 2>/dev/null
:"#,
        ),
        ..RunSpec::default()
    });
}
