use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::io::RawFd;

use cherubsh_common::{AssignError, W_QUOTED};
use cherubsh_expander::{
    expand_word_list_with_proc_subst, quote::shell_string_to_bytes, ExpandFlags,
};
use cherubsh_parser::{Redirect, RedirectInstruction, Redirectee, Redirector};

use crate::runner::ExecRunner;
use crate::util::expand_herestring_word;
use crate::{ExecContext, ExecMode};

const GUARD_SAVE_FD_BASE: i32 = 50;

#[derive(Debug)]
pub struct ExecError {
    pub message: String,
    raw: bool,
    reported: bool,
}

impl ExecError {
    pub fn new<S: Into<String>>(message: S) -> Self {
        Self {
            message: message.into(),
            raw: false,
            reported: false,
        }
    }

    pub fn raw<S: Into<String>>(message: S) -> Self {
        Self {
            message: message.into(),
            raw: true,
            reported: false,
        }
    }

    fn mark_reported(mut self) -> Self {
        self.reported = true;
        self
    }

    pub fn report(&self) {
        if self.reported {
            return;
        }
        if self.raw {
            eprintln!("{}", self.message);
        } else {
            for line in self.message.lines() {
                eprintln!("cherubsh: {line}");
            }
        }
    }

    pub fn report_with_env(&self, env: &dyn cherubsh_common::Environment) {
        if self.reported {
            return;
        }
        if self.raw {
            eprintln!("{}", self.message);
        } else if let (Some(source), Some(line_no)) =
            (env.diagnostic_source_name(), env.diagnostic_line())
        {
            for line in self.message.lines() {
                eprintln!("{source}: line {line_no}: {line}");
            }
        } else {
            self.report();
        }
    }
}

#[derive(Clone, Debug)]
enum Plan {
    /// Open `path` then dup target_fd to it (with optional truncate/append/no-clobber).
    Open {
        path: String,
        flags: i32,
        mode: libc::mode_t,
        target: i32,
    },
    /// dup2(source, target) - keep source open.
    Dup {
        source: i32,
        target: i32,
        source_label: String,
    },
    /// dup2(source, target) and close source.
    Move {
        source: i32,
        target: i32,
        source_label: String,
    },
    /// close(target).
    Close { target: i32 },
    /// Heredoc body: write `body` to pipe, dup read end to target.
    Heredoc { body: String, target: i32 },
    /// Open file then dup to both 1 and 2.
    OpenErrAndOut { path: String, append: bool },
    /// Open file, assign resulting fd to variable name.
    AssignVar {
        path: String,
        flags: i32,
        var: String,
    },
    /// Duplicate source to a newly-allocated fd >= 10 and assign it to var.
    AssignFd {
        source: i32,
        var: String,
        close_source: bool,
    },
    /// Close the fd currently named by var.
    CloseVar { var: String },
    /// Install a heredoc on a newly-allocated fd >= 10 and assign it to var.
    AssignHeredoc { body: String, var: String },
}

pub(crate) fn apply_redirects_to_parent<'a>(
    ctx: &mut ExecContext<'a>,
    redirects: &[Redirect],
) -> Result<RedirGuard, ExecError> {
    let mut guard = RedirGuard::new(ctx.env.option("varredir_close"));
    for r in redirects {
        let plans = match build_plans(ctx, r, ExecMode::Parent) {
            Ok(plans) => plans,
            Err(err) => {
                err.report_with_env(ctx.env);
                return Err(err.mark_reported());
            }
        };
        for plan in plans {
            if let Err(err) = execute_plan(ctx, plan, Some(&mut guard)) {
                err.report_with_env(ctx.env);
                return Err(err.mark_reported());
            }
        }
    }
    Ok(guard)
}

pub(crate) fn apply_redirects_to_child<'a>(
    ctx: &mut ExecContext<'a>,
    redirects: &[Redirect],
) -> Result<(), ExecError> {
    for r in redirects {
        let plans = build_plans(ctx, r, ExecMode::Child)?;
        for plan in plans {
            execute_plan(ctx, plan, None)?;
        }
    }
    Ok(())
}

fn build_plans<'a>(
    ctx: &mut ExecContext<'a>,
    r: &Redirect,
    _mode: ExecMode,
) -> Result<Vec<Plan>, ExecError> {
    let target = match &r.redirector {
        Redirector::Fd(fd) => Some(*fd),
        Redirector::Var(_) => None,
    };
    let noclobber = ctx.env.option("noclobber");

    match &r.instruction {
        RedirectInstruction::InputDirection => {
            let path = redirectee_path_with_glob(&r.redirectee, ctx, !ctx.env.option("posix"))?;
            let target = target.unwrap_or(0);
            let plan = open_or_var(&r.redirector, path, libc::O_RDONLY, 0, target);
            Ok(vec![plan])
        }
        RedirectInstruction::OutputDirection => {
            let path = redirectee_path(&r.redirectee, ctx)?;
            restricted_output_check(ctx, &path)?;
            let target = target.unwrap_or(1);
            let mut flags = libc::O_WRONLY | libc::O_CREAT;
            if noclobber && !file_is_special(&path) {
                flags |= libc::O_EXCL;
            } else {
                flags |= libc::O_TRUNC;
            }
            let plan = open_or_var(&r.redirector, path, flags, 0o644, target);
            Ok(vec![plan])
        }
        RedirectInstruction::OutputForce => {
            let path = redirectee_path(&r.redirectee, ctx)?;
            restricted_output_check(ctx, &path)?;
            let target = target.unwrap_or(1);
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
            let plan = open_or_var(&r.redirector, path, flags, 0o644, target);
            Ok(vec![plan])
        }
        RedirectInstruction::AppendingTo => {
            let path = redirectee_path(&r.redirectee, ctx)?;
            restricted_output_check(ctx, &path)?;
            let target = target.unwrap_or(1);
            let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND;
            let plan = open_or_var(&r.redirector, path, flags, 0o644, target);
            Ok(vec![plan])
        }
        RedirectInstruction::InputOutput => {
            let path = redirectee_path(&r.redirectee, ctx)?;
            restricted_output_check(ctx, &path)?;
            let target = target.unwrap_or(0);
            let flags = libc::O_RDWR | libc::O_CREAT;
            let plan = open_or_var(&r.redirector, path, flags, 0o644, target);
            Ok(vec![plan])
        }
        RedirectInstruction::DuplicatingInput | RedirectInstruction::DuplicatingOutput => {
            let target = target.unwrap_or(match r.instruction {
                RedirectInstruction::DuplicatingInput => 0,
                _ => 1,
            });
            let source = redirectee_fd(&r.redirectee)?;
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::AssignFd {
                    source,
                    var: var.clone(),
                    close_source: false,
                }]);
            }
            Ok(vec![Plan::Dup {
                source,
                target,
                source_label: source.to_string(),
            }])
        }
        RedirectInstruction::DuplicatingInputWord | RedirectInstruction::DuplicatingOutputWord => {
            let default_target =
                if matches!(r.instruction, RedirectInstruction::DuplicatingInputWord) {
                    0
                } else {
                    1
                };
            let target = target.unwrap_or(default_target);
            let source_label = redirectee_label(&r.redirectee);
            let mut word = redirectee_word(&r.redirectee, ctx)?;
            if word.is_empty() && source_label.ends_with('-') {
                word = "-".to_string();
            }
            if word == "-" {
                if let Redirector::Var(var) = &r.redirector {
                    return Ok(vec![Plan::CloseVar { var: var.clone() }]);
                }
                return Ok(vec![Plan::Close { target }]);
            }
            if let Some(source) = word.strip_suffix('-') {
                let source: i32 = source
                    .parse()
                    .map_err(|_| ExecError::new(format!("{}: ambiguous redirect", word)))?;
                if let Redirector::Var(var) = &r.redirector {
                    return Ok(vec![Plan::AssignFd {
                        source,
                        var: var.clone(),
                        close_source: true,
                    }]);
                }
                return Ok(vec![Plan::Move {
                    source,
                    target,
                    source_label: source_label.clone(),
                }]);
            }
            let source: i32 = match word.parse() {
                Ok(source) if source >= 0 => source,
                Ok(_) => return Err(ExecError::new(format!("{word}: ambiguous redirect"))),
                Err(_)
                    if matches!(r.instruction, RedirectInstruction::DuplicatingOutputWord)
                        && target == 1 =>
                {
                    restricted_output_check(ctx, &word)?;
                    return Ok(vec![Plan::OpenErrAndOut {
                        path: word,
                        append: false,
                    }]);
                }
                Err(_) => {
                    return Err(ExecError::new(format!(
                        "{source_label}: ambiguous redirect"
                    )));
                }
            };
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::AssignFd {
                    source,
                    var: var.clone(),
                    close_source: false,
                }]);
            }
            Ok(vec![Plan::Dup {
                source,
                target,
                source_label,
            }])
        }
        RedirectInstruction::MoveInput | RedirectInstruction::MoveOutput => {
            let default_target = if matches!(r.instruction, RedirectInstruction::MoveInput) {
                0
            } else {
                1
            };
            let target = target.unwrap_or(default_target);
            let source = redirectee_fd(&r.redirectee)?;
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::AssignFd {
                    source,
                    var: var.clone(),
                    close_source: true,
                }]);
            }
            Ok(vec![Plan::Move {
                source,
                target,
                source_label: source.to_string(),
            }])
        }
        RedirectInstruction::MoveInputWord | RedirectInstruction::MoveOutputWord => {
            let default_target = if matches!(r.instruction, RedirectInstruction::MoveInputWord) {
                0
            } else {
                1
            };
            let target = target.unwrap_or(default_target);
            let source_label = redirectee_label(&r.redirectee);
            let word = redirectee_word(&r.redirectee, ctx)?;
            if word == "-" || word.is_empty() && source_label.ends_with('-') {
                if let Redirector::Var(var) = &r.redirector {
                    return Ok(vec![Plan::CloseVar { var: var.clone() }]);
                }
                return Ok(vec![Plan::Close { target }]);
            }
            let source_text = word.strip_suffix('-').unwrap_or(&word);
            let source: i32 = source_text
                .parse()
                .map_err(|_| ExecError::new(format!("{source_label}: ambiguous redirect")))?;
            if source < 0 {
                return Err(ExecError::new(format!("{word}: ambiguous redirect")));
            }
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::AssignFd {
                    source,
                    var: var.clone(),
                    close_source: true,
                }]);
            }
            Ok(vec![Plan::Move {
                source,
                target,
                source_label,
            }])
        }
        RedirectInstruction::CloseThis => {
            let target = target.unwrap_or(0);
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::CloseVar { var: var.clone() }]);
            }
            Ok(vec![Plan::Close { target }])
        }
        RedirectInstruction::ErrAndOut => {
            let path = redirectee_path(&r.redirectee, ctx)?;
            restricted_output_check(ctx, &path)?;
            Ok(vec![Plan::OpenErrAndOut {
                path,
                append: false,
            }])
        }
        RedirectInstruction::AppendErrAndOut => {
            let path = redirectee_path(&r.redirectee, ctx)?;
            restricted_output_check(ctx, &path)?;
            Ok(vec![Plan::OpenErrAndOut { path, append: true }])
        }
        RedirectInstruction::ReadingUntil
        | RedirectInstruction::DeblankReadingUntil
        | RedirectInstruction::InputaDirection => {
            let target = target.unwrap_or(0);
            let raw = r.here_doc_body.clone().unwrap_or_default();
            let stripped = matches!(r.instruction, RedirectInstruction::DeblankReadingUntil);
            let quoted = heredoc_delimiter_quoted(r);
            let body = prepare_heredoc_body(&raw, stripped, quoted, ctx);
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::AssignHeredoc {
                    body,
                    var: var.clone(),
                }]);
            }
            Ok(vec![Plan::Heredoc { body, target }])
        }
        RedirectInstruction::ReadingString => {
            let target = target.unwrap_or(0);
            let word = match &r.redirectee {
                Redirectee::Word(word) => expand_herestring_word(word, ctx),
                Redirectee::Fd(_) => redirectee_word(&r.redirectee, ctx)?,
            };
            let body = format!("{}\n", word);
            if let Redirector::Var(var) = &r.redirector {
                return Ok(vec![Plan::AssignHeredoc {
                    body,
                    var: var.clone(),
                }]);
            }
            Ok(vec![Plan::Heredoc { body, target }])
        }
    }
}

fn open_or_var(
    redirector: &Redirector,
    path: String,
    flags: i32,
    mode: libc::mode_t,
    target: i32,
) -> Plan {
    match redirector {
        Redirector::Fd(_) => Plan::Open {
            path,
            flags,
            mode,
            target,
        },
        Redirector::Var(name) => Plan::AssignVar {
            path,
            flags,
            var: name.clone(),
        },
    }
}

fn file_is_special(path: &str) -> bool {
    path.starts_with("/dev/")
}

fn redirectee_path(r: &Redirectee, ctx: &mut ExecContext) -> Result<String, ExecError> {
    redirectee_path_with_glob(r, ctx, true)
}

fn redirectee_path_with_glob(
    r: &Redirectee,
    ctx: &mut ExecContext,
    expand_glob: bool,
) -> Result<String, ExecError> {
    match r {
        Redirectee::Word(w) => expand_redirect_word(w, ctx, expand_glob),
        Redirectee::Fd(_) => Err(ExecError::new("redirectee expected word")),
    }
}

fn restricted_output_check(ctx: &ExecContext<'_>, path: &str) -> Result<(), ExecError> {
    if ctx.env.option("restricted") {
        return Err(ExecError::raw(format!(
            "{}: restricted: cannot redirect output",
            cherubsh_builtins::common::diagnostic_label(ctx.env, path)
        )));
    }
    Ok(())
}

fn redirectee_word(r: &Redirectee, ctx: &mut ExecContext) -> Result<String, ExecError> {
    match r {
        Redirectee::Word(w) => expand_redirect_word(w, ctx, true),
        Redirectee::Fd(fd) => Ok(fd.to_string()),
    }
}

fn expand_redirect_word(
    word: &cherubsh_parser::WordDesc,
    ctx: &mut ExecContext<'_>,
    expand_glob: bool,
) -> Result<String, ExecError> {
    let mut runner = ExecRunner::with_functions_mut_at_depth(
        &mut ctx.functions,
        &mut ctx.function_sources,
        ctx.function_depth,
        ctx.source_depth,
    );
    let mut flags = ExpandFlags::SPLIT_FIELDS | ExpandFlags::QUOTE_REMOVAL | ExpandFlags::FOR_REDIR;
    if expand_glob {
        flags |= ExpandFlags::EXPAND_GLOB;
    }
    let expanded =
        expand_word_list_with_proc_subst(std::slice::from_ref(word), ctx.env, &mut runner, flags)
            .map_err(|err| ExecError::new(err.into_shell_error(Some(word.span)).message))?;
    ctx.register_proc_subst(expanded.proc_subst);
    if expanded.words.is_empty() && word.text.ends_with('-') {
        return Ok("-".to_string());
    }
    if expanded.words.len() != 1 {
        return Err(ExecError::new(format!("{}: ambiguous redirect", word.text)));
    }
    Ok(expanded
        .words
        .into_iter()
        .next()
        .map(|word| word.text)
        .unwrap_or_default())
}

fn redirectee_label(r: &Redirectee) -> String {
    match r {
        Redirectee::Word(w) => w.text.clone(),
        Redirectee::Fd(fd) => fd.to_string(),
    }
}

fn redirectee_fd(r: &Redirectee) -> Result<i32, ExecError> {
    match r {
        Redirectee::Fd(fd) => Ok(*fd),
        Redirectee::Word(w) => w
            .text
            .parse::<i32>()
            .map_err(|_| ExecError::new(format!("{}: ambiguous redirect", w.text))),
    }
}

fn heredoc_delimiter_quoted(r: &Redirect) -> bool {
    match &r.redirectee {
        Redirectee::Word(word) => word.flags & W_QUOTED != 0,
        Redirectee::Fd(_) => false,
    }
}

fn prepare_heredoc_body(
    body: &str,
    strip_tabs: bool,
    quoted_delim: bool,
    ctx: &mut ExecContext,
) -> String {
    let mut processed = if strip_tabs {
        body.lines()
            .map(|l| l.trim_start_matches('\t'))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        body.to_string()
    };
    if !body.is_empty() && !processed.ends_with('\n') {
        processed.push('\n');
    }
    if quoted_delim {
        processed
    } else {
        crate::util::expand_heredoc_body(&processed, ctx)
    }
}

pub(crate) struct RedirGuard {
    saved_fd: Vec<(i32, i32)>,
    saved_var: Vec<(String, Option<String>)>,
    assigned_var_fds: Vec<i32>,
    close_var_fds: bool,
}

impl RedirGuard {
    fn new(close_var_fds: bool) -> Self {
        Self {
            saved_fd: Vec::new(),
            saved_var: Vec::new(),
            assigned_var_fds: Vec::new(),
            close_var_fds,
        }
    }

    fn save_fd(&mut self, target: i32) -> Result<(), ExecError> {
        self.save_fd_avoiding(target, &[])
    }

    fn save_fd_avoiding(&mut self, target: i32, avoid: &[i32]) -> Result<(), ExecError> {
        let saved = unsafe { libc::fcntl(target, libc::F_DUPFD_CLOEXEC, GUARD_SAVE_FD_BASE) };
        let saved = if saved < 0 {
            unsafe { libc::fcntl(target, libc::F_DUPFD_CLOEXEC, 10) }
        } else {
            saved
        };
        if saved < 0 {
            // target may be closed; remember sentinel -1 so we restore by close
            self.saved_fd.push((target, -1));
        } else {
            let saved = if avoid.contains(&saved) {
                unsafe {
                    libc::close(saved);
                    libc::fcntl(target, libc::F_DUPFD_CLOEXEC, saved + 1)
                }
            } else {
                saved
            };
            if saved < 0 {
                self.saved_fd.push((target, -1));
            } else {
                self.saved_fd.push((target, saved));
            }
        }
        Ok(())
    }

    fn save_var(&mut self, name: &str, prior: Option<String>) {
        self.saved_var.push((name.to_string(), prior));
    }

    fn note_var_fd(&mut self, fd: i32) {
        if self.close_var_fds {
            self.assigned_var_fds.push(fd);
        }
    }

    pub(crate) fn persist(mut self) {
        for (_target, saved) in self.saved_fd.drain(..) {
            if saved >= 0 {
                unsafe {
                    libc::close(saved);
                }
            }
        }
        self.saved_var.clear();
        self.assigned_var_fds.clear();
    }
}

impl Drop for RedirGuard {
    fn drop(&mut self) {
        for (target, saved) in self.saved_fd.drain(..).rev() {
            unsafe {
                if saved < 0 {
                    libc::close(target);
                } else {
                    libc::dup2(saved, target);
                    libc::close(saved);
                }
            }
        }
        // saved_var entries are restored by the caller via env reference
        // (we can't borrow env inside Drop). Treat as informational only.
        self.saved_var.clear();
        for fd in self.assigned_var_fds.drain(..).rev() {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

fn execute_plan<'a>(
    ctx: &mut ExecContext<'a>,
    plan: Plan,
    mut guard: Option<&mut RedirGuard>,
) -> Result<(), ExecError> {
    match plan {
        Plan::Open {
            path,
            flags,
            mode,
            target,
        } => {
            let fd = open_path(&path, flags, mode)?;
            if let Some(g) = guard {
                g.save_fd(target)?;
            }
            dup2_close(fd, target)?;
            let target_proc_path = format!("/dev/fd/{target}");
            if path == target_proc_path {
                for handle in &mut ctx.proc_subst {
                    if handle.fd == target && handle.path == path {
                        handle.fd = -1;
                    }
                }
            }
            Ok(())
        }
        Plan::Dup {
            source,
            target,
            source_label,
        } => {
            if source == target {
                return Ok(());
            }
            if let Some(g) = guard {
                g.save_fd_avoiding(target, &[source])?;
            }
            let res = unsafe { libc::dup2(source, target) };
            if res < 0 {
                return Err(ExecError::new(format!(
                    "{source_label}: Bad file descriptor"
                )));
            }
            Ok(())
        }
        Plan::Move {
            source,
            target,
            source_label,
        } => {
            if source == target {
                return Ok(());
            }
            if let Some(g) = guard {
                g.save_fd_avoiding(target, &[source])?;
            }
            let res = unsafe { libc::dup2(source, target) };
            if res < 0 {
                return Err(ExecError::new(format!(
                    "{source_label}: Bad file descriptor"
                )));
            }
            unsafe {
                libc::close(source);
            }
            mark_coproc_fd_moved(ctx, source);
            Ok(())
        }
        Plan::Close { target } => {
            if let Some(g) = guard {
                g.save_fd(target)?;
            }
            unsafe {
                libc::close(target);
            }
            Ok(())
        }
        Plan::OpenErrAndOut { path, append } => {
            let mut flags = libc::O_WRONLY | libc::O_CREAT;
            if append {
                flags |= libc::O_APPEND;
            } else {
                flags |= libc::O_TRUNC;
            }
            let fd = open_path(&path, flags, 0o644)?;
            if let Some(g) = guard {
                g.save_fd(1)?;
                g.save_fd(2)?;
            }
            dup2_close(fd, 1)?;
            let r = unsafe { libc::dup2(1, 2) };
            if r < 0 {
                return Err(ExecError::new("dup2 stderr failed"));
            }
            Ok(())
        }
        Plan::Heredoc { body, target } => {
            install_heredoc(&body, target, guard)?;
            Ok(())
        }
        Plan::AssignVar { path, flags, var } => {
            let fd = open_path(&path, flags, 0o644)?;
            let high = allocate_high_fd(fd);
            let high_errno = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            if high < 0 {
                return Err(var_fd_dup_error(ctx, &path, high_errno));
            }
            if let Some(g) = guard.as_deref_mut() {
                let prior = ctx.env.get(&var);
                g.save_var(&var, prior);
            }
            if let Err(err) = assign_fd_var(ctx, &var, high) {
                unsafe {
                    libc::close(high);
                }
                return Err(err);
            }
            if let Some(g) = guard {
                g.note_var_fd(high);
            }
            Ok(())
        }
        Plan::AssignFd {
            source,
            var,
            close_source,
        } => {
            let high = allocate_high_fd(source);
            if high < 0 {
                return Err(ExecError::new(format!("{source}: Bad file descriptor")));
            }
            if let Some(g) = guard.as_deref_mut() {
                let prior = ctx.env.get(&var);
                g.save_var(&var, prior);
            }
            if let Err(err) = assign_fd_var(ctx, &var, high) {
                unsafe {
                    libc::close(high);
                }
                return Err(err);
            }
            if let Some(g) = guard {
                g.note_var_fd(high);
            }
            if close_source {
                unsafe {
                    libc::close(source);
                }
                mark_coproc_fd_moved(ctx, source);
            }
            Ok(())
        }
        Plan::CloseVar { var } => {
            let fd = fd_from_var(ctx, &var)?;
            unsafe {
                libc::close(fd);
            }
            mark_coproc_fd_moved(ctx, fd);
            Ok(())
        }
        Plan::AssignHeredoc { body, var } => {
            let fd = heredoc_read_fd(&body)?;
            let high = allocate_high_fd(fd);
            unsafe {
                libc::close(fd);
            }
            if high < 0 {
                return Err(ExecError::new(format!(
                    "{}: cannot assign fd to variable",
                    var
                )));
            }
            if let Some(g) = guard.as_deref_mut() {
                let prior = ctx.env.get(&var);
                g.save_var(&var, prior);
            }
            if let Err(err) = assign_fd_var(ctx, &var, high) {
                unsafe {
                    libc::close(high);
                }
                return Err(err);
            }
            if let Some(g) = guard {
                g.note_var_fd(high);
            }
            Ok(())
        }
    }
}

fn allocate_high_fd(fd: RawFd) -> RawFd {
    unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) }
}

enum VarTarget<'a> {
    Scalar(&'a str),
    Indexed { name: &'a str, index: i64 },
}

fn parse_var_target(var: &str) -> Result<VarTarget<'_>, ExecError> {
    let Some(open) = var.find('[') else {
        return Ok(VarTarget::Scalar(var));
    };
    if !var.ends_with(']') {
        return Err(ExecError::new(format!("{var}: ambiguous redirect")));
    }
    let name = &var[..open];
    let subscript = &var[open + 1..var.len() - 1];
    let index = subscript
        .trim()
        .parse::<i64>()
        .map_err(|_| ExecError::new(format!("{var}: bad array subscript")))?;
    Ok(VarTarget::Indexed { name, index })
}

fn assign_fd_var(ctx: &mut ExecContext<'_>, var: &str, fd: RawFd) -> Result<(), ExecError> {
    match parse_var_target(var)? {
        VarTarget::Scalar(name) => {
            if ctx.env.is_readonly(name) {
                return Err(readonly_fd_assign_error(name));
            }
            if let Err(err) = ctx.env.assign(name, fd.to_string()) {
                return Err(fd_assign_error(name, err));
            }
        }
        VarTarget::Indexed { name, index } => {
            if ctx.env.is_readonly(name) {
                return Err(readonly_fd_assign_error(name));
            }
            ctx.env.set_array_indexed(name, index, fd.to_string());
        }
    }
    Ok(())
}

fn fd_assign_error(name: &str, err: AssignError) -> ExecError {
    match err {
        AssignError::ReadOnly(readonly) => readonly_fd_assign_error(&readonly),
        AssignError::InvalidName(value) => ExecError::new(format!(
            "exec: `{value}': not a valid identifier\n{name}: cannot assign fd to variable"
        )),
        AssignError::BadArraySubscript(value) => ExecError::new(format!(
            "{value}: bad array subscript\n{name}: cannot assign fd to variable"
        )),
        AssignError::InvalidInteger(value) => ExecError::new(format!(
            "{value}: invalid integer\n{name}: cannot assign fd to variable"
        )),
        AssignError::CircularNameReference(value) => ExecError::new(format!(
            "{value}: circular name reference\n{name}: cannot assign fd to variable"
        )),
    }
}

fn readonly_fd_assign_error(name: &str) -> ExecError {
    ExecError::new(format!(
        "{name}: readonly variable\n{name}: cannot assign fd to variable"
    ))
}

fn var_fd_dup_error(ctx: &ExecContext<'_>, subject: &str, err: std::io::Error) -> ExecError {
    let err = errno_message(&err);
    if let (Some(source), Some(line_no)) =
        (ctx.env.diagnostic_source_name(), ctx.env.diagnostic_line())
    {
        ExecError::raw(format!(
            "{source}: redirection error: cannot duplicate fd: {err}\n{source}: line {line_no}: {subject}: {err}"
        ))
    } else {
        ExecError::raw(format!(
            "cherubsh: redirection error: cannot duplicate fd: {err}\ncherubsh: {subject}: {err}"
        ))
    }
}

fn errno_message(err: &std::io::Error) -> String {
    match err.raw_os_error() {
        Some(libc::EINVAL) => "Invalid argument".to_string(),
        Some(libc::EBADF) => "Bad file descriptor".to_string(),
        Some(libc::EACCES) => "Permission denied".to_string(),
        Some(libc::EPERM) => "Permission denied".to_string(),
        Some(libc::ENOENT) => "No such file or directory".to_string(),
        Some(libc::EISDIR) => "Is a directory".to_string(),
        Some(libc::EMFILE) => "Too many open files".to_string(),
        Some(errno) => {
            let message = unsafe { libc::strerror(errno) };
            if message.is_null() {
                err.to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(message) }
                    .to_string_lossy()
                    .into_owned()
            }
        }
        None => err.to_string(),
    }
}

fn fd_from_var(ctx: &mut ExecContext<'_>, var: &str) -> Result<RawFd, ExecError> {
    let value = match parse_var_target(var)? {
        VarTarget::Scalar(name) => {
            let target = ctx
                .env
                .resolve_nameref(name)
                .unwrap_or_else(|| name.to_string());
            ctx.env.get(&target)
        }
        VarTarget::Indexed { name, index } => ctx.env.get_array_indexed(name, index),
    };
    value
        .and_then(|value| value.parse::<RawFd>().ok())
        .ok_or_else(|| ExecError::new(format!("{var}: ambiguous redirect")))
}

fn mark_coproc_fd_moved(ctx: &mut ExecContext<'_>, source: i32) {
    let Some(mut values) = ctx.env.get_array("COPROC") else {
        return;
    };
    let source = source.to_string();
    let mut changed = false;
    for value in &mut values {
        if *value == source {
            *value = "-1".to_string();
            changed = true;
        }
    }
    if changed {
        ctx.env.set_array("COPROC", values);
    }
}

fn open_path(path: &str, flags: i32, mode: libc::mode_t) -> Result<RawFd, ExecError> {
    if let Some((host, service)) = tcp_endpoint(path) {
        return open_tcp(path, host, service);
    }
    // Use OpenOptions for read+create paths to interop with rust types when
    // possible; otherwise call libc::open directly.
    let cpath = std::ffi::CString::new(path)
        .map_err(|_| ExecError::new(format!("{path}: invalid path")))?;
    let fd = unsafe { libc::open(cpath.as_ptr(), flags, mode as libc::c_uint) };
    if fd < 0 {
        let errno = std::io::Error::last_os_error();
        let message = if flags & libc::O_EXCL != 0 && errno.raw_os_error() == Some(libc::EEXIST) {
            "cannot overwrite existing file".to_string()
        } else {
            errno_message(&errno)
        };
        return Err(ExecError::new(format!("{path}: {message}")));
    }
    Ok(fd)
}

fn tcp_endpoint(path: &str) -> Option<(&str, &str)> {
    let endpoint = path.strip_prefix("/dev/tcp/")?;
    let (host, service) = endpoint.split_once('/')?;
    if host.is_empty() || service.is_empty() || service.contains('/') {
        return None;
    }
    Some((host, service))
}

fn open_tcp(path: &str, host: &str, service: &str) -> Result<RawFd, ExecError> {
    let host_name = std::ffi::CString::new(host)
        .map_err(|_| ExecError::new(format!("{path}: invalid hostname")))?;
    let service_name = std::ffi::CString::new(service)
        .map_err(|_| ExecError::new(format!("{path}: invalid service")))?;
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;
    let mut addresses: *mut libc::addrinfo = std::ptr::null_mut();
    let resolve_status = unsafe {
        libc::getaddrinfo(
            host_name.as_ptr(),
            service_name.as_ptr(),
            &hints,
            &mut addresses,
        )
    };
    if resolve_status != 0 {
        let message = unsafe {
            let ptr = libc::gai_strerror(resolve_status);
            if ptr.is_null() {
                format!("address lookup failed ({resolve_status})")
            } else {
                std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        let subject = if resolve_status == libc::EAI_SERVICE {
            service
        } else {
            host
        };
        eprintln!("cherubsh: {subject}: {message}");
        return Err(ExecError::new(format!("{path}: Invalid argument")));
    }

    let mut current = addresses;
    let mut last_error = None;
    while !current.is_null() {
        let address = unsafe { &*current };
        let fd =
            unsafe { libc::socket(address.ai_family, address.ai_socktype, address.ai_protocol) };
        if fd >= 0 {
            let connected = unsafe { libc::connect(fd, address.ai_addr, address.ai_addrlen) };
            if connected == 0 {
                unsafe { libc::freeaddrinfo(addresses) };
                return Ok(fd);
            }
            last_error = Some(std::io::Error::last_os_error());
            unsafe { libc::close(fd) };
        } else {
            last_error = Some(std::io::Error::last_os_error());
        }
        current = address.ai_next;
    }
    unsafe { libc::freeaddrinfo(addresses) };

    let error = last_error.unwrap_or_else(|| std::io::Error::from_raw_os_error(libc::EINVAL));
    let message = errno_message(&error);
    eprintln!("cherubsh: connect: {message}");
    Err(ExecError::new(format!("{path}: {message}")))
}

fn dup2_close(source: RawFd, target: i32) -> Result<(), ExecError> {
    let res = unsafe { libc::dup2(source, target) };
    if res < 0 {
        unsafe { libc::close(source) };
        return Err(ExecError::new(format!("dup2({source}, {target}) failed")));
    }
    if source != target {
        unsafe { libc::close(source) };
    }
    Ok(())
}

fn install_heredoc(
    body: &str,
    target: i32,
    guard: Option<&mut RedirGuard>,
) -> Result<(), ExecError> {
    let read_fd = heredoc_read_fd(body)?;
    if let Some(g) = guard {
        g.save_fd(target)?;
    }
    dup2_close(read_fd, target)?;
    Ok(())
}

fn heredoc_read_fd(body: &str) -> Result<RawFd, ExecError> {
    let bytes = shell_string_to_bytes(body);
    let mut pipefd = [0i32; 2];
    if unsafe { libc::pipe(pipefd.as_mut_ptr()) } < 0 {
        return Err(ExecError::new("pipe() failed for heredoc"));
    }
    let (read_fd, write_fd) = (pipefd[0], pipefd[1]);

    // For small bodies (fit in pipe capacity), write from parent then close.
    // For large bodies, spawn a writer process so we don't block.
    const PIPE_THRESHOLD: usize = 4096;
    if bytes.len() < PIPE_THRESHOLD {
        let mut file = unsafe { make_file_for_fd(write_fd) };
        let _ = file.write_all(&bytes);
        drop(file); // closes write_fd
    } else {
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return Err(ExecError::new("fork() failed for heredoc"));
        }
        if pid == 0 {
            unsafe {
                libc::close(read_fd);
            }
            let mut file = unsafe { make_file_for_fd(write_fd) };
            let _ = file.write_all(&bytes);
            unsafe {
                libc::_exit(0);
            }
        }
        unsafe {
            libc::close(write_fd);
        }
    }
    Ok(read_fd)
}

unsafe fn make_file_for_fd(fd: RawFd) -> std::fs::File {
    use std::os::unix::io::FromRawFd;
    std::fs::File::from_raw_fd(fd)
}

// keep OpenOptions in dep graph for binary size guidance (not used directly here)
#[allow(dead_code)]
fn _opener_marker() {
    let _ = OpenOptions::new();
}
