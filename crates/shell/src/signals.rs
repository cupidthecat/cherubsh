//! Signal handler installation and atomic pending-counter array.
//!
//! Mirrors bash-5.2.21 trap.c / sig.c / jobs.c signal layer:
//! - Handlers are async-signal-safe; they only `fetch_add` into a counter.
//! - `check_signals()` converts SIGINT/SIGALRM/SIGTERM into `ShellJump`.
//! - `pending_signal_take()` drains a counter for trap dispatch.
//! - `acquire_terminal` initializes the bash `set_job_control` tty handoff;
//!   exec-side helpers transfer and restore the foreground process group.
//!
//! Each signal we care about gets a `sigaction(SA_RESTART)` install. The
//! actual dispatch lives in `crates/shell/src/traps.rs`.

use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use cherubsh_common::signals::{SignalMaskGuard, NSIG};
use cherubsh_common::{ShellJump, ShellResult};

/// Async-signal-safe pending counters, one slot per signal number. Indexed
/// 1..NSIG (slot 0 is unused / reserved for EXIT trap below).
static PENDING_COUNTS: [AtomicU32; NSIG] = {
    // Const init: array of NSIG zeroed AtomicU32. Rust 1.79+ allows this via
    // an explicit array expression with `AtomicU32::new(0)` since AtomicU32
    // is `const`-constructible.
    [const { AtomicU32::new(0) }; NSIG]
};

/// Set whenever any signal arrives - fast-path check.
static CATCH_FLAG: AtomicBool = AtomicBool::new(false);

/// Mirrors bash's `terminating_signal`. Non-zero means a fatal signal was
/// received and the shell should exit with 128+sig once safe.
static TERM_RECEIVED: AtomicI32 = AtomicI32::new(0);

/// Signals inherited as SIG_IGN when the shell started. Bash preserves these:
/// a child shell cannot trap or reset them later.
static STARTUP_IGNORED_KNOWN: [AtomicBool; NSIG] = [const { AtomicBool::new(false) }; NSIG];
static STARTUP_IGNORED: [AtomicBool; NSIG] = [const { AtomicBool::new(false) }; NSIG];

/// Mirrors bash's `interrupt_state`. Cleared by `check_signals()` once the
/// SIGINT has been converted into a `ShellJump::Discard`.
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

/// SIGALRM tally (TMOUT timer).
static ALARM_FIRED: AtomicBool = AtomicBool::new(false);

/// SIGWINCH tally - drained by the prompt code to refresh COLUMNS/LINES.
static WINCH_FIRED: AtomicBool = AtomicBool::new(false);

extern "C" fn generic_counter_handler(sig: libc::c_int) {
    if sig > 0 && (sig as usize) < NSIG {
        PENDING_COUNTS[sig as usize].fetch_add(1, Ordering::SeqCst);
    }
    CATCH_FLAG.store(true, Ordering::SeqCst);
    match sig {
        libc::SIGINT => {
            SIGINT_RECEIVED.store(true, Ordering::SeqCst);
        }
        libc::SIGALRM => {
            ALARM_FIRED.store(true, Ordering::SeqCst);
        }
        libc::SIGWINCH => {
            WINCH_FIRED.store(true, Ordering::SeqCst);
        }
        libc::SIGTERM | libc::SIGHUP | libc::SIGQUIT => {
            TERM_RECEIVED.store(sig, Ordering::SeqCst);
        }
        _ => {}
    }
}

fn install(signum: libc::c_int, handler: extern "C" fn(libc::c_int), flags: libc::c_int) {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = flags;
        libc::sigaction(signum, &sa, std::ptr::null_mut());
    }
}

fn install_default(signum: libc::c_int) {
    unsafe {
        libc::signal(signum, libc::SIG_DFL);
    }
}

fn install_ignore(signum: libc::c_int) {
    unsafe {
        libc::signal(signum, libc::SIG_IGN);
    }
}

fn ensure_startup_ignored(signum: libc::c_int) {
    if signum <= 0 || (signum as usize) >= NSIG || matches!(signum, libc::SIGKILL | libc::SIGSTOP) {
        return;
    }
    let index = signum as usize;
    if STARTUP_IGNORED_KNOWN[index].load(Ordering::Acquire) {
        return;
    }
    STARTUP_IGNORED[index].store(current_signal_is_ignored(signum), Ordering::Release);
    STARTUP_IGNORED_KNOWN[index].store(true, Ordering::Release);
}

fn current_signal_is_ignored(signum: libc::c_int) -> bool {
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(signum, std::ptr::null(), &mut old) != 0 {
            return false;
        }
        old.sa_sigaction == libc::SIG_IGN
    }
}

pub fn signal_ignored_at_start(signum: libc::c_int) -> bool {
    ensure_startup_ignored(signum);
    signum > 0
        && (signum as usize) < NSIG
        && STARTUP_IGNORED[signum as usize].load(Ordering::Acquire)
}

pub fn startup_ignored_signals() -> Vec<i32> {
    (1..NSIG as i32)
        .filter(|sig| *sig != libc::SIGPIPE)
        .filter(|sig| signal_ignored_at_start(*sig))
        .collect()
}

fn signal_is_trappable(signum: libc::c_int) -> bool {
    signum > 0 && (signum as usize) < NSIG && signum != libc::SIGKILL && signum != libc::SIGSTOP
}

/// Install the always-on handlers: SIGINT/SIGALRM/SIGTERM/SIGHUP.
/// Non-interactive shells leave INT/TERM at their default (so `kill` from a
/// parent terminates us promptly).
pub fn install_default_handlers(interactive: bool) {
    let flags = libc::SA_RESTART;
    if interactive {
        install_baseline(libc::SIGINT, Some((generic_counter_handler, flags)));
        install_baseline(libc::SIGTERM, Some((generic_counter_handler, flags)));
        install_baseline(libc::SIGHUP, Some((generic_counter_handler, flags)));
        install_baseline(libc::SIGQUIT, Some((generic_counter_handler, flags)));
    } else {
        // `install_early_sigint` runs before option parsing, so non-interactive
        // shells must restore SIGINT. TERM/HUP/QUIT have not been touched yet;
        // leaving their inherited disposition alone is already the baseline.
        install_baseline(libc::SIGINT, None);
    }
    install_baseline(libc::SIGALRM, Some((generic_counter_handler, flags)));
    install_baseline(libc::SIGUSR1, None);
    install_baseline(libc::SIGUSR2, None);
}

fn install_baseline(
    signum: libc::c_int,
    handler: Option<(extern "C" fn(libc::c_int), libc::c_int)>,
) {
    if signal_ignored_at_start(signum) {
        install_ignore(signum);
    } else if let Some((handler, flags)) = handler {
        install(signum, handler, flags);
    } else {
        install_default(signum);
    }
}

/// Install SIGCHLD/SIGTSTP/SIGTTIN/SIGTTOU/SIGWINCH for interactive shells
/// that have acquired the terminal. Must be called AFTER `acquire_terminal`.
pub fn install_job_control_signals() {
    let flags = libc::SA_RESTART;
    ensure_startup_ignored(libc::SIGCHLD);
    ensure_startup_ignored(libc::SIGWINCH);
    ensure_startup_ignored(libc::SIGTSTP);
    ensure_startup_ignored(libc::SIGTTIN);
    ensure_startup_ignored(libc::SIGTTOU);
    install(libc::SIGCHLD, generic_counter_handler, flags);
    install(libc::SIGWINCH, generic_counter_handler, flags);
    install_ignore(libc::SIGTSTP);
    install_ignore(libc::SIGTTIN);
    install_ignore(libc::SIGTTOU);
}

/// Reconcile a user-visible `trap` action with the kernel disposition.
///
/// A command trap needs the counting handler so dispatch can happen at the
/// next safe point. An ignored trap uses SIG_IGN immediately. Clearing a trap
/// restores the shell's baseline disposition rather than blindly using
/// SIG_DFL for signals the interactive/job-control shell must continue to
/// observe internally.
pub fn configure_trap_signal(
    signum: libc::c_int,
    action: Option<&cherubsh_common::TrapAction>,
    interactive: bool,
    job_control: bool,
) -> bool {
    if !signal_is_trappable(signum) {
        return false;
    }
    if signal_ignored_at_start(signum) {
        install_ignore(signum);
        return false;
    }
    match action {
        Some(cherubsh_common::TrapAction::Ignore) => install_ignore(signum),
        Some(cherubsh_common::TrapAction::Command(_)) => {
            install(signum, generic_counter_handler, libc::SA_RESTART);
        }
        Some(cherubsh_common::TrapAction::Default) | None => {
            if signum == libc::SIGALRM
                || (job_control && matches!(signum, libc::SIGCHLD | libc::SIGWINCH))
                || (interactive
                    && matches!(
                        signum,
                        libc::SIGINT | libc::SIGTERM | libc::SIGHUP | libc::SIGQUIT
                    ))
            {
                install(signum, generic_counter_handler, libc::SA_RESTART);
            } else {
                install_default(signum);
            }
        }
    }
    true
}

pub fn install_early_sigint() {
    extern "C" fn early(_sig: libc::c_int) {
        unsafe {
            libc::_exit(2);
        }
    }
    install(libc::SIGINT, early, libc::SA_RESTART);
}

pub fn sigint_taken() -> bool {
    SIGINT_RECEIVED.swap(false, Ordering::SeqCst)
}

pub fn alarm_taken() -> bool {
    ALARM_FIRED.swap(false, Ordering::SeqCst)
}

pub fn term_taken() -> i32 {
    TERM_RECEIVED.swap(0, Ordering::SeqCst)
}

pub fn catch_flag_set() -> bool {
    CATCH_FLAG.load(Ordering::SeqCst)
}

pub fn catch_flag_clear() {
    CATCH_FLAG.store(false, Ordering::SeqCst);
}

/// Atomically drain a signal's pending counter.
pub fn pending_signal_take(sig: i32) -> u32 {
    if sig <= 0 || (sig as usize) >= NSIG {
        return 0;
    }
    PENDING_COUNTS[sig as usize].swap(0, Ordering::SeqCst)
}

/// Iterator over all signals with pending events (non-zero counter).
pub fn pending_signals_snapshot() -> Vec<i32> {
    let mut out = Vec::new();
    for (sig, counter) in PENDING_COUNTS.iter().enumerate().skip(1) {
        if counter.load(Ordering::SeqCst) > 0 {
            out.push(sig as i32);
        }
    }
    out
}

/// QUIT macro equivalent - call at safe points to convert pending fatal
/// signals into ShellJump errors.
pub fn check_signals() -> ShellResult<()> {
    if alarm_taken() {
        return Err(ShellJump::ExitProg(0));
    }
    let sig = term_taken();
    if sig != 0 {
        return Err(ShellJump::SigExit(128 + sig));
    }
    if sigint_taken() {
        return Err(ShellJump::Discard);
    }
    Ok(())
}

/// Arm SIGALRM after `seconds` seconds. Mirrors eval.c:383-385.
pub fn arm_alarm(seconds: u32) {
    ALARM_FIRED.store(false, Ordering::SeqCst);
    unsafe {
        libc::alarm(seconds as libc::c_uint);
    }
}

pub fn disarm_alarm() {
    unsafe {
        libc::alarm(0);
    }
    ALARM_FIRED.store(false, Ordering::SeqCst);
}

/// Acquire control of the terminal. Mirrors `set_job_control` in
/// bash jobs.c:4595. On success:
///   - state.tty_fd points at /dev/tty
///   - state.shell_pgrp == getpid()
///   - state.original_pgrp == previous tty pgrp
///   - the shell process group owns the terminal
///
/// Returns `false` if we can't take the terminal (e.g. backgrounded).
pub fn acquire_terminal(
    tty_fd_out: &mut Option<RawFd>,
    shell_pgrp_out: &mut i32,
    original_pgrp_out: &mut i32,
) -> bool {
    // Try /dev/tty first, then stderr - bash uses fd 2 if /dev/tty fails.
    let raw_fd = unsafe {
        let mut fd = libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
        if fd < 0 {
            // Fallback: dup stderr if it's a tty.
            if libc::isatty(2) != 0 {
                fd = libc::dup(2);
                if fd >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
        }
        fd
    };
    if raw_fd < 0 {
        return false;
    }
    *tty_fd_out = Some(raw_fd);
    let shell_pid = unsafe { libc::getpid() };
    let mut tries = 0;
    // Loop while another process group owns the terminal - bash raises
    // SIGTTIN against itself until it becomes the foreground group.
    loop {
        let owner = unsafe { libc::tcgetpgrp(raw_fd) };
        if owner == -1 {
            return false;
        }
        if owner == unsafe { libc::getpgrp() } {
            break;
        }
        if tries >= 8 {
            // Give up after eight retries to avoid infinite loop in unusual
            // tty setups (e.g. orphaned process groups).
            return false;
        }
        unsafe {
            libc::kill(-owner, libc::SIGTTIN);
        }
        tries += 1;
    }
    *original_pgrp_out = unsafe { libc::tcgetpgrp(raw_fd) };
    unsafe {
        libc::setpgid(0, shell_pid);
    }
    let _guard = SignalMaskGuard::block_terminal_handoff();
    unsafe {
        libc::tcsetpgrp(raw_fd, shell_pid);
    }
    *shell_pgrp_out = shell_pid;
    true
}
