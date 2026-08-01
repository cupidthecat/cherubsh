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
        ("validation.rs", "fn state_valid_name"),
        ("option_helpers.rs", "fn shellopts_value"),
        ("environment/variables.rs", "fn assign"),
        ("environment/input_runtime.rs", "fn next_shell_input_line"),
        (
            "environment/status_diagnostics.rs",
            "fn diagnostic_source_name",
        ),
        ("environment/options.rs", "fn option"),
        ("environment/runtime.rs", "fn last_async_pid"),
        ("environment/arrays.rs", "fn set_array"),
        ("environment/locals.rs", "fn resolve_nameref"),
        ("environment/aliases.rs", "fn alias_set"),
        ("environment/umask.rs", "fn umask_get"),
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
            "include!(\"validation.rs\");",
            "include!(\"option_helpers.rs\");",
        ]
    );

    let environment_files = [
        "variables",
        "input_runtime",
        "status_diagnostics",
        "options",
        "runtime",
        "arrays",
        "locals",
        "aliases",
        "umask",
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
    let expected_includes: Vec<_> = environment_files
        .iter()
        .map(|name| format!("include!(\"environment/{name}.rs\");"))
        .collect();
    assert_eq!(environment_includes, expected_includes);

    let environment_methods: Vec<_> = module
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("environment_") && line.ends_with("!();"))
        .collect();
    let expected_methods = [
        "environment_variable_accessors!();",
        "environment_input_runtime!();",
        "environment_variable_assignment!();",
        "environment_positionals!();",
        "environment_status_diagnostics!();",
        "environment_options!();",
        "environment_runtime!();",
        "environment_arrays!();",
        "environment_locals!();",
        "environment_aliases!();",
        "environment_umask!();",
        "environment_traps!();",
        "environment_commands!();",
        "environment_trap_actions!();",
        "environment_jobs!();",
        "environment_history!();",
        "environment_completion!();",
        "environment_signals!();",
    ];
    assert_eq!(environment_methods, expected_methods);

    let variables = fs::read_to_string(source.join("environment/variables.rs"))
        .expect("read variable environment methods");
    for misplaced in [
        "fn next_shell_input_line",
        "fn enter_loadable_child",
        "fn diagnostic_source_name",
        "fn set_last_status",
    ] {
        assert!(
            !variables.contains(misplaced),
            "environment/variables.rs still contains {misplaced}"
        );
    }

    let aliases = fs::read_to_string(source.join("environment/aliases.rs"))
        .expect("read alias environment methods");
    assert!(!aliases.contains("fn umask_get"));

    let signal_names =
        fs::read_to_string(source.join("signal_names.rs")).expect("read signal-name helpers");
    assert!(!signal_names.contains("fn state_valid_name"));
}
