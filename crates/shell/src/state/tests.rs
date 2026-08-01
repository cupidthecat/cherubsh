#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn random_matches_bash_seeded_sequence() {
        let mut state = ShellState::default();
        state.assign("RANDOM", "42".into()).unwrap();

        assert_eq!(state.get("RANDOM").as_deref(), Some("17772"));
        assert_eq!(state.get("RANDOM").as_deref(), Some("26794"));
        assert_eq!(state.get("RANDOM").as_deref(), Some("1435"));
        assert_eq!(state.get("RANDOM").as_deref(), Some("24388"));
    }

    #[test]
    fn declare_array_preserves_existing_scalar_as_element_zero() {
        let mut state = ShellState::default();
        state.assign("a", "abcde".into()).unwrap();

        state.set_attr("a", VarAttrs::ARRAY, true);

        assert_eq!(state.kind("a"), VarKind::Indexed);
        assert_eq!(state.get("a").as_deref(), Some("abcde"));
        assert_eq!(state.get_array_indexed("a", 0).as_deref(), Some("abcde"));
    }

    #[test]
    fn repeated_array_attr_does_not_seed_empty_element() {
        let mut state = ShellState::default();

        state.set_attr("a", VarAttrs::ARRAY, true);
        state.set_attr("a", VarAttrs::ARRAY, true);

        assert_eq!(state.kind("a"), VarKind::Indexed);
        assert_eq!(state.get_array_indexed("a", 0), None);
    }

    #[test]
    fn indexed_array_dense_sparse_and_unset_semantics() {
        let mut state = ShellState::default();

        state.set_array("a", vec!["zero".into(), "one".into()]);
        state.set_array_indexed("a", 10_000, "far".into());

        assert_eq!(state.array_len("a"), 3);
        assert_eq!(state.array_max_index("a"), Some(10_000));
        assert_eq!(state.array_keys("a"), Some(vec![0, 1, 10_000]));
        assert_eq!(
            state.get_array_all("a"),
            Some(vec![
                (0, "zero".into()),
                (1, "one".into()),
                (10_000, "far".into()),
            ])
        );

        state.unset_array_elem("a", "1");
        assert_eq!(state.array_len("a"), 2);
        assert_eq!(state.array_keys("a"), Some(vec![0, 10_000]));
        assert_eq!(state.get("a").as_deref(), Some("zero"));
    }

    #[test]
    fn indexed_array_negative_subscript_uses_max_index() {
        let mut state = ShellState::default();
        state.set_array("a", vec!["zero".into(), "one".into(), "two".into()]);

        let max = state.array_max_index("a").unwrap();
        let resolved = max + 1 - 1;

        assert_eq!(
            state.get_array_indexed("a", resolved).as_deref(),
            Some("two")
        );
    }

    #[test]
    fn assoc_len_is_direct() {
        let mut state = ShellState::default();
        state.set_array_assoc("m", "k1", "v1".into());
        state.set_array_assoc("m", "k2", "v2".into());

        assert_eq!(state.assoc_len("m"), 2);
    }

    #[test]
    fn assoc_key_zero_is_scalar_value() {
        let mut state = ShellState::default();
        state.set_attr("m", VarAttrs::ASSOC, true);
        state.set_array_assoc("m", "0", "zero".into());

        assert_eq!(state.get("m").as_deref(), Some("zero"));
        assert_eq!(state.get_array_assoc("m", "0").as_deref(), Some("zero"));
    }
}

