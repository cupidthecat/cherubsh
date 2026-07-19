//! Array target parity for assignment-style builtins.

use cherubsh_test_harness::{assert_parity_strict, RunSpec};

#[test]
fn declare_and_unset_indexed_elements_preserve_scalar_slot() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"unset a b c
a=abcde
a[2]=bdef
declare -a b[256]
declare -r c[100]
declare a["7 + 8"]="test 2"
unset a[2]
printf 'a0=<%s> a2=<%s> a15=<%s>\n' "$a" "${a[2]-}" "${a[15]}"
declare -p b c a"#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn read_and_printf_v_assign_indexed_elements() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"printf x | { read r[2]; declare -p r; }
printf -v p[3] '%s' y
declare -p p"#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn nounset_arithmetic_treats_declared_array_elements_as_zero() {
    assert_parity_strict(&RunSpec {
        script: Some(
            r#"set -u
declare -a indexed=()
declare -A assoc=()
printf 'before:%d:%d\n' "$((indexed[2]))" "$((assoc[key]))"
((indexed[2] += 3))
((assoc[key] += 4))
printf 'after:%d:%d\n' "${indexed[2]}" "${assoc[key]}""#,
        ),
        ..RunSpec::default()
    });
}
