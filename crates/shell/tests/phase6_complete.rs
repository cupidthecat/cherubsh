//! Programmable-completion smoke tests.

use cherubsh_test_harness::{assert_parity, run_cherub, workspace_root, RunSpec};

#[test]
fn compgen_wordlist() {
    let out = run_cherub(&RunSpec {
        script: Some("compgen -W 'apple banana cherry' a"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert_eq!(out.stdout.trim(), "apple", "stdout={:?}", out.stdout);
}

#[test]
fn complete_register_and_print() {
    let out = run_cherub(&RunSpec {
        script: Some("complete -W 'a b c' mycmd; complete -p"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(
        out.stdout.contains("mycmd"),
        "expected mycmd in completion listing, got: {:?}",
        out.stdout
    );
}

#[test]
fn complete_remove() {
    let out = run_cherub(&RunSpec {
        script: Some("complete -W 'x' c; complete -r c; complete -p"),
        ..RunSpec::default()
    })
    .expect("run cherub");
    assert!(!out.stdout.contains(" c\n"), "spec for c should be gone");
}

#[test]
fn complete_remove_all_parity() {
    assert_parity(&RunSpec {
        script: Some("complete -W 'x' c; complete -f d; complete -r; complete -p"),
        ..RunSpec::default()
    });
}

#[test]
fn complete_double_dash_and_named_print_parity() {
    assert_parity(&RunSpec {
        script: Some("complete -f -- . source; complete -p . source"),
        ..RunSpec::default()
    });
}

#[test]
fn complete_missing_named_spec_parity() {
    assert_parity(&RunSpec {
        script: Some("complete -f cmd; complete -p cmd missing; echo print=$?; complete -r missing; echo remove=$?"),
        ..RunSpec::default()
    });
}

#[test]
fn compgen_shopt_names_parity() {
    assert_parity(&RunSpec {
        script: Some("compgen -A shopt globskipdots; compgen -A setopt pipefail"),
        ..RunSpec::default()
    });
}

#[test]
fn compgen_action_catalog_parity() {
    assert_parity(&RunSpec {
        script: Some(
            r#"
                export LC_ALL=C
                alias cherub_alias=':'
                cherub_function() { :; }
                CHERUB_SCALAR=value
                CHERUB_ARRAY=(one two)
                export CHERUB_EXPORT=value
                enable -n echo
                for request in \
                    'alias cherub' \
                    'arrayvar CHERUB' \
                    'binding beginning-of-l' \
                    'builtin comp' \
                    'command compg' \
                    'directory crates/bu' \
                    'disabled ec' \
                    'enabled comp' \
                    'export CHERUB' \
                    'file Cargo.t' \
                    'function cherub' \
                    'group ro' \
                    'helptopic comp' \
                    'hostname local' \
                    'keyword el' \
                    'service http' \
                    'setopt vi' \
                    'shopt extg' \
                    'signal SIGT' \
                    'user ro' \
                    'variable CHERUB'
                do
                    set -- $request
                    printf '<%s>\n' "$1"
                    compgen -A "$1" -- "$2"
                    printf 'status=%s\n' "$?"
                done
                sleep 2 &
                child=$!
                printf '%s\n' '<job>'
                compgen -A job sl
                printf '%s\n' '<running>'
                compgen -A running sl
                kill "$child"
                wait "$child" 2>/dev/null
                :
            "#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn compgen_generators_and_callbacks_parity() {
    assert_parity(&RunSpec {
        script: Some(
            r#"
                set +e
                value='red blue'
                compgen -G 'crates/*/Cargo.toml' zzz | sort
                printf 'glob=%s\n' "${PIPESTATUS[0]}"
                compgen -W '$value "two words" crates/*/Cargo.toml' t
                printf 'words=%s\n' "$?"
                compgen -W 'Alpha alpha beta' -X 'A*' ''
                compgen -W 'apple apricot berry' -X '!a*' ''
                callback() {
                    COMPREPLY=(
                        "args=$1|$2|$3"
                        "line=${COMP_LINE-unset}"
                        "point=${COMP_POINT-unset}"
                        "type=${COMP_TYPE-unset}"
                        "key=${COMP_KEY-unset}"
                        "cword=${COMP_CWORD-unset}"
                        "words=${COMP_WORDS[*]-unset}"
                    )
                }
                compgen -F callback word
                printf 'function=%s vars=%s/%s/%s/%s/%s/%s reply=%s\n' \
                    "$?" "${COMP_LINE-unset}" "${COMP_POINT-unset}" \
                    "${COMP_TYPE-unset}" "${COMP_KEY-unset}" \
                    "${COMP_CWORD-unset}" "${COMP_WORDS[*]-unset}" \
                    "${COMPREPLY[*]-unset}"
                compgen -C 'printf "args=%s|%s|%s\\nline=%s\\npoint=%s\\ntype=%s\\nkey=%s\\n" "$1" "$2" "$3" "$COMP_LINE" "$COMP_POINT" "$COMP_TYPE" "$COMP_KEY"' word
                printf 'command=%s vars=%s/%s/%s/%s/%s/%s\n' \
                    "$?" "${COMP_LINE-unset}" "${COMP_POINT-unset}" \
                    "${COMP_TYPE-unset}" "${COMP_KEY-unset}" \
                    "${COMP_CWORD-unset}" "${COMP_WORDS[*]-unset}"
                compgen
                printf 'empty=%s\n' "$?"
                for word in '$PA' '~ro' '@local' Cargo; do
                    printf '<%s>\n' "$word"
                    compgen -o bashdefault "$word"
                    printf 'bashdefault=%s\n' "$?"
                done
            "#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn compgen_file_wordlist_and_remove_edge_cases_parity() {
    assert_parity(&RunSpec {
        script: Some(
            r#"
                directory=$(mktemp -d)
                trap 'rm -rf "$directory"' EXIT
                cd "$directory" || exit
                touch .shell item1 foo.txt bar.txt
                mkdir .dir
                HOME=$PWD
                printf '%s\n' '<files>'
                compgen -A file | sort
                printf '%s\n' '<literal-wordlist>'
                compgen -W '*.txt' | sort
                printf '%s\n' '<quoted-tilde>'
                compgen -f '~/' | sort
                printf '%s\n' '<quoted-variable>'
                compgen -f '$HOME/' | sort
                complete -r missing
                printf 'remove-name=%s\n' "$?"
                complete -r -D
                printf 'remove-slot=%s\n' "$?"
            "#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn complete_slots_and_compopt_parity() {
    assert_parity(&RunSpec {
        script: Some(
            r#"
                set +e
                complete -A signal -a -A alias -o fullquote 'odd name'
                complete -p 'odd name'
                complete -D -W 'default one'
                complete -E -W 'empty one'
                complete -I -W 'initial one'
                complete -p -DEI ignored-name
                complete -W 'alpha beta' sample
                compopt sample
                compopt -o nospace +o default sample
                complete -p sample
                compopt -I
                compopt -I -o filenames ignored-name
                complete -p -I
                complete -r -E
                printf 'remove=%s\n' "$?"
                complete -p -E 2>/dev/null
                printf 'missing-slot=%s\n' "$?"
                compopt absent 2>/dev/null
                printf 'missing-name=%s\n' "$?"
            "#,
        ),
        ..RunSpec::default()
    });
}

#[test]
fn bash_completion_2_18_git_loader_parity() {
    let source = workspace_root().join("target/upstream/bash-completion-2.18.0/bash_completion");
    if !source.is_file() {
        assert!(
            std::env::var_os("RUN_PARITY_TESTS").is_none(),
            "missing bash-completion source at {}",
            source.display()
        );
        return;
    }
    let source = source.to_string_lossy().replace('\'', "'\\''");
    let script = format!(
        r#"
            source '{source}'
            printf 'source=%s loader=%s\n' "$?" "$(type -t _completion_loader)"
            _completion_loader git
            printf 'load=%s gitfn=%s\n' "$?" "$(type -t _comp_cmd_git)"
            complete -p git
            COMP_LINE='git ch'
            COMP_POINT=6
            COMP_TYPE=9
            COMP_KEY=9
            COMP_WORDS=(git ch)
            COMP_CWORD=1
            COMPREPLY=()
            __git_wrap__git_main git ch git
            printf 'reply=%s\n' "${{COMPREPLY[*]}}"
        "#
    );
    assert_parity(&RunSpec {
        script: Some(&script),
        ..RunSpec::default()
    });
}
