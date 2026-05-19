//! Cross-crate signal-handling primitives.
//!
//! The shell crate owns the actual `sigaction` installations and the global
//! atomic pending-counter array. This module just defines the surface the
//! rest of the workspace agrees on: signal numbers, trap kinds, and the
//! `SignalMaskGuard` RAII helper that mirrors bash's BLOCK_CHILD /
//! UNBLOCK_CHILD macro pairs.

pub const NSIG: usize = 64;

/// All distinct trap "signals" bash recognises, including the pseudo-signals
/// EXIT/ERR/DEBUG/RETURN.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrapKind {
    Numeric(i32),
    Exit,
    Err,
    Debug,
    Return,
}

impl TrapKind {
    pub fn parse(name: &str) -> Option<Self> {
        let upper = name.to_ascii_uppercase();
        let stripped = upper.strip_prefix("SIG").unwrap_or(&upper);
        Some(match stripped {
            "EXIT" | "0" => TrapKind::Exit,
            "ERR" => TrapKind::Err,
            "DEBUG" => TrapKind::Debug,
            "RETURN" => TrapKind::Return,
            _ => {
                if let Ok(n) = stripped.parse::<i32>() {
                    if n <= 0 || n as usize >= NSIG {
                        return None;
                    }
                    TrapKind::Numeric(n)
                } else {
                    // signal short name → number lookup; delegate to lib.rs
                    return None;
                }
            }
        })
    }

    pub fn as_signal(self) -> Option<i32> {
        match self {
            TrapKind::Numeric(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_str(self) -> std::borrow::Cow<'static, str> {
        match self {
            TrapKind::Exit => "EXIT".into(),
            TrapKind::Err => "ERR".into(),
            TrapKind::Debug => "DEBUG".into(),
            TrapKind::Return => "RETURN".into(),
            TrapKind::Numeric(n) => n.to_string().into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrapAction {
    /// SIG_DFL - restore default disposition.
    Default,
    /// SIG_IGN - keep but discard.
    Ignore,
    /// User-supplied shell text re-parsed at each dispatch.
    Command(String),
}

impl TrapAction {
    pub fn is_default(&self) -> bool {
        matches!(self, TrapAction::Default)
    }
    pub fn is_ignore(&self) -> bool {
        matches!(self, TrapAction::Ignore)
    }
    pub fn command(&self) -> Option<&str> {
        match self {
            TrapAction::Command(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// RAII guard over `sigprocmask(SIG_BLOCK, mask, &old)`. Used pervasively in
/// job-table mutators to defer SIGCHLD until the mutation completes.
pub struct SignalMaskGuard {
    saved: libc::sigset_t,
}

impl SignalMaskGuard {
    /// Block SIGCHLD only (bash BLOCK_CHILD).
    pub fn block_sigchld() -> Self {
        Self::block_signals(&[libc::SIGCHLD])
    }

    /// Block SIGCHLD + SIGTTOU + SIGTTIN + SIGTSTP - used around `tcsetpgrp`.
    pub fn block_terminal_handoff() -> Self {
        Self::block_signals(&[libc::SIGCHLD, libc::SIGTTOU, libc::SIGTTIN, libc::SIGTSTP])
    }

    pub fn block_signals(sigs: &[libc::c_int]) -> Self {
        let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut saved: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut set);
            for s in sigs {
                libc::sigaddset(&mut set, *s);
            }
            libc::sigprocmask(libc::SIG_BLOCK, &set, &mut saved);
        }
        Self { saved }
    }
}

impl Drop for SignalMaskGuard {
    fn drop(&mut self) {
        unsafe {
            libc::sigprocmask(libc::SIG_SETMASK, &self.saved, std::ptr::null_mut());
        }
    }
}

/// Suspend the calling thread until a signal arrives, with `mask` as the
/// active blocked-signal set (used in `wait` builtin). Returns when any
/// signal is delivered.
pub fn sigsuspend_empty() {
    unsafe {
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        libc::sigsuspend(&empty);
    }
}
