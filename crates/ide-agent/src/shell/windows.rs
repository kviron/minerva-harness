use super::{Ran, tail};
use crate::workspace_writes::WorkspaceWrite;
use ide_core::{CancellationToken, process};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

// The launcher blocks on stdin until it belongs to the job. It then passes the raw
// /S /C argument line to the same cmd.exe that previously ran the command directly.
const LAUNCHER: &str = r#"
$ErrorActionPreference = 'Stop'
$reader = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.Encoding]::UTF8)
$arguments = $reader.ReadToEnd()
if (!$arguments) { exit 1 }
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = if ($env:COMSPEC) { $env:COMSPEC } else { 'cmd.exe' }
$start.Arguments = $arguments
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$child = [Diagnostics.Process]::Start($start)
$child.StandardInput.Close()
$out = $child.StandardOutput.BaseStream.CopyToAsync([Console]::OpenStandardOutput())
$err = $child.StandardError.BaseStream.CopyToAsync([Console]::OpenStandardError())
$child.WaitForExit()
[void][Threading.Tasks.Task]::WaitAll([Threading.Tasks.Task[]]@($out, $err), 1000)
exit $child.ExitCode
"#;

pub(crate) async fn run(command: &str, cwd: &Path, cancel: &CancellationToken, writer: Option<&WorkspaceWrite>) -> Ran {
    match run_in_job(command, cwd, cancel, writer).await {
        Ok(ran) => ran,
        Err(error) => Ran { error: Some(error), ..Ran::default() },
    }
}

async fn run_in_job(
    command: &str,
    cwd: &Path,
    cancel: &CancellationToken,
    writer: Option<&WorkspaceWrite>,
) -> Result<Ran, String> {
    if cancel.is_cancelled() {
        return Err("Stopped".into());
    }
    let mut launcher = Command::new("powershell.exe");
    launcher.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", LAUNCHER]);
    launcher.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    process::prepare(launcher.as_std_mut());
    let mut child = launcher.spawn().map_err(|error| format!("Cannot run it: {error}"))?;
    let job_name = format!("Global\\OpenIDEA-AgentShell-{}", uuid::Uuid::new_v4());
    let job = child
        .id()
        .and_then(|pid| process::Job::for_pid_named(pid, &job_name))
        .ok_or("Cannot contain the shell process")?;
    if let Some(writer) = writer {
        writer.record_windows_job(&job_name).map_err(|error| error.to_string())?;
    }
    let mut input = child.stdin.take().ok_or("Cannot open shell input")?;
    let mut stdout = child.stdout.take().ok_or("Cannot read shell output")?;
    let mut stderr = child.stderr.take().ok_or("Cannot read shell errors")?;
    let arguments = format!("/D /S /C \"{command}\"");
    let output = tokio::spawn(async move {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        (tail(&out), tail(&err))
    });
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Err("Stopped".to_owned()),
        result = async {
            input.write_all(arguments.as_bytes()).await?;
            drop(input);
            child.wait().await
        } => result.map_err(|error| error.to_string()),
    };
    let cleanup = tokio::time::timeout(Duration::from_secs(2), async {
        if job.try_is_running()? && !job.terminate() {
            return Err(std::io::Error::last_os_error());
        }
        child.wait().await?;
        while job.try_is_running()? {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), std::io::Error>(())
    })
    .await;
    match cleanup {
        Ok(Ok(())) => {
            if let Some(writer) = writer {
                writer.clear_windows_job_record(&job_name);
            }
        }
        failure => {
            output.abort();
            if let Some(writer) = writer.cloned() {
                tokio::spawn(async move {
                    while !matches!(job.try_is_running(), Ok(false)) {
                        job.terminate();
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    let _ = child.wait().await;
                    writer.clear_windows_job_record(&job_name);
                });
            }
            return Err(match failure {
                Ok(Err(error)) => format!("Shell termination could not be confirmed: {error}"),
                _ => "Shell termination could not be confirmed".into(),
            });
        }
    }
    let (stdout, stderr) = output.await.map_err(|error| error.to_string())?;
    Ok(Ran { code: result?.code(), stdout, stderr, error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contained_shell_preserves_cmd_quoting_and_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let ran = run(
            "echo quoted > \"with spaces.txt\" & type \"with spaces.txt\" & exit /b 7",
            dir.path(),
            &CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(ran.error, None);
        assert_eq!(ran.code, Some(7));
        assert_eq!(ran.stdout.trim(), "quoted");
    }

    #[tokio::test]
    async fn contained_shell_cancellation_settles_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let (ran, ()) =
            tokio::join!(run("echo ready>ready.txt & ping -n 30 127.0.0.1 >nul", dir.path(), &cancel, None), async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !dir.path().join("ready.txt").is_file() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
                cancel.cancel();
            });
        assert_eq!(ran.error.as_deref(), Some("Stopped"));
    }
}
