use std::fs;
use std::path::PathBuf;

#[test]
fn shell_state_has_focused_responsibility_files() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/state");
    let expected = [
        ("mod.rs", "impl Environment for ShellState"),
        ("runtime.rs", "fn current_epoch_seconds"),
        ("variables.rs", "pub struct VariableEntry"),
        ("arrays.rs", "pub struct IndexedArray"),
        ("model.rs", "pub struct ShellState"),
        ("variable_helpers.rs", "fn special_scalar_attrs"),
        ("tests.rs", "mod tests"),
        ("signal_names.rs", "pub fn canonical_trap_signal"),
        ("option_helpers.rs", "fn shellopts_value"),
        ("environment/variables.rs", "fn assign"),
        ("environment/options.rs", "fn option"),
        ("environment/runtime.rs", "fn last_async_pid"),
        ("environment/arrays.rs", "fn set_array"),
        ("environment/locals.rs", "fn resolve_nameref"),
        ("environment/aliases.rs", "fn alias_set"),
        ("environment/traps.rs", "fn trap_set"),
        ("environment/commands.rs", "fn hash_set"),
        ("environment/trap_actions.rs", "fn trap_action"),
        ("environment/jobs.rs", "fn jobs_table"),
        ("environment/history.rs", "fn history"),
        ("environment/completion.rs", "fn compspec_set"),
        ("environment/signals.rs", "fn pending_signal_take"),
    ];

    for (name, responsibility_anchor) in expected {
        let contents = fs::read_to_string(source.join(name))
            .unwrap_or_else(|error| panic!("read shell state file {name}: {error}"));
        assert!(
            contents.lines().count() < 900,
            "shell state file {name} is still too large"
        );
        assert!(
            contents.contains(responsibility_anchor),
            "shell state file {name} lost {responsibility_anchor}"
        );
    }

    let module = fs::read_to_string(source.join("mod.rs")).expect("read shell state module root");
    let root_includes: Vec<_> = module
        .lines()
        .filter(|line| line.starts_with("include!(") && !line.contains("environment/"))
        .collect();
    assert_eq!(
        root_includes,
        [
            "include!(\"runtime.rs\");",
            "include!(\"variables.rs\");",
            "include!(\"arrays.rs\");",
            "include!(\"model.rs\");",
            "include!(\"variable_helpers.rs\");",
            "include!(\"tests.rs\");",
            "include!(\"signal_names.rs\");",
            "include!(\"option_helpers.rs\");",
        ]
    );

    let environment_order = [
        "variables",
        "options",
        "runtime",
        "arrays",
        "locals",
        "aliases",
        "traps",
        "commands",
        "trap_actions",
        "jobs",
        "history",
        "completion",
        "signals",
    ];
    let environment_includes: Vec<_> = module
        .lines()
        .filter(|line| line.starts_with("include!(\"environment/"))
        .collect();
    let expected_includes: Vec<_> = environment_order
        .iter()
        .map(|name| format!("include!(\"environment/{name}.rs\");"))
        .collect();
    assert_eq!(environment_includes, expected_includes);

    let environment_methods: Vec<_> = module
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("environment_") && line.ends_with("!();"))
        .collect();
    let expected_methods: Vec<_> = environment_order
        .iter()
        .map(|name| format!("environment_{name}!();"))
        .collect();
    assert_eq!(environment_methods, expected_methods);
}
