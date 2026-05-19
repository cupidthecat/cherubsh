//! Expansion context and supporting types. Threaded through every recursive
//! step of expansion.

use bitflags::bitflags;
use cherubsh_common::{Environment, ProcSubstDir};

use crate::error::ExpandError;
use crate::ifs::IfsState;

pub const EXPANSION_NESTING_MAX: u32 = 32;
pub const MAX_ARITH_NESTING: u32 = 64;

bitflags! {
    /// Drives which stages of the expansion pipeline fire and how quote
    /// removal behaves. Mirrors bash's mix of `W_*` flags and the call-site
    /// conventions in expand_word_unsplit / expand_words.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct ExpandFlags: u32 {
        const SPLIT_FIELDS   = 1 << 0;
        const EXPAND_GLOB    = 1 << 1;
        const QUOTE_REMOVAL  = 1 << 2;
        const ASSIGN_RHS     = 1 << 3;
        const FOR_REDIR      = 1 << 4;
        const FOR_CASE_PAT   = 1 << 5;
        const FOR_HERESTRING = 1 << 6;
        const FOR_ARITH      = 1 << 7;
        const NO_TILDE       = 1 << 8;
        const NO_BRACE       = 1 << 9;
    }
}

/// Handle for a process substitution started during expansion. The exec layer
/// keeps these around to `waitpid` after the consuming command finishes.
#[derive(Debug, Clone)]
pub struct ProcSubstHandle {
    pub path: String,
    pub pid: libc::pid_t,
    pub fd: libc::c_int,
}

/// Bridge between the expander and the rest of the shell. Implemented by exec
/// to break the expander↔exec circular crate dependency.
pub trait CommandRunner {
    /// Execute `src` in a subshell, capturing stdout. Trailing newlines are
    /// stripped per POSIX. Used by `$(cmd)` and backticks.
    fn run_subst(&mut self, env: &mut dyn Environment, src: &str) -> Result<Vec<u8>, ExpandError>;

    /// Execute legacy backquote command substitution. Bash reports diagnostics
    /// from this path with slightly different line accounting than `$()`.
    fn run_backquote_subst(
        &mut self,
        env: &mut dyn Environment,
        src: &str,
    ) -> Result<Vec<u8>, ExpandError> {
        self.run_subst(env, src)
    }

    /// Spawn an async command and return a filename whose I/O is wired to its
    /// stdin/stdout. Used by `<(cmd)` and `>(cmd)`. Default impl returns
    /// `ExpandError::Other` so callers without the feature still link.
    fn spawn_proc_subst(
        &mut self,
        _env: &mut dyn Environment,
        _dir: ProcSubstDir,
        _src: &str,
    ) -> Result<ProcSubstHandle, ExpandError> {
        Err(ExpandError::Other(
            "process substitution unsupported".into(),
        ))
    }
}

/// No-op runner used by callers that don't have command-substitution context
/// (e.g. unit tests, prompt expansion). Returns errors when a command/process
/// substitution is encountered.
#[derive(Default)]
pub struct NullRunner;

impl CommandRunner for NullRunner {
    fn run_subst(
        &mut self,
        _env: &mut dyn Environment,
        _src: &str,
    ) -> Result<Vec<u8>, ExpandError> {
        Err(ExpandError::Other(
            "command substitution unavailable in this context".into(),
        ))
    }
}

/// State threaded through every recursion of the expansion pipeline.
pub struct ExpCtx<'a> {
    pub env: &'a mut dyn Environment,
    pub runner: &'a mut dyn CommandRunner,
    pub ifs: IfsState,
    pub depth: u32,
    pub arith_depth: u32,
    pub assign_in_progress: bool,
    pub param_rhs_nosplit: bool,
    pub split_fields: bool,
    pub arith_expand_subscripts: bool,
    pub eval_unbound_error: bool,
    pub heredoc_context: bool,
    pub last_cmd_subst_status: i32,
    pub proc_subst: Vec<ProcSubstHandle>,
}

impl<'a> ExpCtx<'a> {
    pub fn new(env: &'a mut dyn Environment, runner: &'a mut dyn CommandRunner) -> Self {
        let nounset = env.option("nounset");
        let ifs = IfsState::from_env(env);
        Self {
            env,
            runner,
            ifs,
            depth: 0,
            arith_depth: 0,
            assign_in_progress: false,
            param_rhs_nosplit: false,
            split_fields: false,
            arith_expand_subscripts: true,
            eval_unbound_error: nounset,
            heredoc_context: false,
            last_cmd_subst_status: 0,
            proc_subst: Vec::new(),
        }
    }

    pub fn enter(&mut self) -> Result<(), ExpandError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > EXPANSION_NESTING_MAX {
            return Err(ExpandError::SubstitutionRecursive(self.depth));
        }
        Ok(())
    }

    pub fn leave(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
    }

    pub fn refresh_ifs(&mut self) {
        self.ifs.refresh(self.env);
    }
}
