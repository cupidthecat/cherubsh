use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use cherubsh_common::{
    completion::{CompSlot, CompSpec},
    history::{HistControl, HistoryTable},
    jobs::JobTable,
    keymap::{EditAction, Keymap},
    signals::{TrapAction, TrapKind},
    AssignError, Environment, FastHashMap as HashMap, FastHashSet as HashSet, TrapEntry, VarAttrs,
    VarKind, VarSnapshot,
};
use cherubsh_lineedit::LineEditor;

use crate::input::BashInput;

include!("runtime.rs");
include!("variables.rs");
include!("arrays.rs");
include!("model.rs");
include!("variable_helpers.rs");
include!("tests.rs");
include!("environment/variables.rs");
include!("environment/options.rs");
include!("environment/runtime.rs");
include!("environment/arrays.rs");
include!("environment/locals.rs");
include!("environment/aliases.rs");
include!("environment/traps.rs");
include!("environment/commands.rs");
include!("environment/trap_actions.rs");
include!("environment/jobs.rs");
include!("environment/history.rs");
include!("environment/completion.rs");
include!("environment/signals.rs");

impl Environment for ShellState {
    environment_variables!();
    environment_options!();
    environment_runtime!();
    environment_arrays!();
    environment_locals!();
    environment_aliases!();
    environment_traps!();
    environment_commands!();
    environment_trap_actions!();
    environment_jobs!();
    environment_history!();
    environment_completion!();
    environment_signals!();
}

include!("signal_names.rs");
include!("option_helpers.rs");
