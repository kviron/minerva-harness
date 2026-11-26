use super::{output::Output, process, JobOutput, JobSnapshot, JobStatus};
use crate::tools::{Cancellation, ToolContext};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::JoinHandle};

const SESSION_ACTIVE: usize = 10;
const GLOBAL_ACTIVE: usize = 64;
const SESSION_SETTLED: usize = 20;
const GLOBAL_SETTLED: usize = 256;
#[derive(Default)]
struct State {
    jobs: BTreeMap<String, Arc<Job>>,
    leases: HashMap<String, HashSet<u64>>,
    paused_sessions: HashSet<String>,
    next_lease: u64,
    stopped: bool,
    #[cfg(windows)]
    journals: HashMap<String, std::sync::Weak<ProcessJournal>>,
}
pub(super) struct JobState {
    pub status: JobStatus,
    pub published: bool,
    pub exit_code: Option<i32>,
    pub stdout: Output,
    pub stderr: Output,
}
pub(super) struct Job {
    pub id: String,
    pub session: String,
    pub label: String,
    pub command: String,
    pub started_at: u64,
    pub state: Mutex<JobState>,
    pub cancellation: Cancellation,
    pub launch: watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    pub changed: watch::Sender<u64>,
}
impl Job {
    pub fn status(&self) -> JobStatus {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).status
    }
    pub fn published(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).published
    }
    pub fn notify(&self) {
        self.changed.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.is_active() {
            state.status = JobStatus::Stopping;
            self.cancellation.cancel();
            drop(state);
            self.notify();
        }
    }
    pub fn snapshot(&self) -> JobSnapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (stdout, a) = state.stdout.tail(!state.status.is_active());
        let (stderr, b) = state.stderr.tail(!state.status.is_active());
        JobSnapshot {
            id: self.id.clone(),
            label: self.label.clone(),
            command: self.command.clone(),
            status: state.status,
            started_at: self.started_at,
            exit_code: state.exit_code,
            stdout,
            stderr,
            truncated: a || b,
        }
    }
    pub fn output(&self, out: u64, err: u64, limit: usize) -> JobOutput {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (stdout, stdout_cursor, stdout_lost) = state.stdout.read(out, limit, !state.status.is_active());
        let (stderr, stderr_cursor, stderr_lost) = state.stderr.read(err, limit, !state.status.is_active());
        JobOutput {
            job_id: self.id.clone(),
            status: state.status,
            exit_code: state.exit_code,
            stdout,
            stderr,
            stdout_cursor,
            stderr_cursor,
            stdout_lost,
            stderr_lost,
            truncated: stdout_lost
                || stderr_lost
                || stdout_cursor < state.stdout.cursor()
                || stderr_cursor < state.stderr.cursor(),
        }
    }
}

pub struct ShellJobs {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<u64>,
    cleanup: tokio::sync::Mutex<()>,
}

pub struct ShellLaunchPause {
    state: Arc<Mutex<State>>,
    session: String,
}

impl Drop for ShellLaunchPause {
    fn drop(&mut self) {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).paused_sessions.remove(&self.session);
    }
}

#[cfg(windows)]
pub struct ProcessJournal {
    pub(super) record: Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    pub(super) confirm: Box<dyn Fn(&str) + Send + Sync>,
}
impl Default for ShellJobs {
    fn default() -> Self {
        Self { state: Arc::default(), changed: watch::channel(0).0, cleanup: Default::default() }
    }
}
impl cordis::Service for ShellJobs {
    const NAME: &'static str = "studio.shell-jobs";
}
impl ShellJobs {
    #[cfg(windows)]
    pub fn bind_process_journal(
        &self,
        session: &str,
        record: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static,
        confirm: impl Fn(&str) + Send + Sync + 'static,
    ) -> Arc<ProcessJournal> {
        let journal = Arc::new(ProcessJournal { record: Box::new(record), confirm: Box::new(confirm) });
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.journals.retain(|_, journal| journal.strong_count() != 0);
        state.journals.insert(session.into(), Arc::downgrade(&journal));
        journal
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
    pub fn attach_session(&self, session: &str) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            return Err("Shell jobs are stopped".into());
        }
        if !state.leases.contains_key(session) && state.leases.len() >= 1024 {
            return Err("Too many tracked shell job owners".into());
        }
        state.next_lease = state.next_lease.checked_add(1).ok_or("Shell job lease limit reached")?;
        let token = state.next_lease;
        state.leases.entry(session.to_owned()).or_default().insert(token);
        Ok(token)
    }
    pub async fn detach_session(&self, session: &str, token: u64) -> Result<(), String> {
        let jobs = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(leases) = state.leases.get_mut(session) else { return Ok(()) };
            if !leases.remove(&token) || !leases.is_empty() {
                return Ok(());
            }
            let jobs: Vec<_> = state.jobs.values().filter(|job| job.session == session).cloned().collect();
            for job in &jobs {
                job.stop();
            }
            jobs
        };
        self.join(jobs).await
    }
    pub fn list(&self, session: &str) -> Vec<JobSnapshot> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut jobs: Vec<_> = state
            .jobs
            .values()
            .filter(|job| job.session == session && job.published())
            .map(|job| job.snapshot())
            .collect();
        jobs.sort_by(|a, b| (a.started_at, &a.id).cmp(&(b.started_at, &b.id)));
        jobs
    }
    /// Close shell launch admission in the same critical section that checks running
    /// jobs, including jobs still waiting to publish or to confirm termination.
    pub fn try_pause_launches(&self, session: &str) -> Result<ShellLaunchPause, String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped || state.leases.get(session).is_some_and(HashSet::is_empty) {
            return Err("Shell job owner is disconnected".into());
        }
        if state.jobs.values().any(|job| job.session == session && job.status().is_active()) {
            return Err("Shell jobs are still active".into());
        }
        if !state.paused_sessions.insert(session.to_owned()) {
            return Err("Shell launches are already paused".into());
        }
        Ok(ShellLaunchPause { state: self.state.clone(), session: session.to_owned() })
    }

    pub fn session_is_quiescent(&self, session: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // A missing lease entry has never closed admission for this session.
        state.leases.get(session).is_some_and(|leases| leases.is_empty())
            && !state.jobs.values().any(|job| job.session == session && job.status().is_active())
    }
    pub(super) fn get(&self, session: &str, id: &str) -> Result<Arc<Job>, String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .get(id)
            .filter(|job| job.session == session)
            .cloned()
            .ok_or_else(|| "Shell job not found".into())
    }
    pub fn kill(&self, session: &str, id: &str) -> Result<(), String> {
        self.get(session, id)?.stop();
        Ok(())
    }
    pub(super) fn start(&self, ctx: &ToolContext, command: &str, label: &str) -> Result<Arc<Job>, String> {
        let session = ctx.session_id.as_str();
        let cwd = ctx.cwd.as_deref().ok_or("Select a workspace directory first")?;
        let caller = &ctx.cancellation;
        ctx.verify_admitted_write()?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut state);
        if state.stopped {
            return Err("Shell jobs are stopped".into());
        }
        if caller.is_cancelled() {
            return Err("Shell launch cancelled".into());
        }
        if state.leases.get(session).is_some_and(HashSet::is_empty) {
            return Err("Shell job owner is disconnected".into());
        }
        if state.paused_sessions.contains(session) {
            return Err("Shell launches are paused".into());
        }
        let active: Vec<_> = state.jobs.values().filter(|job| job.status().is_active()).collect();
        if active.len() >= GLOBAL_ACTIVE || active.iter().filter(|job| job.session == session).count() >= SESSION_ACTIVE
        {
            return Err("Too many active shell jobs".into());
        }
        // Admission is reserved under this lock before spawning; commands remain gated on Windows stdin until the tree is attached.
        let id = uuid::Uuid::new_v4().to_string();
        #[cfg(windows)]
        let name = format!("Global\\OpenIDEA-MinervaShell-{id}");
        #[cfg(windows)]
        let journal =
            state.journals.get(session).and_then(std::sync::Weak::upgrade).map(|journal| (journal, name.clone()));
        let child = process::spawn(
            cwd,
            #[cfg(windows)]
            &name,
        )?;
        if caller.is_cancelled() {
            return Err("Shell launch cancelled".into());
        }
        let job = Arc::new(Job {
            id,
            session: session.to_owned(),
            label: display_line(label, 256),
            command: display_line(command, 512),
            started_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            state: Mutex::new(JobState {
                status: JobStatus::Running,
                published: false,
                exit_code: None,
                stdout: Default::default(),
                stderr: Default::default(),
            }),
            cancellation: Cancellation::default(),
            launch: watch::channel(false).0,
            worker: Mutex::new(None),
            changed: self.changed.clone(),
        });
        state.jobs.insert(job.id.clone(), job.clone());
        let owned = job.clone();
        let command = command.to_owned();
        let registry = self.state.clone();
        let authority = ctx.clone();
        *job.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(tokio::spawn(async move {
            let result = process::run(
                child,
                &command,
                &owned,
                authority,
                #[cfg(windows)]
                journal,
            )
            .await;
            let mut output = owned.state.lock().unwrap_or_else(|e| e.into_inner());
            output.status =
                if result.as_ref().err().is_some_and(|error| error.starts_with("Shell termination unconfirmed")) {
                    JobStatus::Stopping
                } else if owned.cancellation.is_cancelled() {
                    JobStatus::Stopped
                } else if result.as_ref().is_ok_and(|code| *code == Some(0)) {
                    JobStatus::Completed
                } else {
                    JobStatus::Failed
                };
            match result {
                Ok(code) => output.exit_code = code,
                Err(error) => output.stderr.append(error.as_bytes()),
            }
            drop(output);
            owned.notify();
            Self::prune(&mut registry.lock().unwrap_or_else(|e| e.into_inner()));
        }));
        drop(state);
        job.notify();
        Ok(job)
    }
    fn prune(state: &mut State) {
        let mut settled: Vec<_> = state.jobs.values().filter(|job| !job.status().is_active()).cloned().collect();
        settled.sort_by_key(|job| (job.started_at, job.id.clone()));
        let mut counts = HashMap::<String, usize>::new();
        for job in &settled {
            *counts.entry(job.session.clone()).or_default() += 1;
        }
        let mut total = settled.len();
        for job in settled {
            let count = counts.get_mut(&job.session).unwrap();
            if *count > SESSION_SETTLED || total > GLOBAL_SETTLED {
                state.jobs.remove(&job.id);
                *count -= 1;
                total -= 1;
            }
        }
    }
    fn stop_session_jobs(&self, session: &str) -> Vec<Arc<Job>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let jobs: Vec<_> = state.jobs.values().filter(|job| job.session == session).cloned().collect();
        for job in &jobs {
            job.stop();
        }
        jobs
    }

    pub fn request_stop_session(&self, session: &str) {
        self.stop_session_jobs(session);
    }

    pub async fn stop_session(&self, session: &str) -> Result<(), String> {
        self.join(self.stop_session_jobs(session)).await
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        let jobs = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            let jobs: Vec<_> = state.jobs.values().cloned().collect();
            for job in &jobs {
                job.stop();
            }
            jobs
        };
        self.join(jobs).await
    }
    async fn join(&self, jobs: Vec<Arc<Job>>) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let _serial = tokio::time::timeout_at(deadline, self.cleanup.lock())
            .await
            .map_err(|_| "Shell cleanup coordinator timed out")?;
        for job in jobs {
            let mut lease =
                JoinLease { handle: job.worker.lock().unwrap_or_else(|e| e.into_inner()).take(), job: job.clone() };
            if let Some(handle) = lease.handle.as_mut() {
                let result = tokio::time::timeout_at(deadline, handle)
                    .await
                    .map_err(|_| "Shell process cleanup timed out; termination unconfirmed")?;
                lease.handle.take();
                result.map_err(|_| "Shell process worker failed; termination unconfirmed")?;
            }
            if job.status().is_active() {
                return Err("Shell process termination remains unconfirmed".into());
            }
        }
        Ok(())
    }
}
struct JoinLease {
    job: Arc<Job>,
    handle: Option<JoinHandle<()>>,
}
impl Drop for JoinLease {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            *self.job.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }
    }
}
impl Drop for ShellJobs {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stopped = true;
        for job in state.jobs.values() {
            job.stop();
            if let Some(worker) = job.worker.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                worker.abort();
            }
        }
    }
}

fn display_line(text: &str, limit: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
