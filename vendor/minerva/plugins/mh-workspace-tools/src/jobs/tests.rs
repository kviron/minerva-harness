use super::*;
use crate::tools::{Cancellation, InteractionBroker, PermissionPolicy, ToolContext, ToolRegistry};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
fn context(session: &str, cwd: &Path) -> ToolContext {
    ToolContext {
        session_id: session.into(),
        run_id: "run".into(),
        call_id: "call".into(),
        cwd: Some(cwd.into()),
        policy: PermissionPolicy::Allow,
        live_policy: None,
        cancellation: Cancellation::default(),
        interactions: Arc::new(InteractionBroker::default()),
        sink: None,
    }
}
fn command() -> &'static str {
    if cfg!(windows) {
        "Write-Output 'begin'; Start-Sleep -Milliseconds 350; Write-Output 'end'"
    } else {
        "printf 'begin\n'; sleep 0.35; printf 'end\n'"
    }
}
async fn settled(jobs: &ShellJobs, session: &str, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut changed = jobs.subscribe();
        loop {
            let job = jobs.get(session, id).unwrap().snapshot();
            if !job.status.is_active() {
                return job;
            }
            changed.changed().await.unwrap();
        }
    })
    .await
    .expect("shell must settle")
}
fn setup() -> (Arc<ShellJobs>, ToolRegistry) {
    let jobs = Arc::new(ShellJobs::default());
    let registry = ToolRegistry::default();
    register_native_with_jobs(&registry, jobs.clone()).unwrap();
    (jobs, registry)
}
#[tokio::test]
async fn background_survives_turn_cancel_and_read_cursors_are_not_consumed_by_ui() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let ctx = context("one", dir.path());
    let answer = registry
        .snapshot()
        .execute("shell", json!({"command":command(),"run_in_background":true}), ctx.clone())
        .await
        .unwrap();
    let id = answer["jobId"].as_str().unwrap();
    ctx.cancellation.cancel();
    let final_job = settled(&jobs, "one", id).await;
    assert_eq!(final_job.status, JobStatus::Completed);
    assert!(final_job.stdout.contains("end"));
    assert_eq!(jobs.list("one").len(), 1);
    let listed = registry.snapshot().execute("job_list", json!({}), context("one", dir.path())).await.unwrap();
    assert!(listed["jobs"][0].get("stdout").is_none());
    assert!(listed["jobs"][0].get("stderr").is_none());
    let a = jobs.get("one", id).unwrap().output(0, 0, 8192);
    let _ = jobs.list("one");
    let b = jobs.get("one", id).unwrap().output(0, 0, 8192);
    assert_eq!(a.stdout, b.stdout);
    assert!(jobs.get("two", id).is_err());
    assert!(jobs.kill("two", id).is_err());
    let mut read_only = context("one", dir.path());
    read_only.policy = PermissionPolicy::ReadOnly;
    let killed = registry.snapshot().execute("job_kill", json!({"jobId":id}), read_only.clone()).await.unwrap();
    assert!(killed.get("stdout").is_none());
    assert!(killed.get("stderr").is_none());
    assert!(registry.snapshot().execute("shell", json!({"command":"echo forbidden"}), read_only).await.is_err());
    jobs.shutdown().await.unwrap();
}
#[tokio::test]
async fn foreground_timeout_promotes_the_same_process_once() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let command = if cfg!(windows) {
        "Add-Content -LiteralPath 'count' -Value 'one'; Start-Sleep -Milliseconds 350; Write-Output 'done'"
    } else {
        "printf 'one\n' >> count; sleep 0.35; printf 'done\n'"
    };
    let answer = registry
        .snapshot()
        .execute("shell", json!({"command":command,"timeoutMs":10}), context("one", dir.path()))
        .await
        .unwrap();
    let id = answer["jobId"].as_str().unwrap();
    assert!(jobs.get("one", id).unwrap().snapshot().status.is_active());
    assert!(settled(&jobs, "one", id).await.stdout.contains("done"));
    assert_eq!(std::fs::read_to_string(dir.path().join("count")).unwrap().lines().collect::<Vec<_>>(), vec!["one"]);
    jobs.shutdown().await.unwrap();
}
#[tokio::test]
async fn dropping_foreground_call_kills_instead_of_detaching() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let admitted = registry.snapshot();
    let ctx = context("one", dir.path());
    let cmd = if cfg!(windows) {
        "Set-Content -LiteralPath 'started' -Value 'ready'; Start-Sleep -Milliseconds 500; Set-Content -LiteralPath 'unwanted' -Value 'bad'"
    } else {
        "touch started; sleep 0.5; touch unwanted"
    };
    let task =
        tokio::spawn(async move { admitted.execute("shell", json!({"command":cmd,"timeoutMs":600000}), ctx).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !dir.path().join("started").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(jobs.list("one").is_empty());
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!dir.path().join("unwanted").exists());

    jobs.shutdown().await.unwrap();
}
#[tokio::test]
async fn last_peer_detach_does_not_stop_jobs_admitted_after_its_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let a = jobs.attach_session("one").unwrap();
    let b = jobs.attach_session("one").unwrap();
    let old = registry
        .snapshot()
        .execute("shell", json!({"command":command(),"run_in_background":true}), context("one", dir.path()))
        .await
        .unwrap();
    let old = old["jobId"].as_str().unwrap().to_owned();
    jobs.detach_session("one", a).await.unwrap();
    assert!(jobs.get("one", &old).unwrap().snapshot().status.is_active());
    let detach = tokio::spawn({
        let jobs = jobs.clone();
        async move { jobs.detach_session("one", b).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut changed = jobs.subscribe();
        while jobs.get("one", &old).unwrap().snapshot().status == JobStatus::Running {
            changed.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let c = jobs.attach_session("one").unwrap();
    let new = registry
        .snapshot()
        .execute("shell", json!({"command":command(),"run_in_background":true}), context("one", dir.path()))
        .await
        .unwrap();
    detach.await.unwrap().unwrap();
    jobs.detach_session("one", b).await.unwrap();
    assert_ne!(jobs.get("one", new["jobId"].as_str().unwrap()).unwrap().snapshot().status, JobStatus::Stopped);
    jobs.detach_session("one", c).await.unwrap();
    jobs.shutdown().await.unwrap();
}
#[tokio::test]
async fn output_is_bounded_before_decode_and_reports_evicted_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let cmd = if cfg!(windows) {
        "[Console]::Out.Write(('x' * 100000)); [Console]::Error.Write(('y' * 100000))"
    } else {
        "head -c 100000 /dev/zero | tr '\\0' x; head -c 100000 /dev/zero | tr '\\0' y >&2"
    };
    let result =
        registry.snapshot().execute("shell", json!({"command":cmd}), context("one", dir.path())).await.unwrap();
    let job = jobs.get("one", result["jobId"].as_str().unwrap()).unwrap();
    let output = job.output(0, 0, 16384);
    assert!(output.stdout_lost && output.stderr_lost && output.truncated);
    assert!(output.stdout.len() <= 16384 && output.stderr.len() <= 16384);
    assert!(job.snapshot().stdout.len() <= 8192);
    assert!(jobs.list("one").is_empty(), "completed foreground calls are not background history");
    jobs.shutdown().await.unwrap();
}
#[tokio::test]
async fn admission_is_bounded_before_launch_and_shutdown_rejects_retained_callers() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = Arc::new(ShellJobs::default());
    let mut reserved = Vec::new();
    for _ in 0..10 {
        reserved.push(jobs.start("one", dir.path(), "echo never", "reserved", &Cancellation::default()).unwrap());
    }
    assert!(jobs.start("one", dir.path(), "echo unwanted", "overflow", &Cancellation::default()).is_err());
    assert!(jobs.list("one").is_empty());
    jobs.shutdown().await.unwrap();
    assert!(reserved.iter().all(|job| job.status() == JobStatus::Stopped));
    assert!(jobs.start("one", dir.path(), "echo unwanted", "after shutdown", &Cancellation::default()).is_err());
    assert!(jobs.attach_session("one").is_err());
}
#[tokio::test]
async fn plugin_disposal_shuts_down_even_with_service_arc_retained() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = cordis::Context::new();
    let _tools = ctx.provide(Arc::new(ToolRegistry::default())).unwrap();
    let _context =
        ctx.provide(Arc::new(mh_context::ContextService::open(&dir.path().join("context.sqlite")).unwrap())).unwrap();
    let fiber = ctx.spawn(cordis::PreparedPlugin::from_input(crate::WorkspaceToolsPlugin(true), ())).await.unwrap();
    let jobs = ctx.try_service::<ShellJobs>().unwrap();
    let gated = jobs.start("one", dir.path(), "echo gated", "gated", &Cancellation::default()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), fiber.dispose()).await.unwrap().unwrap();
    assert!(ctx.try_service::<ShellJobs>().is_err());
    assert_eq!(gated.status(), JobStatus::Stopped);
    assert!(jobs.list("one").iter().all(|j| !j.status.is_active()));
    assert!(jobs.start("one", dir.path(), "echo unwanted", "after dispose", &Cancellation::default()).is_err());
}

#[tokio::test]
async fn closed_native_owner_rejects_late_launch_until_a_new_attach() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = ShellJobs::default();
    let token = jobs.attach_session("one").unwrap();
    jobs.detach_session("one", token).await.unwrap();
    assert!(jobs.start("one", dir.path(), "echo unwanted", "late", &Cancellation::default()).is_err());
    let token = jobs.attach_session("one").unwrap();
    jobs.start("one", dir.path(), "echo gated", "new", &Cancellation::default()).unwrap();
    jobs.detach_session("one", token).await.unwrap();
    jobs.shutdown().await.unwrap();
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // A retained process object is not evidence that the process is still running.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0;
        let read = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        read != 0 && code == 259
    }
}
#[cfg(not(windows))]
fn process_alive(pid: u32) -> bool {
    if std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|text| text.split(") ").nth(1).is_some_and(|tail| tail.starts_with('Z')))
    {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
#[tokio::test]
async fn stopping_a_job_kills_its_descendants_and_reaps_the_leader() {
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let command = if cfg!(windows) {
        "$p=Start-Process -FilePath powershell.exe -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30' -PassThru; Set-Content -LiteralPath 'child.pid' -Value $p.Id; Write-Output 'ready'; Wait-Process -Id $p.Id"
    } else {
        "sleep 30 & echo $! > child.pid; wait"
    };
    let result = registry
        .snapshot()
        .execute("shell", json!({"command":command,"run_in_background":true}), context("one", dir.path()))
        .await
        .unwrap();
    let pid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(dir.path().join("child.pid")) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(process_alive(pid));
    let id = result["jobId"].as_str().unwrap();
    jobs.kill("one", id).unwrap();
    assert_eq!(settled(&jobs, "one", id).await.status, JobStatus::Stopped);
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_alive(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    jobs.shutdown().await.unwrap();
}

#[tokio::test]
async fn foreground_progress_uses_bounded_metadata_and_one_final_result() {
    struct Sink(std::sync::Mutex<Vec<(String, Option<serde_json::Value>)>>);
    impl crate::tools::ToolEventSink for Sink {
        fn emit(
            &self,
            _: &ToolContext,
            _: &str,
            _: &serde_json::Value,
            status: &str,
            result: Option<&Result<serde_json::Value, String>>,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push((status.into(), result.and_then(|r| r.as_ref().ok().cloned())));
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (jobs, registry) = setup();
    let sink = Arc::new(Sink(Default::default()));
    let mut ctx = context("one", dir.path());
    ctx.sink = Some(sink.clone());
    let progress_command = if cfg!(windows) {
        "Write-Output ('x' * 10000); Write-Output 'begin'; Start-Sleep -Milliseconds 500; Write-Output 'end'"
    } else {
        "printf '%10000s' '' | tr ' ' x; printf '\nbegin\n'; sleep 0.5; printf 'end\n'"
    };
    registry.snapshot().execute("shell", json!({"command":progress_command}), ctx).await.unwrap();
    {
        let events = sink.0.lock().unwrap();
        assert!(events.iter().any(|(status, value)| status == "running"
            && value
                .as_ref()
                .is_some_and(|v| v["stdout"].as_str().is_some_and(|s| s.contains("begin") && !s.contains("end")))));
        assert_eq!(events.iter().filter(|(status, _)| status == "completed").count(), 1);
        assert!(events
            .iter()
            .filter_map(|(_, value)| value.as_ref())
            .all(|v| v["stdout"].as_str().is_none_or(|s| s.len() <= 8192)));
    }
    assert!(jobs.list("one").is_empty());
    jobs.shutdown().await.unwrap();
}
