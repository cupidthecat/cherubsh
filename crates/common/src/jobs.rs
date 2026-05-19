//! Job-control data model.
//!
//! Mirrors bash-5.2.21 `jobs.h` PROCESS/JOB structs. The single source of
//! truth for live jobs lives in `ShellState.jobs` (a `JobTable`) so builtins
//! and the exec engine can both mutate it through the `Environment` trait.
//!
//! Concurrency: every mutator that races against SIGCHLD must be wrapped in
//! a `SignalMaskGuard` (see `cherubsh_common::signals`). The signal handler
//! itself is async-signal-safe and only touches the atomic pending-counters
//! in `signals::pending_counts`; reaping happens at safe points via
//! `JobTable::reap_all`.

use std::collections::HashMap;

use bitflags::bitflags;

/// Identifier used for jobspec `%N`. Increments monotonically; ids are
/// reused only after a job is `purge_done`-ed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u32);

impl JobId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}]", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Done,
}

bitflags! {
    /// Per-job flags. Mirrors bash jobs.h `J_*`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct JobFlags: u32 {
        const FOREGROUND     = 1 << 0;
        const ASYNC          = 1 << 1;
        const NOTIFIED       = 1 << 2;
        const WAITED_FOR     = 1 << 3;
        const NOHUP          = 1 << 4;
        const RUN_AND_DISCARD = 1 << 5;
        const JOBCONTROL     = 1 << 6;
    }
}

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: i32,
    /// Raw `waitpid` status, decoded on demand via `libc::WIFEXITED` etc.
    pub status_raw: i32,
    pub state: JobState,
    pub command: String,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: JobId,
    pub pgid: i32,
    pub leader_pid: i32,
    pub state: JobState,
    pub flags: JobFlags,
    pub processes: Vec<Process>,
    pub command_line: String,
    pub notified: bool,
    /// The pipeline's terminating exit status once all processes terminate.
    pub exit_status: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobSpec {
    /// `%%` / `%+` - the current job.
    Current,
    /// `%-` - the previous job.
    Previous,
    /// `%N` - job by id.
    Id(JobId),
    /// `%string` - most-recent job whose command begins with `string`.
    PrefixString(String),
    /// `%?substring` - most-recent job whose command contains `substring`.
    SubstringString(String),
    /// raw pid.
    Pid(i32),
}

impl JobSpec {
    /// Parse a token like `%1`, `%+`, `%name`, `%?sub`, or a bare pid.
    pub fn parse(token: &str) -> Option<Self> {
        if let Some(rest) = token.strip_prefix('%') {
            if rest.is_empty() || rest == "%" || rest == "+" {
                return Some(JobSpec::Current);
            }
            if rest == "-" {
                return Some(JobSpec::Previous);
            }
            if let Some(sub) = rest.strip_prefix('?') {
                return Some(JobSpec::SubstringString(sub.to_string()));
            }
            if let Ok(n) = rest.parse::<u32>() {
                return Some(JobSpec::Id(JobId(n)));
            }
            return Some(JobSpec::PrefixString(rest.to_string()));
        }
        if let Ok(pid) = token.parse::<i32>() {
            return Some(JobSpec::Pid(pid));
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobLookupErr {
    NoSuchJob,
    Ambiguous,
}

#[derive(Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    current: Option<JobId>,
    previous: Option<JobId>,
    next_id: u32,
    pid_to_job: HashMap<i32, JobId>,
}

impl JobTable {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            current: None,
            previous: None,
            next_id: 0,
            pid_to_job: HashMap::new(),
        }
    }

    fn allocate_id(&mut self) -> JobId {
        // bash assigns the next gap-free positive integer; we approximate by
        // scanning for an unused id starting at 1 (matches typical output).
        let mut id = 1u32;
        loop {
            if !self.jobs.iter().any(|j| j.id.0 == id) {
                if id > self.next_id {
                    self.next_id = id;
                }
                return JobId(id);
            }
            id += 1;
        }
    }

    pub fn add(
        &mut self,
        pgid: i32,
        leader_pid: i32,
        command_line: String,
        async_flag: bool,
        job_control: bool,
        processes: Vec<Process>,
    ) -> JobId {
        let id = self.allocate_id();
        let mut flags = JobFlags::empty();
        if async_flag {
            flags |= JobFlags::ASYNC;
        } else {
            flags |= JobFlags::FOREGROUND;
        }
        if job_control {
            flags |= JobFlags::JOBCONTROL;
        }
        for p in &processes {
            self.pid_to_job.insert(p.pid, id);
        }
        let job = Job {
            id,
            pgid,
            leader_pid,
            state: JobState::Running,
            flags,
            processes,
            command_line,
            notified: false,
            exit_status: None,
        };
        self.jobs.push(job);
        self.previous = self.current;
        self.current = Some(id);
        id
    }

    pub fn list(&self) -> &[Job] {
        &self.jobs
    }

    pub fn list_mut(&mut self) -> &mut [Job] {
        &mut self.jobs
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn current(&self) -> Option<JobId> {
        self.current
    }

    pub fn previous(&self) -> Option<JobId> {
        self.previous
    }

    pub fn remove(&mut self, id: JobId) {
        if let Some(pos) = self.jobs.iter().position(|j| j.id == id) {
            let job = self.jobs.remove(pos);
            for p in &job.processes {
                self.pid_to_job.remove(&p.pid);
            }
        }
        if self.current == Some(id) {
            self.current = self.previous;
            self.previous = None;
        } else if self.previous == Some(id) {
            self.previous = None;
        }
        // pick a fresh previous from any remaining stopped/running job
        if self.previous.is_none() && self.current.is_some() {
            let cur = self.current.unwrap();
            self.previous = self.jobs.iter().rev().find(|j| j.id != cur).map(|j| j.id);
        }
    }

    pub fn job_of_pid(&self, pid: i32) -> Option<JobId> {
        self.pid_to_job.get(&pid).copied()
    }

    pub fn lookup(&self, spec: &JobSpec) -> Result<JobId, JobLookupErr> {
        match spec {
            JobSpec::Current => self.current.ok_or(JobLookupErr::NoSuchJob),
            JobSpec::Previous => self.previous.ok_or(JobLookupErr::NoSuchJob),
            JobSpec::Id(id) => {
                if self.get(*id).is_some() {
                    Ok(*id)
                } else {
                    Err(JobLookupErr::NoSuchJob)
                }
            }
            JobSpec::PrefixString(s) => {
                let mut hits: Vec<JobId> = self
                    .jobs
                    .iter()
                    .filter(|j| j.command_line.starts_with(s))
                    .map(|j| j.id)
                    .collect();
                match hits.len() {
                    0 => Err(JobLookupErr::NoSuchJob),
                    1 => Ok(hits.pop().unwrap()),
                    _ => Err(JobLookupErr::Ambiguous),
                }
            }
            JobSpec::SubstringString(s) => {
                let mut hits: Vec<JobId> = self
                    .jobs
                    .iter()
                    .filter(|j| j.command_line.contains(s.as_str()))
                    .map(|j| j.id)
                    .collect();
                match hits.len() {
                    0 => Err(JobLookupErr::NoSuchJob),
                    1 => Ok(hits.pop().unwrap()),
                    _ => Err(JobLookupErr::Ambiguous),
                }
            }
            JobSpec::Pid(pid) => self
                .pid_to_job
                .get(pid)
                .copied()
                .ok_or(JobLookupErr::NoSuchJob),
        }
    }

    pub fn mark_dead(&mut self, pid: i32, status_raw: i32) {
        let Some(jid) = self.pid_to_job.get(&pid).copied() else {
            return;
        };
        if let Some(job) = self.get_mut(jid) {
            for p in job.processes.iter_mut() {
                if p.pid == pid {
                    p.status_raw = status_raw;
                    p.state = JobState::Done;
                }
            }
            if job.processes.iter().all(|p| p.state == JobState::Done) {
                job.state = JobState::Done;
                let last = job.processes.last().map(|p| p.status_raw).unwrap_or(0);
                job.exit_status = Some(decode_status(last));
            }
        }
    }

    pub fn mark_stopped(&mut self, pid: i32, stop_sig: i32) {
        let Some(jid) = self.pid_to_job.get(&pid).copied() else {
            return;
        };
        if let Some(job) = self.get_mut(jid) {
            for p in job.processes.iter_mut() {
                if p.pid == pid {
                    p.state = JobState::Stopped;
                    p.status_raw = (stop_sig << 8) | 0x7f;
                }
            }
            job.state = JobState::Stopped;
        }
        if self.current != Some(jid) {
            self.previous = self.current;
            self.current = Some(jid);
        }
    }

    pub fn mark_running(&mut self, pid: i32) {
        let Some(jid) = self.pid_to_job.get(&pid).copied() else {
            return;
        };
        self.mark_job_running(jid);
    }

    pub fn mark_job_running(&mut self, jid: JobId) {
        if let Some(job) = self.get_mut(jid) {
            for p in job.processes.iter_mut() {
                if p.state == JobState::Stopped {
                    p.state = JobState::Running;
                }
            }
            if job.processes.iter().any(|p| p.state == JobState::Running) {
                job.state = JobState::Running;
                job.notified = false;
            }
        }
    }

    /// Drain finished jobs that have been notified. Returns the pruned ids.
    pub fn purge_done(&mut self) -> Vec<JobId> {
        let mut purged = Vec::new();
        let keep: Vec<bool> = self
            .jobs
            .iter()
            .map(|j| !(j.state == JobState::Done && j.notified))
            .collect();
        let mut i = 0;
        self.jobs.retain(|j| {
            let k = keep[i];
            i += 1;
            if !k {
                purged.push(j.id);
                for p in &j.processes {
                    self.pid_to_job.remove(&p.pid);
                }
            }
            k
        });
        if let Some(c) = self.current {
            if purged.contains(&c) {
                self.current = self.previous;
                self.previous = None;
            }
        }
        if let Some(p) = self.previous {
            if purged.contains(&p) {
                self.previous = None;
            }
        }
        purged
    }

    /// Loop `waitpid(-1, WNOHANG|WUNTRACED|WCONTINUED)` until 0/-1.
    /// **Caller must hold a `SignalMaskGuard` over SIGCHLD.**
    pub fn reap_all(&mut self) -> Vec<(i32, i32)> {
        let mut reaped = Vec::new();
        loop {
            let mut status: libc::c_int = 0;
            let pid = unsafe {
                libc::waitpid(
                    -1,
                    &mut status,
                    libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED,
                )
            };
            if pid <= 0 {
                break;
            }
            unsafe {
                if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                    self.mark_dead(pid, status);
                } else if libc::WIFSTOPPED(status) {
                    self.mark_stopped(pid, libc::WSTOPSIG(status));
                } else if libc::WIFCONTINUED(status) {
                    self.mark_running(pid);
                }
            }
            reaped.push((pid, status));
        }
        reaped
    }

    /// Pending notifications: returns clones of jobs that have changed state
    /// since the last call. Marks them `notified = true` as a side effect.
    pub fn pending_notifications(&mut self) -> Vec<Job> {
        let mut out = Vec::new();
        for j in self.jobs.iter_mut() {
            if !j.notified && j.state != JobState::Running {
                out.push(j.clone());
                j.notified = true;
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn any_running(&self) -> bool {
        self.jobs.iter().any(|j| j.state == JobState::Running)
    }
}

/// Decode a raw `waitpid` status into the bash-conventional 0..255 exit code
/// (signal-terminated → 128+sig, exited → status, stopped → 128+sig).
pub fn decode_status(raw: i32) -> i32 {
    unsafe {
        if libc::WIFEXITED(raw) {
            libc::WEXITSTATUS(raw)
        } else if libc::WIFSIGNALED(raw) {
            128 + libc::WTERMSIG(raw)
        } else if libc::WIFSTOPPED(raw) {
            128 + libc::WSTOPSIG(raw)
        } else {
            1
        }
    }
}
