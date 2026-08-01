#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cherubsh_common::{Environment, Span, VarKind, W_HASDOLLAR};
    use cherubsh_parser::WordDesc;

    use crate::{expand_word_list, ExpandError, ExpandFlags, NullRunner};

    #[derive(Default)]
    struct TestEnv {
        vars: BTreeMap<String, String>,
        arrays: BTreeMap<String, BTreeMap<i64, String>>,
        assoc: BTreeMap<String, BTreeMap<String, String>>,
        positionals: Vec<String>,
        posix: bool,
        nounset: bool,
        nocasematch: bool,
    }

    impl Environment for TestEnv {
        fn get(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }

        fn set(&mut self, name: &str, value: String) {
            self.vars.insert(name.to_string(), value);
        }

        fn unset(&mut self, name: &str) {
            self.vars.remove(name);
            self.assoc.remove(name);
        }

        fn exported(&self, _name: &str) -> bool {
            false
        }

        fn export(&mut self, _name: &str) {}

        fn positional(&self, index: usize) -> Option<String> {
            self.positionals.get(index).cloned()
        }

        fn positional_count(&self) -> usize {
            self.positionals.len().saturating_sub(1)
        }

        fn set_positionals(&mut self, params: Vec<String>) {
            self.positionals = params;
        }

        fn last_status(&self) -> i32 {
            0
        }

        fn set_last_status(&mut self, _status: i32) {}

        fn option(&self, name: &str) -> bool {
            match name {
                "posix" => self.posix,
                "nounset" => self.nounset,
                "nocasematch" => self.nocasematch,
                _ => false,
            }
        }

        fn all_var_names_with_prefix(&self, prefix: &str) -> Vec<String> {
            let mut out: Vec<String> = self
                .vars
                .keys()
                .chain(self.arrays.keys())
                .chain(self.assoc.keys())
                .filter(|name| name.starts_with(prefix))
                .cloned()
                .collect();
            out.sort();
            out.dedup();
            out
        }

        fn get_array_all(&self, name: &str) -> Option<Vec<(i64, String)>> {
            self.arrays.get(name).map(|items| {
                items
                    .iter()
                    .map(|(key, value)| (*key, value.clone()))
                    .collect()
            })
        }

        fn get_array_indexed(&self, name: &str, index: i64) -> Option<String> {
            self.arrays
                .get(name)
                .and_then(|items| items.get(&index))
                .cloned()
        }

        fn array_keys(&self, name: &str) -> Option<Vec<i64>> {
            self.arrays
                .get(name)
                .map(|items| items.keys().copied().collect())
        }

        fn array_len(&self, name: &str) -> usize {
            self.arrays.get(name).map(|items| items.len()).unwrap_or(0)
        }

        fn get_array_assoc(&self, name: &str, key: &str) -> Option<String> {
            self.assoc
                .get(name)
                .and_then(|items| items.get(key))
                .cloned()
        }

        fn assoc_all(&self, name: &str) -> Option<Vec<(String, String)>> {
            self.assoc.get(name).map(|items| {
                items
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
        }

        fn assoc_keys(&self, name: &str) -> Option<Vec<String>> {
            self.assoc
                .get(name)
                .map(|items| items.keys().cloned().collect())
        }

        fn assoc_len(&self, name: &str) -> usize {
            self.assoc.get(name).map(|items| items.len()).unwrap_or(0)
        }

        fn kind(&self, name: &str) -> VarKind {
            if self.assoc.contains_key(name) {
                VarKind::Assoc
            } else if self.arrays.contains_key(name) {
                VarKind::Indexed
            } else if self.vars.contains_key(name) {
                VarKind::Scalar
            } else {
                VarKind::Unset
            }
        }
    }

    fn expand_one(env: &mut TestEnv, text: &str) -> Vec<String> {
        let mut runner = NullRunner;
        let flags = if text.as_bytes().iter().any(|b| matches!(b, b'$' | b'`')) {
            W_HASDOLLAR
        } else {
            0
        };
        let words = expand_word_list(
            &[WordDesc {
                text: text.to_string(),
                flags,
                span: Span::dummy(),
                raw: None,
            }],
            env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap();
        words.into_iter().map(|word| word.text).collect()
    }

    fn expand_error(env: &mut TestEnv, text: &str) -> ExpandError {
        let mut runner = NullRunner;
        let flags = if text.as_bytes().iter().any(|b| matches!(b, b'$' | b'`')) {
            W_HASDOLLAR
        } else {
            0
        };
        expand_word_list(
            &[WordDesc {
                text: text.to_string(),
                flags,
                span: Span::dummy(),
                raw: None,
            }],
            env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err()
    }

    #[test]
    fn quoted_star_substring_joins_with_first_ifs_char() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "uv\x01\x01wx".into(), "uv\x01\x01wx".into()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#""${*:1:2}""#),
            vec!["uv\x01\x01wx uv\x01\x01wx"]
        );
        assert_eq!(expand_one(&mut env, r#""${*:1:0}""#), vec![""]);
    }

    #[test]
    fn quoted_at_substring_preserves_separate_fields() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "uv\x01\x01wx".into(), "uv\x01\x01wx".into()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#""${@:1:2}""#),
            vec!["uv\x01\x01wx", "uv\x01\x01wx"]
        );
    }

    #[test]
    fn assoc_scalar_substring_reads_key_zero() {
        let mut env = TestEnv::default();
        env.assoc.insert(
            "assoc".into(),
            BTreeMap::from([
                ("0".into(), "uv\x01\x01wx".into()),
                ("1".into(), "other".into()),
            ]),
        );

        assert_eq!(expand_one(&mut env, "${assoc:0:4}"), vec!["uv\x01\x01"]);
        assert_eq!(
            expand_one(&mut env, r#""${assoc:0:4}""#),
            vec!["uv\x01\x01"]
        );
    }

    #[test]
    fn assoc_subscript_skips_quoted_closing_bracket() {
        let mut env = TestEnv::default();
        env.assoc.insert(
            "myarray".into(),
            BTreeMap::from([("a]a".into(), "abc".into())]),
        );
        assert_eq!(expand_one(&mut env, r#"${myarray["a]a"]}"#), vec!["abc"]);
    }

    #[test]
    fn indexed_array_substring_uses_sparse_indexes() {
        let mut env = TestEnv::default();
        env.arrays.insert(
            "a".into(),
            BTreeMap::from([(1, "one".into()), (3, "three".into()), (10, "ten".into())]),
        );

        assert_eq!(
            expand_one(&mut env, r#""${a[@]:1}""#),
            vec!["one", "three", "ten"]
        );
        assert_eq!(
            expand_one(&mut env, r#""${a[@]:2:2}""#),
            vec!["three", "ten"]
        );
        assert_eq!(expand_one(&mut env, r#""${a[@]: -1}""#), vec!["ten"]);
    }

    #[test]
    fn assoc_length_uses_entry_count() {
        let mut env = TestEnv::default();
        env.assoc.insert(
            "m".into(),
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())]),
        );

        assert_eq!(expand_one(&mut env, "${#m[@]}"), vec!["2"]);
    }

    #[test]
    fn literal_remove_and_substitute_match_pattern_output() {
        let mut env = TestEnv::default();
        env.vars.insert("s".into(), "alpha_beta_gamma_delta".into());

        assert_eq!(
            expand_one(&mut env, "${s//alpha/omega}"),
            vec!["omega_beta_gamma_delta"]
        );
        assert_eq!(
            expand_one(&mut env, "${s#alpha_}"),
            vec!["beta_gamma_delta"]
        );
        assert_eq!(
            expand_one(&mut env, "${s%_delta}"),
            vec!["alpha_beta_gamma"]
        );
    }

    #[test]
    fn scalar_all_ref_substring_uses_scalar_value() {
        let mut env = TestEnv::default();
        env.vars.insert("var".into(), "blah".into());

        assert_eq!(expand_one(&mut env, r#""${var[@]:3}""#), vec!["h"]);
        assert_eq!(expand_one(&mut env, r#""${var[*]:3}""#), vec!["h"]);
        assert_eq!(expand_one(&mut env, r#""${var[@]:0:0}""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#""${var[*]:0:0}""#), vec![""]);

        env.vars.remove("var");
        assert_eq!(
            expand_one(&mut env, r#""${var[@]:3}""#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, r#""${var[*]:3}""#), vec![""]);
    }

    #[test]
    fn substring_offset_ternary_colon_is_not_length_separator() {
        let mut env = TestEnv::default();
        env.vars.insert("PARAM".into(), "abcdefg".into());

        assert_eq!(expand_one(&mut env, "${PARAM:1 ? 4 : 2}"), vec!["efg"]);
        assert_eq!(expand_one(&mut env, "${PARAM:1 ? 4 : 2:1}"), vec!["e"]);
        assert_eq!(expand_one(&mut env, "${PARAM: 4<5 ? 4 : 2}"), vec!["efg"]);
        assert_eq!(expand_one(&mut env, "${PARAM: 5>4 ? 4 : 2:1}"), vec!["e"]);
    }

    #[test]
    fn substring_arithmetic_diagnostics_match_bash() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "a".into()],
            ..Default::default()
        };
        env.vars.insert("PARAM".into(), "abcdefg".into());
        env.vars.insert("bad".into(), "}".into());

        let err = expand_error(&mut env, "${PARAM:bad}");
        assert!(matches!(
            err,
            ExpandError::ArithSyntax(msg)
                if msg == r#"PARAM: }: arithmetic syntax error: operand expected (error token is "}")"#
        ));

        let err = expand_error(&mut env, r#"${@:1:$(($# - 2))}"#);
        assert!(matches!(
            err,
            ExpandError::Other(msg) if msg == "$(($# - 2)): substring expression < 0"
        ));
    }

    #[test]
    fn malformed_nested_pattern_substitution_reports_outer_braces() {
        let mut env = TestEnv::default();
        env.vars.insert("c".into(), String::new());

        let err = expand_error(&mut env, r#"${c//${$(($#-1))}/x/}"#);
        assert!(matches!(
            err,
            ExpandError::BadSubstitution(msg) if msg == r#"${$(($#-1))}"#
        ));
    }

    #[test]
    fn parameter_patterns_honor_nocasematch() {
        let mut env = TestEnv {
            nocasematch: true,
            ..Default::default()
        };
        env.vars.insert("string".into(), "abcd".into());

        assert_eq!(expand_one(&mut env, "${string//A/z}"), vec!["zbcd"]);
        assert_eq!(expand_one(&mut env, "${string//BC/x}"), vec!["axd"]);
        assert_eq!(expand_one(&mut env, "${string//[BC]/x}"), vec!["axxd"]);
        assert_eq!(expand_one(&mut env, "${string//[bC]/x}"), vec!["axxd"]);
    }

    #[test]
    fn pattern_substitution_unclosed_bracket_still_finds_separator() {
        let mut env = TestEnv::default();
        env.vars.insert("var".into(), "[hello".into());

        assert_eq!(expand_one(&mut env, r#""${var//[/}""#), vec!["hello"]);
    }

    #[test]
    fn transform_declare_quotes_array_control_and_raw_bytes() {
        let mut env = TestEnv::default();
        env.arrays.insert(
            "array".into(),
            BTreeMap::from([(0, "x\u{1}y\u{7f}z".into())]),
        );
        assert_eq!(
            expand_one(&mut env, r#""${array[@]@A}""#),
            vec![r#"declare -a array=([0]=$'x\001y\177z')"#]
        );

        env.assoc.insert(
            "assoc".into(),
            BTreeMap::from([(
                "x\u{1}y\u{7f}z".into(),
                crate::quote::bytes_to_shell_string(&[b'a', 0xa2, b'b', 0x02, b'c']),
            )]),
        );
        assert_eq!(
            expand_one(&mut env, r#""${assoc[@]@A}""#),
            vec![r#"declare -A assoc=([$'x\001y\177z']=$'a\242b\002c' )"#]
        );
    }

    #[test]
    fn quoted_empty_parameter_preserves_field() {
        let mut env = TestEnv::default();
        env.vars.insert("empty".into(), String::new());

        assert_eq!(expand_one(&mut env, r#""$empty""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#"x"$empty"y"#), vec!["xy"]);
    }

    #[test]
    fn quoted_default_alt_treats_single_quotes_as_literals() {
        let mut env = TestEnv::default();
        env.vars.insert("set".into(), "value".into());

        assert_eq!(expand_one(&mut env, r#""${set:+'$set'}""#), vec!["'value'"]);
        assert_eq!(expand_one(&mut env, r#""${unset:-''}""#), vec!["''"]);
        assert_eq!(expand_one(&mut env, r#""${unset:-}""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#""${set:+}""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#""${set:+"\p"}""#), vec!["p"]);
    }

    #[test]
    fn quoted_default_alt_expands_top_level_dollar_quotes() {
        let mut env = TestEnv::default();

        assert_eq!(expand_one(&mut env, r#""${unset:-$'\t'}""#), vec!["\t"]);
        assert_eq!(expand_one(&mut env, r#""${unset:-$"hi"}""#), vec!["hi"]);
        assert_eq!(
            expand_one(&mut env, r#""${unset:-"$'\t'"}""#),
            vec![r#"$'t'"#]
        );
    }

    #[test]
    fn posix_quoted_default_alt_single_quote_does_not_protect_brace() {
        let mut env = TestEnv {
            posix: true,
            ..Default::default()
        };
        env.vars.insert("IFS".into(), " \t\n".into());

        assert_eq!(
            expand_one(&mut env, r#""${IFS+'bar} baz""#),
            vec!["'bar baz"]
        );
    }

    #[test]
    fn quoted_default_alt_line_continuation_is_removed() {
        let mut env = TestEnv {
            posix: true,
            ..Default::default()
        };
        env.vars.insert("IFS".into(), " \t\n".into());

        assert_eq!(
            expand_one(&mut env, "\"${IFS+foo \"b\\\nar\" baz}\""),
            vec!["foo bar baz"]
        );
    }

    #[test]
    fn legacy_dollar_bracket_arithmetic_expands() {
        let mut env = TestEnv::default();

        assert_eq!(expand_one(&mut env, "$[ 13 * 2 ]"), vec!["26"]);
        env.vars.insert("i".into(), "2".into());
        env.arrays
            .insert("a".into(), BTreeMap::from([(2, "20".into())]));
        assert_eq!(expand_one(&mut env, "$[ a[i] + 1 ]"), vec!["21"]);
    }

    #[test]
    fn positional_star_default_and_substring_preserve_empty_and_del() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), String::new()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#"${*-fallback}"#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, r#""${*:1}""#), vec![""]);

        env.positionals = vec!["shell".into(), "\x7f".into()];
        assert_eq!(expand_one(&mut env, r#"${*-fallback}"#), vec!["\x7f"]);
        assert_eq!(expand_one(&mut env, r#""${*:1}""#), vec!["\x7f"]);
    }

    #[test]
    fn quoted_at_preserves_empty_positional() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), String::new()],
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#""$@""#), vec![""]);
        assert_eq!(expand_one(&mut env, r#"${undef-"$@"}"#), vec![""]);

        env.positionals = vec!["shell".into()];
        assert_eq!(expand_one(&mut env, r#""${undef-$@}""#), vec![""]);
        assert_eq!(
            expand_one(&mut env, r#"${undef-"$@"}"#),
            Vec::<String>::new()
        );
        env.vars.insert("empty".into(), String::new());
        env.vars.insert("also_empty".into(), String::new());
        assert_eq!(expand_one(&mut env, r#""$empty$@""#), Vec::<String>::new());
        assert_eq!(
            expand_one(&mut env, r#""$empty$also_empty$@""#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, "\"\"$empty$@"), vec![""]);
    }

    #[test]
    fn quoted_at_alternate_preserves_set_empty_elements() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), String::new()],
            arrays: BTreeMap::from([("a".into(), BTreeMap::from([(0, String::new())]))]),
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#""${@:+nonnull}""#), vec![""]);
        env.positionals = vec!["shell".into()];
        assert_eq!(
            expand_one(&mut env, r#""${@:+nonnull}""#),
            Vec::<String>::new()
        );
        assert_eq!(expand_one(&mut env, r#""${a[@]:+nonnull}""#), vec![""]);
        env.arrays.clear();
        assert_eq!(
            expand_one(&mut env, r#""${a[@]:+nonnull}""#),
            Vec::<String>::new()
        );
    }

    #[test]
    fn simple_numeric_parameter_matches_bash() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "A".into()],
            ..Default::default()
        };
        assert_eq!(expand_one(&mut env, "$10"), vec!["A0"]);

        let mut env = TestEnv {
            positionals: vec!["shell".into()],
            nounset: true,
            ..Default::default()
        };
        let err = crate::expand_word_list(
            &[WordDesc {
                text: "$9".to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();
        assert!(matches!(err, ExpandError::UnboundVariable(name) if name == "$9"));

        let err = crate::expand_word_list(
            &[WordDesc {
                text: "${9}".to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();
        assert!(matches!(err, ExpandError::UnboundVariable(name) if name == "9"));
    }

    #[test]
    fn quoted_default_alt_bare_at_preserves_fields() {
        let mut env = TestEnv {
            positionals: vec![
                "shell".into(),
                "a b".into(),
                "c".into(),
                "d".into(),
                "e".into(),
                "f".into(),
            ],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#""${1+$@}""#),
            vec!["a b", "c", "d", "e", "f"]
        );
    }

    #[test]
    fn hash_special_parameter_operators_match_bash() {
        let mut env = TestEnv {
            positionals: vec![
                "shell".into(),
                "1".into(),
                "2".into(),
                "3".into(),
                "4".into(),
                "5".into(),
            ],
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#"${#:foo}"#), vec!["5"]);
        assert_eq!(expand_one(&mut env, r#"${#:-foo}"#), vec!["5"]);
        assert_eq!(expand_one(&mut env, r#"${#-posparams}"#), vec!["5"]);
        assert_eq!(expand_one(&mut env, r#"${#:-posparams}"#), vec!["5"]);

        env.positionals = vec!["shell".into()];
        assert_eq!(expand_one(&mut env, r#"${#:foo}"#), vec!["0"]);
        assert_eq!(expand_one(&mut env, r#"${#:-foo}"#), vec!["0"]);
        assert_eq!(expand_one(&mut env, r#"${#-posparams}"#), vec!["0"]);
        assert_eq!(
            expand_one(&mut env, r#"${!:-posparams}"#),
            vec!["posparams"]
        );
        assert_eq!(expand_one(&mut env, r#"${#-}"#), vec!["0"]);
        assert!(crate::expand_word_list(
            &[WordDesc {
                text: r#"${#:}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .is_err());
        assert!(crate::expand_word_list(
            &[WordDesc {
                text: r#"${#1xyz}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut NullRunner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .is_err());
    }

    #[test]
    fn literal_ifs_bytes_inside_words_do_not_split() {
        let mut env = TestEnv::default();
        env.vars.insert("IFS".into(), "+".into());
        env.vars.insert("x".into(), "a".into());
        env.vars.insert("y".into(), "b".into());

        assert_eq!(expand_one(&mut env, r#"$x+$y"#), vec!["a+b"]);
        assert_eq!(expand_one(&mut env, r#"+"$@""#), vec!["+"]);

        let mut runner = NullRunner;
        let words = expand_word_list(
            &[
                WordDesc {
                    text: "+".to_string(),
                    flags: 0,
                    span: Span::dummy(),
                    raw: None,
                },
                WordDesc {
                    text: r#""$@""#.to_string(),
                    flags: W_HASDOLLAR,
                    span: Span::dummy(),
                    raw: None,
                },
            ],
            &mut env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap();
        assert_eq!(
            words.into_iter().map(|word| word.text).collect::<Vec<_>>(),
            vec!["+"]
        );
    }

    #[test]
    fn ansi_c_quote_is_literal_inside_double_quotes() {
        let mut env = TestEnv::default();

        assert_eq!(expand_one(&mut env, r#""$'\x41'""#), vec![r#"$'\x41'"#]);
    }

    #[test]
    fn command_substitution_heredoc_delimiter_can_touch_closing_paren() {
        let src = b"cat <<EOF\nhere is the text\nEOF)";
        let (body, end) = super::extract_paren(src, 0).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "cat <<EOF\nhere is the text\nEOF"
        );
        assert_eq!(end, src.len());
    }

    #[test]
    fn command_substitution_heredoc_delimiter_removes_escaped_newline() {
        let src = b"cat <<\\EOT\\\n4\nd \\\ng\nEOT4\n)";
        let (body, end) = super::extract_paren(src, 0).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "cat <<\\EOT4\nd \\\ng\nEOT4\n"
        );
        assert_eq!(end, src.len());
    }

    #[test]
    fn command_substitution_double_quotes_track_nested_substitutions() {
        let src = br#"echo "foo$(echo ")")")"#;
        let (body, end) = super::extract_paren(src, 0).unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), r#"echo "foo$(echo ")")""#);
        assert_eq!(end, src.len());
    }

    #[test]
    fn parameter_brace_backticks_protect_closing_brace() {
        let src = br#"HOME:`echo }`}"#;
        let (body, end) = super::extract_brace_body(src, 0, false).unwrap();
        assert_eq!(String::from_utf8(body).unwrap(), r#"HOME:`echo }`"#);
        assert_eq!(end, src.len());
    }

    #[test]
    fn indirect_default_and_special_array_targets_match_bash() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "a".into(), "ef".into(), "op".into()],
            ..Default::default()
        };
        env.vars.insert("z".into(), "abcdefghijklmnop".into());
        env.vars.insert("ef".into(), "4".into());
        env.vars.insert("a".into(), String::new());

        assert_eq!(expand_one(&mut env, "${!9:-$z}"), vec!["abcdefghijklmnop"]);
        assert_eq!(expand_one(&mut env, "${!2}"), vec!["4"]);
        assert_eq!(expand_one(&mut env, "${!#}"), vec!["op"]);
        assert_eq!(expand_one(&mut env, "${!1:-$z}"), vec!["abcdefghijklmnop"]);
        assert_eq!(expand_one(&mut env, "${!1-$z}"), Vec::<String>::new());

        env.positionals = vec!["shell".into(), "a".into(), "b c".into(), "d".into()];
        env.vars.insert("foo".into(), "@".into());
        assert_eq!(expand_one(&mut env, "${!foo}"), vec!["a", "b", "c", "d"]);
        assert_eq!(expand_one(&mut env, r#""${!foo}""#), vec!["a", "b c", "d"]);
    }

    #[test]
    fn default_assignment_rejects_positional_parameters() {
        let mut env = TestEnv::default();
        let mut runner = NullRunner;
        let err = crate::expand_word_list(
            &[WordDesc {
                text: r#"${6=arg6}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();

        assert!(matches!(err, ExpandError::Other(msg) if msg == "$6: cannot assign in this way"));
    }

    #[test]
    fn quoted_indirect_prefix_star_and_at_match_bash() {
        let mut env = TestEnv::default();
        env.vars.insert("IFS".into(), "- \t\n".into());
        for name in [
            "_QUANTITY",
            "_QUART",
            "_QUEST",
            "_QUILL",
            "_QUOTA",
            "_QUOTE",
        ] {
            env.vars.insert(name.into(), String::new());
        }

        assert_eq!(
            expand_one(&mut env, r#""${!_Q*}""#),
            vec!["_QUANTITY-_QUART-_QUEST-_QUILL-_QUOTA-_QUOTE"]
        );
        assert_eq!(
            expand_one(&mut env, r#""${!_Q@}""#),
            vec![
                "_QUANTITY",
                "_QUART",
                "_QUEST",
                "_QUILL",
                "_QUOTA",
                "_QUOTE"
            ]
        );
        assert_eq!(expand_one(&mut env, r#"${!*}"#), Vec::<String>::new());

        let mut runner = NullRunner;
        let err = crate::expand_word_list(
            &[WordDesc {
                text: r#"${!1*}"#.to_string(),
                flags: W_HASDOLLAR,
                span: Span::dummy(),
                raw: None,
            }],
            &mut env,
            &mut runner,
            ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL,
        )
        .unwrap_err();
        assert!(matches!(err, ExpandError::BadSubstitution(msg) if msg == "${!1*}"));
    }

    #[test]
    fn positional_at_pattern_substitution_maps_each_arg() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "uv\x01\x01wx".into(), "uv\x01\x01wx".into()],
            ..Default::default()
        };

        assert_eq!(
            expand_one(&mut env, r#"${@/$'\001'/A}"#),
            vec!["uvA\x01wx", "uvA\x01wx"]
        );
        assert_eq!(
            expand_one(&mut env, r#""${@/w/W}""#),
            vec!["uv\x01\x01Wx", "uv\x01\x01Wx"]
        );
    }

    #[test]
    fn positional_star_pattern_substitution_uses_joined_scalar() {
        let mut env = TestEnv {
            positionals: vec!["shell".into(), "ax".into(), "ay".into()],
            ..Default::default()
        };

        assert_eq!(expand_one(&mut env, r#""${*/a/A}""#), vec!["Ax Ay"]);
        assert_eq!(expand_one(&mut env, r#"${*/a/A}"#), vec!["Ax", "Ay"]);
    }
}
