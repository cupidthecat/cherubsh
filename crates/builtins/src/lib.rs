//! Built-in command implementations for cherubsh.
//!
//! Each builtin lives in its own module and registers a static implementor of
//! [`Builtin`]. The crate exposes [`lookup`] for dispatch from `cherubsh-exec`
//! and [`iter_builtins`] for `enable -a` style enumeration.

use bitflags::bitflags;
use cherubsh_common::Environment;
use cherubsh_parser::{Command, Redirect};

pub mod common;
pub mod getopt;
pub mod options;
pub mod shopt_table;

pub mod alias;
pub mod builtin_cmd;
pub mod caller;
pub mod colon;
pub mod command_cmd;
pub mod echo;
pub mod enable;
pub mod eval;
pub mod exec_cmd;
pub mod exit_cmd;
pub mod getopts;
pub mod hash;
pub mod help;
pub mod kill;
pub mod let_cmd;
pub mod mapfile;
pub mod times;
pub mod type_cmd;
pub mod ulimit;
pub mod umask;
pub mod unalias;

pub mod cd;
pub mod cond;
pub mod declare;
pub mod export;
pub mod printf;
pub mod read;
pub mod readonly;
pub mod set;
pub mod shopt;
pub mod source;
pub mod test_cmd;

pub mod break_cmd;
pub mod continue_cmd;
pub mod dirs;
pub mod local;
pub mod popd;
pub mod pushd;
pub mod return_cmd;
pub mod shift;
pub mod unset;

pub mod bg;
pub mod bind;
pub mod complete;
pub mod disown;
pub mod fc;
pub mod fg;
pub mod history;
pub mod jobs;
pub mod pwd;
pub mod suspend;
pub mod trap;
pub mod wait;

bitflags! {
    /// Flags describing a builtin per bash builtins.c metadata.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct BuiltinFlags: u32 {
        /// POSIX special builtin (assignments persist, errors abort posix mode).
        const SPECIAL    = 1 << 0;
        /// Accepts assignment-style arguments (declare/export/readonly/local/typeset).
        const ASSIGNMENT = 1 << 1;
        /// Localvar context (typeset/declare/local).
        const LOCALVAR   = 1 << 2;
        /// POSIX builtin (printed under `set -o posix` listings).
        const POSIX      = 1 << 3;
    }
}

/// Trait implemented by every builtin. Implementations are constructed as
/// static singletons (see each `<name>.rs`) and registered in [`lookup`].
pub trait Builtin: Sync {
    fn name(&self) -> &'static str;
    fn flags(&self) -> BuiltinFlags {
        BuiltinFlags::empty()
    }
    fn synopsis(&self) -> &'static str {
        ""
    }
    fn run(&self, ctx: &mut BuiltinCtx<'_>) -> i32;
}

/// Per-invocation context handed to each builtin.
pub struct BuiltinCtx<'a> {
    pub args: &'a [String],
    pub arg_flags: &'a [u32],
    pub assignments: &'a [(String, String)],
    pub redirects: &'a [Redirect],
    pub invoked_via_command: bool,
    pub shell: &'a mut dyn ShellOps,
}

impl<'a> BuiltinCtx<'a> {
    pub fn arg_flag(&self, index: usize) -> u32 {
        self.arg_flags.get(index).copied().unwrap_or(0)
    }

    pub fn env(&mut self) -> &mut dyn Environment {
        self.shell.env()
    }

    pub fn env_ref(&self) -> &dyn Environment {
        self.shell.env_ref()
    }
}

/// Operations on the live shell context (function table, control flow,
/// recursive command execution) that builtins need beyond the Environment
/// surface. Implemented by `cherubsh-exec`'s `ExecContext`.
pub trait ShellOps {
    fn env(&mut self) -> &mut dyn Environment;
    fn env_ref(&self) -> &dyn Environment;

    // Function table.
    fn function_define(&mut self, name: &str, body: Command);
    fn function_get(&self, name: &str) -> Option<Command>;
    fn function_remove(&mut self, name: &str) -> bool;
    fn function_names(&self) -> Vec<String>;
    fn function_set_trace(&mut self, _name: &str, _on: bool) {}
    fn function_is_trace(&self, _name: &str) -> bool {
        false
    }
    fn set_debug_trap_scope_active(&mut self, _on: bool) {}

    /// Parse + execute a script string in this shell (eval, source body).
    fn run_source(&mut self, src: &str) -> i32;
    fn run_source_named(&mut self, src: &str, _source_name: &str) -> i32 {
        self.run_source(src)
    }
    fn run_eval(&mut self, src: &str) -> i32 {
        self.run_source(src)
    }

    /// Execute an already-parsed command (used by `source` after parse).
    fn run_command(&mut self, cmd: &Command) -> i32;

    /// Mark the shell as pending an `exit`/`return`/`break`/`continue` unwind.
    fn request_exit(&mut self, status: i32);
    fn request_return(&mut self, status: i32);
    fn request_break(&mut self, levels: u32);
    fn request_continue(&mut self, levels: u32);

    /// Depth queries (for `return`, `break`, `continue`, `local` validation).
    fn function_depth(&self) -> u32;
    fn source_depth(&self) -> u32 {
        0
    }
    fn loop_depth(&self) -> u32;

    /// Evaluate an arithmetic expression. Used by `let` and `(( ))`.
    fn evaluate_arith(&mut self, expr: &str) -> Result<i64, String>;

    /// Apply an assignment using the live shell expansion context.
    fn apply_assignment_arg(&mut self, arg: &str, compound: bool) -> i32 {
        crate::common::apply_assignment_arg(self.env(), arg, compound)
    }

    /// Resolve a command name through hash + PATH. Mirrors exec's `search_path`.
    fn resolve_command(&mut self, name: &str) -> Option<std::path::PathBuf>;

    /// Push a fresh local-variable frame (function entry) - exposed for
    /// `source` when bash semantics call for it (here mostly a no-op).
    fn enter_subscope(&mut self) {}
    fn leave_subscope(&mut self) {}

    /// Record a local-variable snapshot in the active function frame.
    /// Returns false if there's no active frame.
    fn record_local(&mut self, _name: &str, _prior: Option<String>) -> bool {
        false
    }
    /// True iff the active function frame already has a snapshot for `name`.
    fn local_recorded(&self, _name: &str) -> bool {
        false
    }
    /// Note an explicit `export name`/`export name=value` in the active shell
    /// context. Function calls use this to decide whether a temporary prefix
    /// assignment should survive return.
    fn note_exported(&mut self, _name: &str) {}
    fn current_function_prefix_assignment(&self, _name: &str) -> bool {
        false
    }
}

/// Resolve a builtin by name. Returns `None` if no such builtin exists or if
/// the builtin has been disabled via `enable -n`.
pub fn lookup(name: &str) -> Option<&'static dyn Builtin> {
    BUILTINS.iter().copied().find(|b| b.name() == name)
}

/// Resolve including disabled state - for `enable`/`builtin` which need to
/// see all entries.
pub fn lookup_raw(name: &str) -> Option<&'static dyn Builtin> {
    BUILTINS.iter().copied().find(|b| b.name() == name)
}

pub fn iter_builtins() -> impl Iterator<Item = &'static dyn Builtin> {
    BUILTINS.iter().copied()
}

pub fn is_special(name: &str) -> bool {
    lookup_raw(name)
        .map(|b| b.flags().contains(BuiltinFlags::SPECIAL))
        .unwrap_or(false)
}

pub fn is_assignment_builtin(name: &str) -> bool {
    lookup_raw(name)
        .map(|b| b.flags().contains(BuiltinFlags::ASSIGNMENT))
        .unwrap_or(false)
}

/// Master registry. Order is informational; lookups are O(N) on builtin count
/// which is fine for ~50 entries.
static BUILTINS: &[&dyn Builtin] = &[
    &colon::COLON,
    &colon::TRUE,
    &colon::FALSE,
    &cd::CD,
    &pwd::PWD,
    &echo::ECHO,
    &exit_cmd::EXIT,
    &exit_cmd::LOGOUT,
    &export::EXPORT,
    &readonly::READONLY,
    &unset::UNSET,
    &return_cmd::RETURN,
    &break_cmd::BREAK,
    &continue_cmd::CONTINUE,
    &shift::SHIFT,
    &set::SET,
    &shopt::SHOPT,
    &local::LOCAL,
    &source::DOT,
    &source::SOURCE,
    &eval::EVAL,
    &exec_cmd::EXEC,
    &declare::DECLARE,
    &declare::TYPESET,
    &alias::ALIAS,
    &unalias::UNALIAS,
    &type_cmd::TYPE,
    &builtin_cmd::BUILTIN,
    &command_cmd::COMMAND,
    &caller::CALLER,
    &let_cmd::LET,
    &ulimit::ULIMIT,
    &umask::UMASK,
    &times::TIMES,
    &help::HELP,
    &getopts::GETOPTS,
    &hash::HASH,
    &kill::KILL,
    &enable::ENABLE,
    &mapfile::MAPFILE,
    &mapfile::READARRAY,
    &printf::PRINTF,
    &read::READ,
    &test_cmd::TEST,
    &test_cmd::LBRACKET,
    &pushd::PUSHD,
    &popd::POPD,
    &dirs::DIRS,
    &trap::TRAP,
    &jobs::JOBS,
    &fg::FG,
    &bg::BG,
    &wait::WAIT,
    &disown::DISOWN,
    &suspend::SUSPEND,
    &bind::BIND,
    &complete::COMPLETE,
    &complete::COMPGEN,
    &complete::COMPOPT,
    &fc::FC,
    &history::HISTORY,
];
