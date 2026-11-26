use super::{registry::Job, ShellJobs};
use crate::{
    native,
    tools::{Cancellation, ToolContext, ToolDefinition, ToolRegistry},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

struct ForegroundLease {
    job: Arc<Job>,
    armed: bool,
}
impl ForegroundLease {
    fn admit(&mut self, caller: &Cancellation, background: bool) -> Result<(), String> {
        let mut state = self.job.state.lock().unwrap_or_else(|e| e.into_inner());
        if caller.is_cancelled() {
            drop(state);
            self.job.stop();
            return Err("Shell launch cancelled before admission".into());
        }
        self.job.launch.send_replace(true);
        state.published = background;
        self.armed = !background;
        drop(state);
        if background {
            self.job.notify();
        }
        Ok(())
    }
    fn handoff(&mut self, caller: &Cancellation) -> Result<(), String> {
        let mut state = self.job.state.lock().unwrap_or_else(|e| e.into_inner());
        if caller.is_cancelled() {
            drop(state);
            self.job.stop();
            return Err("Shell launch cancelled before handoff".into());
        }
        let publish = state.status.is_active() && !state.published;
        if publish {
            state.published = true;
        }
        self.armed = false;
        drop(state);
        if publish {
            self.job.notify();
        }
        Ok(())
    }
}
impl Drop for ForegroundLease {
    fn drop(&mut self) {
        if self.armed {
            self.job.stop();
        }
    }
}

pub fn register_native_with_jobs(registry: &ToolRegistry, jobs: Arc<ShellJobs>) -> Result<(), String> {
    native::register_without_shell(registry)?;
    for (name,description,properties,required,effect) in [
        ("shell","Execute a workspace shell command. With run_in_background it immediately returns jobId. Otherwise wait up to timeoutMs, then keep the SAME process running and return jobId. Read later output with job_output; use job_kill to stop it.",
          json!({"command":{"type":"string","minLength":1,"maxLength":65536},"label":{"type":"string","maxLength":256},"run_in_background":{"type":"boolean"},"timeoutMs":{"type":"integer","minimum":1,"maximum":600000}}),vec!["command"],true),
        ("job_list","List metadata for this session's background shell jobs. Read output explicitly with job_output.",json!({}),vec![],false),
        ("job_output","Read bounded shell output by independent stdoutCursor/stderrCursor byte offsets. Output can be lost when its bounded buffer wraps; inspect stdoutLost/stderrLost. Poll while status is running/stopping. Reading does not stop the job.",
          json!({"jobId":{"type":"string"},"stdoutCursor":{"type":"integer","minimum":0},"stderrCursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":16384},"waitMs":{"type":"integer","minimum":0,"maximum":10000}}),vec!["jobId"],false),
        ("job_kill","Stop a shell job owned by this session, including its descendants. This resource cleanup remains available in read-only mode.",json!({"jobId":{"type":"string"}}),vec!["jobId"],false),
    ] {
        let jobs=jobs.clone();
        registry.register_with_timeout("native.workspace",ToolDefinition{name:name.into(),description:description.into(),
            input_schema:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
            output_schema:json!({"type":"object"}),side_effect:effect},Duration::from_secs(900),move |ctx,args| {
            let jobs=jobs.clone();Box::pin(async move {dispatch(&jobs,name,ctx,args).await})
        })?;
    }
    Ok(())
}
async fn dispatch(jobs: &ShellJobs, name: &str, ctx: ToolContext, args: Value) -> Result<Value, String> {
    if name == "job_list" {
        return Ok(json!({"jobs":jobs.list(&ctx.session_id).into_iter().map(metadata).collect::<Vec<_>>()}));
    }
    if name == "job_kill" {
        let id = string(&args, "jobId")?;
        jobs.kill(&ctx.session_id, id)?;
        return Ok(metadata(jobs.get(&ctx.session_id, id)?.snapshot()));
    }
    if name == "job_output" {
        let job = jobs.get(&ctx.session_id, string(&args, "jobId")?)?;
        let out = args["stdoutCursor"].as_u64().unwrap_or(0);
        let err = args["stderrCursor"].as_u64().unwrap_or(0);
        let limit = args["limit"].as_u64().unwrap_or(8192).clamp(1, 16384) as usize;
        let mut changed = jobs.subscribe();
        let wait = async {
            loop {
                let output = job.output(out, err, limit);
                if !output.status.is_active() || output.stdout_cursor != out || output.stderr_cursor != err {
                    return;
                }
                if changed.changed().await.is_err() {
                    return;
                }
            }
        };
        let _ =
            tokio::time::timeout(Duration::from_millis(args["waitMs"].as_u64().unwrap_or(0).min(10000)), wait).await;
        return Ok(json!(job.output(out, err, limit)));
    }
    if ctx.cancellation.is_cancelled() {
        return Err("Shell launch cancelled".into());
    }
    let command = string(&args, "command")?;
    let job = jobs.start(&ctx, command, args["label"].as_str().unwrap_or(command))?;
    let mut lease = ForegroundLease { job: job.clone(), armed: true };
    let background = args["run_in_background"].as_bool().unwrap_or(false);
    lease.admit(&ctx.cancellation, background)?;
    if !background {
        let mut changed = jobs.subscribe();
        let deadline =
            tokio::time::sleep(Duration::from_millis(args["timeoutMs"].as_u64().unwrap_or(120000).clamp(1, 600000)));
        tokio::pin!(deadline);
        let mut ticker = tokio::time::interval(Duration::from_millis(150));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = Value::Null;
        while job.status().is_active() {
            tokio::select! {
                _=ctx.cancellation.cancelled()=>{job.stop();return Err("Shell cancelled".into())},
                _=&mut deadline=>break,
                _=changed.changed()=>{},
                _=ticker.tick()=>{
                    let output=job.snapshot();
                    let progress=json!({"stdout":output.stdout,"stderr":output.stderr,"truncated":output.truncated});
                    if progress!=previous && (!output.stdout.is_empty() || !output.stderr.is_empty()) {
                        if let Some(sink)=&ctx.sink { sink.emit(&ctx,"shell",&args,"running",Some(&Ok(progress.clone())))?; }
                        previous=progress;
                    }
                }
            }
        }
    }
    if !background {
        lease.handoff(&ctx.cancellation)?;
    }
    Ok(json!(job.output(0, 0, 8192)))
}
fn metadata(job: super::JobSnapshot) -> Value {
    json!({"id":job.id,"label":job.label,"command":job.command,"status":job.status,"startedAt":job.started_at,"exitCode":job.exit_code,"truncated":job.truncated})
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key].as_str().ok_or_else(|| format!("Missing {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_admission_never_releases_the_command_gate() {
        let dir = tempfile::tempdir().unwrap();
        let jobs = ShellJobs::default();
        let command = if cfg!(windows) { "Set-Content -LiteralPath 'unwanted' -Value 'bad'" } else { "touch unwanted" };
        let ctx = super::super::tests::context("one", dir.path());
        let job = jobs.start(&ctx, command, "provisional").unwrap();
        let caller = Cancellation::default();
        caller.cancel();
        let mut lease = ForegroundLease { job: job.clone(), armed: true };
        assert!(lease.admit(&caller, true).is_err());
        drop(lease);
        jobs.shutdown().await.unwrap();
        assert!(!dir.path().join("unwanted").exists());
        assert_eq!(job.snapshot().status, super::super::JobStatus::Stopped);
    }
}
