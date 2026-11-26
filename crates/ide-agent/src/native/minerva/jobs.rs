//! Projects owned shell jobs onto OpenIDEA's existing background-task protocol.
use super::Engine;
use ide_minerva::minerva_runtime::Runtime;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const SNAPSHOT_OUTPUT_BYTES: usize = 64 * 1024;
static SNAPSHOT_SEQUENCE: parking_lot::Mutex<u64> = parking_lot::Mutex::new(0);

impl Engine {
    pub(super) fn watch_jobs(&self, runtime: &Arc<Runtime>, session: &str) -> Result<(), String> {
        let jobs = runtime.plugins.jobs()?;
        let client = self.client.get().ok_or("Minerva connection is closed")?.clone();
        if client.is_closed() {
            return Err("Minerva connection is closed".into());
        }
        if !self.job_sessions.lock().insert(session.into()) {
            return Ok(());
        }
        let token = match jobs.attach_session(session) {
            Ok(token) => token,
            Err(error) => {
                self.job_sessions.lock().remove(session);
                return Err(error);
            }
        };
        let session = session.to_owned();
        let watched = self.job_sessions.clone();
        let runs = self.runs.clone();
        let runtime = runtime.clone();
        self.runtime.spawn(async move {
            let mut changes = jobs.subscribe();
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut previous = Value::Null;
            let mut last_sent = Instant::now();
            loop {
                tokio::select! {
                    _ = client.closed() => break,
                    _ = interval.tick() => {},
                }
                let dirty = changes.has_changed().unwrap_or(true);
                if !dirty && previous.as_array().is_some_and(Vec::is_empty) {
                    continue;
                }
                // Mark the observed changes before sampling so a concurrent job update stays pending.
                changes.borrow_and_update();
                // Sampling also recovers a roster received during session/load replay.
                let Ok((revision, tasks, active)) = snapshot(&runtime, &session) else { break };
                let snapshot = json!(tasks);
                if dirty || snapshot != previous || active || last_sent.elapsed() >= Duration::from_secs(5) {
                    let update = json!({"sessionId":session,"update":{
                        "sessionUpdate":crate::BACKGROUND_TASKS_UPDATE,"tasks":snapshot,
                        "backgroundRevision":revision,
                    }});
                    if client.notify("session/update", update).is_err() {
                        break;
                    }
                    previous = snapshot;
                    last_sent = Instant::now();
                }
            }
            if let Some(run) = runs.lock().get(&session).cloned() {
                run.cancelled.store(true, super::Ordering::Release);
                let _ = runtime.cancel_run(&session, &run.id);
            }
            if let Err(error) = jobs.detach_session(&session, token).await {
                tracing::warn!(%error, "Minerva shell job cleanup did not settle");
            }
            watched.lock().remove(&session);
        });
        Ok(())
    }
}

pub(super) fn snapshot(runtime: &Runtime, session: &str) -> Result<(u64, Vec<Value>, bool), String> {
    let jobs = runtime.plugins.jobs()?;
    let (revision, listed) = {
        let mut sequence = SNAPSHOT_SEQUENCE.lock();
        *sequence += 1;
        (*sequence, jobs.list(session))
    };
    let mut tasks = Vec::new();
    let active = listed.iter().any(|job| job.status.is_active());
    let output_limit = (SNAPSHOT_OUTPUT_BYTES / listed.len().max(1)).min(8 * 1024);
    for job in listed {
        let mut label = job.command.lines().next().unwrap_or_default().to_owned();
        if label.len() > 512 {
            let mut end = 512;
            while !label.is_char_boundary(end) {
                end -= 1;
            }
            label.truncate(end);
        }
        let mut output = job.stdout;
        if !job.stderr.is_empty() {
            output.push_str("\n[stderr]\n");
            output.push_str(&job.stderr);
        }
        if job.truncated {
            output.insert_str(0, "[earlier output omitted]\n");
        }
        tasks.push(json!({
            "task_id":job.id,"task_type":"local_bash","description":label,
            "started_ms":job.started_at,"output":crate::background::output_tail_with_limit(&output,output_limit),
            "status":job.status,
        }));
    }
    Ok((revision, tasks, active))
}
