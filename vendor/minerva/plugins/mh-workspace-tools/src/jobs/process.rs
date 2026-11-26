use super::registry::Job;
use crate::process_tree::ProcessTree;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStderr, ChildStdout, Command},
};

pub(super) struct ManagedChild {
    child: Child,
    tree: ProcessTree,
    stdout: ChildStdout,
    stderr: ChildStderr,
}
pub(super) fn spawn(cwd: &Path, #[cfg(windows)] name: &str) -> Result<ManagedChild, String> {
    let cwd = cwd.canonicalize().map_err(|e| e.to_string())?;
    if !cwd.is_dir() {
        return Err("Shell working directory is not a directory".into());
    }
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); & ([ScriptBlock]::Create([Console]::In.ReadToEnd()))"]);
        c.creation_flags(0x08000000);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = Command::new("sh");
        c.arg("-s");
        c.process_group(0);
        c
    };
    let mut child = command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let tree = ProcessTree::attach(&child, Some(name))?;
    #[cfg(not(windows))]
    let tree = ProcessTree::attach(&child)?;
    let stdout = child.stdout.take().ok_or("Missing shell stdout")?;
    let stderr = child.stderr.take().ok_or("Missing shell stderr")?;
    Ok(ManagedChild { child, tree, stdout, stderr })
}
fn append(job: &Job, bytes: &[u8], stderr: bool) {
    {
        let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
        if stderr {
            state.stderr.append(bytes)
        } else {
            state.stdout.append(bytes)
        }
    }
    job.notify();
}
async fn drain(reader: &mut (impl AsyncRead + Unpin), job: &Job, stderr: bool) -> Result<(), String> {
    let mut bytes = [0u8; 4096];
    loop {
        let n = reader.read(&mut bytes).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        append(job, &bytes[..n], stderr);
    }
}
pub(super) async fn run(
    mut process: ManagedChild,
    command: &str,
    job: &Job,
    authority: crate::tools::ToolContext,
    #[cfg(windows)] journal: Option<(std::sync::Arc<super::registry::ProcessJournal>, String)>,
) -> Result<Option<i32>, String> {
    let outcome=async {
        let mut launch = job.launch.subscribe();
        tokio::select! { biased;
            _=job.cancellation.cancelled()=>return Err("Shell stopped before launch".into()),
            result=launch.wait_for(|ready| *ready)=>{result.map_err(|_| "Shell launch gate closed")?;}
        }
        #[cfg(windows)]
        if let Some((journal, name)) = &journal {
            let (journal, name) = (journal.clone(), name.clone());
            tokio::task::spawn_blocking(move || (journal.record)(&name))
                .await
                .map_err(|error| error.to_string())??;
            if job.cancellation.is_cancelled() {
                return Err("Shell stopped before launch".into());
            }
        }
        authority.verify_admitted_write()?;
        drop(authority);
        let mut input=process.child.stdin.take().ok_or("Missing shell stdin")?;
        let write=async {input.write_all(command.as_bytes()).await?; input.write_all(b"\n").await?;input.shutdown().await};
        tokio::select! {biased; _=job.cancellation.cancelled()=>return Err("Shell stopped".into()),result=write=>result.map_err(|e|e.to_string())?}
        drop(input);
        let (mut out_open,mut err_open)=(true,true);
        let (mut out,mut err)=([0u8;4096],[0u8;4096]);
        loop {tokio::select! {
            result=process.child.wait()=>return result.map(|s|s.code()).map_err(|e|e.to_string()),
            _=job.cancellation.cancelled()=>return Err("Shell stopped".into()),
            read=process.stdout.read(&mut out),if out_open=>{let n=read.map_err(|e|e.to_string())?;out_open=n!=0;append(job,&out[..n],false)},
            read=process.stderr.read(&mut err),if err_open=>{let n=read.map_err(|e|e.to_string())?;err_open=n!=0;append(job,&err[..n],true)},
        }}
    }.await;
    // Every exit (including pipe/input errors) kills descendants, reaps the leader, then drains boundedly.
    process.tree.kill();
    let _ = process.child.kill().await;
    process.child.wait().await.map_err(|e| format!("Shell termination unconfirmed: {e}"))?;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !process.tree.is_empty()? {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| "Shell termination unconfirmed: the job still has active processes".to_owned())?
    .map_err(|e| format!("Shell termination unconfirmed: {e}"))?;
    #[cfg(windows)]
    if let Some((journal, name)) = journal {
        let _ = tokio::task::spawn_blocking(move || (journal.confirm)(&name)).await;
    }
    let drained = tokio::time::timeout(Duration::from_secs(2), async {
        let (out, err) = tokio::join!(drain(&mut process.stdout, job, false), drain(&mut process.stderr, job, true));
        out?;
        err
    })
    .await
    .map_err(|_| "Shell output drain timed out".to_owned())?;
    drained?;
    outcome
}
