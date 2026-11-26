//! Session-owned shell jobs. Process ownership is independent of model-turn cancellation after handoff.
mod output;
mod process;
mod registry;
#[cfg(test)]
mod tests;
mod tools;

pub use registry::{ShellJobs, ShellLaunchPause};
use serde::Serialize;
pub use tools::register_native_with_jobs;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Failed,
    Stopped,
}
impl JobStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub label: String,
    pub command: String,
    pub status: JobStatus,
    pub started_at: u64,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutput {
    pub job_id: String,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_cursor: u64,
    pub stderr_cursor: u64,
    pub stdout_lost: bool,
    pub stderr_lost: bool,
    pub truncated: bool,
}
