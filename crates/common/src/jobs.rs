//! Job-control state shared by the shell and its builtins.
//!
//! The layout follows the process and job records in Bash 5.3.15. Code that
//! can race with `SIGCHLD` must hold a `SignalMaskGuard`; child reaping is
//! deferred to safe points.

use std::collections::{HashMap, HashSet};

use bitflags::bitflags;

/// Identifier used for jobspec `%N`. A free slot is reused after its job leaves the table.
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
    pid_to_job: HashMap<i32, JobId>,
    waited_pids: HashSet<i32>,
}

impl JobTable {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            current: None,
            previous: None,
            pid_to_job: HashMap::new(),
            waited_pids: HashSet::new(),
        }
    }

    fn allocate_id(&mut self) -> JobId {
        let mut id = 1u32;
        loop {
            if !self.jobs.iter().any(|j| j.id.0 == id) {
                return JobId(id);
            }
            id += 1;
        }
    }

    fn reset_current_previous(&mut self) {
        let mut candidates: Vec<(usize, JobId, bool)> = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| job.state != JobState::Done)
            .map(|(index, job)| (index, job.id, job.state == JobState::Stopped))
            .collect();
        if candidates.is_empty() {
            let mut completed = self
                .jobs
                .iter()
                .enumerate()
                .filter(|(_, job)| !job.notified)
                .map(|(index, job)| (index, job.id));
            self.current = completed.next_back().map(|(_, id)| id);
            self.previous = completed.next_back().map(|(_, id)| id);
            return;
        }
        candidates.sort_by_key(|(index, _, stopped)| (*stopped, *index));
        self.current = candidates.pop().map(|(_, id, _)| id);
        self.previous = candidates.pop().map(|(_, id, _)| id);
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
            self.waited_pids.remove(&p.pid);
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
                self.waited_pids.remove(&p.pid);
            }
        }
        self.reset_current_previous();
    }

    pub fn job_of_pid(&self, pid: i32) -> Option<JobId> {
        self.pid_to_job.get(&pid).copied()
    }

    pub fn pid_was_waited(&self, pid: i32) -> bool {
        self.waited_pids.contains(&pid)
    }

    pub fn mark_pid_waited(&mut self, pid: i32) {
        self.waited_pids.insert(pid);
        let Some(job_id) = self.job_of_pid(pid) else {
            return;
        };
        let all_waited = self.get(job_id).is_some_and(|job| {
            job.processes
                .iter()
                .all(|p| self.waited_pids.contains(&p.pid))
        });
        if all_waited {
            if let Some(job) = self.get_mut(job_id) {
                job.flags.insert(JobFlags::WAITED_FOR);
            }
        }
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
        let mut completed = false;
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
                completed = true;
            }
        }
        if completed && (self.current == Some(jid) || self.previous == Some(jid)) {
            self.reset_current_previous();
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
                    self.waited_pids.remove(&p.pid);
                }
            }
            k
        });
        if !purged.is_empty() {
            self.reset_current_previous();
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
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                self.mark_dead(pid, status);
            } else if libc::WIFSTOPPED(status) {
                self.mark_stopped(pid, libc::WSTOPSIG(status));
            } else if libc::WIFCONTINUED(status) {
                self.mark_running(pid);
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

/// Converts a raw `waitpid` status to the shell's 0-255 exit status.
pub fn decode_status(raw: i32) -> i32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: i32) -> Process {
        Process {
            pid,
            status_raw: 0,
            state: JobState::Running,
            command: format!("job-{pid}"),
        }
    }

    fn add_job(table: &mut JobTable, pid: i32) -> JobId {
        table.add(
            pid,
            pid,
            format!("job-{pid}"),
            true,
            true,
            vec![process(pid)],
        )
    }

    #[test]
    fn reuses_the_lowest_free_job_id() {
        let mut table = JobTable::new();
        assert_eq!(add_job(&mut table, 101), JobId(1));
        assert_eq!(add_job(&mut table, 102), JobId(2));
        assert_eq!(add_job(&mut table, 103), JobId(3));
        table.remove(JobId(2));
        assert_eq!(add_job(&mut table, 104), JobId(2));
    }

    #[test]
    fn stopped_jobs_take_current_and_previous_markers() {
        let mut table = JobTable::new();
        let first = add_job(&mut table, 201);
        let second = add_job(&mut table, 202);
        assert_eq!(table.current(), Some(second));
        assert_eq!(table.previous(), Some(first));

        table.mark_stopped(201, libc::SIGTSTP);
        assert_eq!(table.current(), Some(first));
        assert_eq!(table.previous(), Some(second));

        table.mark_dead(201, 0);
        assert_eq!(table.current(), Some(second));
        assert_eq!(table.previous(), None);
    }

    #[test]
    fn lone_completed_job_keeps_the_current_marker_until_notification() {
        let mut table = JobTable::new();
        let only = add_job(&mut table, 211);

        table.mark_dead(211, 0);

        assert_eq!(table.current(), Some(only));
        assert_eq!(table.previous(), None);
    }

    #[test]
    fn continuing_a_stopped_job_preserves_current_markers() {
        let mut table = JobTable::new();
        let first = add_job(&mut table, 221);
        let second = add_job(&mut table, 222);
        table.mark_stopped(221, libc::SIGTSTP);

        table.mark_running(221);

        assert_eq!(table.current(), Some(first));
        assert_eq!(table.previous(), Some(second));
    }

    #[test]
    fn removing_current_recomputes_both_markers() {
        let mut table = JobTable::new();
        let first = add_job(&mut table, 301);
        let second = add_job(&mut table, 302);
        let third = add_job(&mut table, 303);
        table.remove(third);
        assert_eq!(table.current(), Some(second));
        assert_eq!(table.previous(), Some(first));
    }
}
