//! Serializes overlapping checkout mutations among live hosts of the same user.

use ide_core::{IdeError, IdeResult};
use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub(super) struct WorkspaceWrite {
    _reservation: Arc<Reservation>,
}

pub(super) struct WorkspaceTurn(WorkspaceWrite);

#[cfg(windows)]
impl minerva_plugin_sdk::delegation::NativeProcessJournal for WorkspaceTurn {
    fn record(&self, job: &str) -> Result<(), String> {
        self.0.record_windows_job(job).map_err(|error| error.to_string())
    }

    fn confirm_stopped(&self, job: &str) {
        if matches!(ide_core::process::Job::named_is_running(job), Ok(false)) {
            self.0.clear_windows_job_record(job);
        }
    }
}

impl Drop for WorkspaceTurn {
    fn drop(&mut self) {
        self.0._reservation.turn_active.store(false, Ordering::Release);
    }
}

struct Reservation {
    turn_active: AtomicBool,
    // Drop the file handle before removing its directory on Windows.
    _owner: File,
    #[cfg(windows)]
    directory: parking_lot::Mutex<tempfile::TempDir>,
    #[cfg(not(windows))]
    _directory: tempfile::TempDir,
}

impl WorkspaceWrite {
    pub(super) fn claim_turn(&self) -> IdeResult<WorkspaceTurn> {
        self._reservation
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| IdeError::unavailable("A turn already holds this checkout reservation"))?;
        Ok(WorkspaceTurn(self.clone()))
    }

    #[cfg(windows)]
    pub(super) fn record_windows_job(&self, name: &str) -> IdeResult<()> {
        use std::io::Write;
        let mut directory = self._reservation.directory.lock();
        let path = windows_job_marker(directory.path(), name);
        let mut marker = match File::create_new(&path) {
            Ok(marker) => marker,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && fs::read(&path)? == name.as_bytes() => {
                File::options().write(true).open(&path)?.sync_all()?;
                directory.disable_cleanup(true);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        marker.write_all(name.as_bytes())?;
        marker.sync_all()?;
        directory.disable_cleanup(true);
        Ok(())
    }

    #[cfg(windows)]
    pub(super) fn clear_windows_job_record(&self, name: &str) {
        let mut directory = self._reservation.directory.lock();
        let _ = fs::remove_file(windows_job_marker(directory.path(), name));
        if windows_job_markers(directory.path()).is_ok_and(|markers| markers.is_empty()) {
            directory.disable_cleanup(false);
        }
    }

    pub(super) async fn acquire(roots: Vec<PathBuf>) -> IdeResult<Self> {
        super::blocking(move || Self::reserve(&roots)).await
    }

    fn reserve(roots: &[PathBuf]) -> IdeResult<Self> {
        let registry = dirs::data_local_dir()
            .ok_or_else(|| IdeError::unavailable("No local data directory for checkout admission"))?
            .join("OpenIDEA")
            .join("agent-writes-v1");
        Self::reserve_in(&registry, roots)
    }

    fn reserve_in(registry: &Path, roots: &[PathBuf]) -> IdeResult<Self> {
        let roots = roots
            .iter()
            .map(|root| {
                let repo = ide_vcs::Repo::discover(root)?;
                let path = dunce::canonicalize(repo.as_ref().map_or(root.as_path(), |repo| repo.root()))?;
                #[cfg(windows)]
                let path = PathBuf::from(path.to_string_lossy().to_lowercase());
                Ok(path)
            })
            .collect::<IdeResult<Vec<_>>>()?;
        fs::create_dir_all(registry)?;
        let admission = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(registry.join("admission.lock"))?;
        admission.lock()?;
        for entry in fs::read_dir(registry)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !entry.file_name().to_string_lossy().starts_with("writer-") {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
            let Some(held) = live_roots(&entry.path())? else {
                continue;
            };
            if held.iter().any(|held| roots.iter().any(|path| overlaps(held, path))) {
                return Err(IdeError::unavailable("Another agent operation is changing this checkout"));
            }
        }
        let directory = tempfile::Builder::new().prefix("writer-").tempdir_in(registry)?;
        let owner = File::create_new(directory.path().join("owner.lock"))?;
        owner.lock()?;
        let data = serde_json::to_vec(&roots).map_err(std::io::Error::other)?;
        use std::io::Write;
        let mut manifest = File::create_new(directory.path().join("roots.json"))?;
        manifest.write_all(&data)?;
        manifest.sync_all()?;
        Ok(Self {
            _reservation: Arc::new(Reservation {
                turn_active: AtomicBool::new(false),
                _owner: owner,
                #[cfg(windows)]
                directory: parking_lot::Mutex::new(directory),
                #[cfg(not(windows))]
                _directory: directory,
            }),
        })
    }
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn live_roots(directory: &Path) -> IdeResult<Option<Vec<PathBuf>>> {
    let owner = match File::options().read(true).write(true).open(directory.join("owner.lock")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match owner.try_lock() {
        Ok(()) => recovered_roots(directory, owner),
        Err(TryLockError::WouldBlock) => {
            match fs::read(directory.join("roots.json")) {
                Ok(data) => Ok(Some(serde_json::from_slice(&data).map_err(std::io::Error::other)?)),
                // The writer can release its lock and clean up while this scan is in progress.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && owner.try_lock().is_ok() => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

fn recovered_roots(directory: &Path, owner: File) -> IdeResult<Option<Vec<PathBuf>>> {
    #[cfg(windows)]
    {
        let markers = match windows_job_markers(directory) {
            Ok(markers) => markers,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        for marker in &markers {
            let name = match fs::read_to_string(marker) {
                Ok(name) => name,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if ide_core::process::Job::named_is_running(&name).unwrap_or(true) {
                let data = fs::read(directory.join("roots.json"))?;
                return Ok(Some(serde_json::from_slice(&data).map_err(std::io::Error::other)?));
            }
        }
        if markers.is_empty() {
            return Ok(None);
        }
        // Admission holds the registry lock and no new writer reuses this directory.
        // Windows needs the owner handle closed before its file can be removed.
        drop(owner);
        for marker in markers {
            let _ = fs::remove_file(marker);
        }
        for name in ["roots.json", "owner.lock"] {
            let _ = fs::remove_file(directory.join(name));
        }
        let _ = fs::remove_dir(directory);
    }
    #[cfg(not(windows))]
    let _ = (directory, owner);
    Ok(None)
}

#[cfg(windows)]
fn windows_job_marker(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("windows-job-{}", blake3::hash(name.as_bytes()).to_hex()))
}

#[cfg(windows)]
fn windows_job_markers(directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut markers = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "windows-job" || name.starts_with("windows-job-") {
            markers.push(entry.path());
        }
    }
    Ok(markers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn minerva_background_command_recovers_after_host_exit() {
        const CHILD: &str = "OPENIDEA_TEST_CRASH_MINERVA_HOST";
        if let Some(root) = std::env::var_os(CHILD) {
            let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            runtime.block_on(exit_with_running_minerva_command(PathBuf::from(root)));
            unreachable!();
        }
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let registry = dir.path().join("registry");
        fs::create_dir(&checkout).unwrap();
        let host = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workspace_writes::tests::minerva_background_command_recovers_after_host_exit"])
            .env(CHILD, dir.path())
            .output()
            .unwrap();
        assert_eq!(host.status.code(), Some(17), "{}", String::from_utf8_lossy(&host.stdout));
        assert!(checkout.join("running").is_file());
        let record =
            fs::read_dir(&registry).unwrap().map(|entry| entry.unwrap().path()).find(|path| path.is_dir()).unwrap();
        let [marker] = windows_job_markers(&record).unwrap().try_into().unwrap();
        let name = fs::read_to_string(marker).unwrap();
        assert!(name.starts_with("Global\\OpenIDEA-MinervaShell-"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if WorkspaceWrite::reserve_in(&registry, std::slice::from_ref(&checkout)).is_ok() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the checkout was still fenced");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!ide_core::process::Job::named_is_running(&name).unwrap());
        assert!(!record.exists());
    }

    #[cfg(windows)]
    async fn exit_with_running_minerva_command(root: PathBuf) {
        use ide_minerva::minerva_runtime::tools::{PermissionPolicy, ToolContext};
        let checkout = root.join("checkout");
        let writer = WorkspaceWrite::reserve_in(&root.join("registry"), std::slice::from_ref(&checkout)).unwrap();
        let service = ide_minerva::Service::new(root.join("data"), root.join("config"));
        let workspace = service.workspace(&checkout).await.unwrap();
        let runtime = service.runtime().await.unwrap();
        runtime.create_session("crash-test", Some(&workspace.id)).unwrap();
        let jobs = runtime.plugins.jobs().unwrap();
        let recorder = writer.clone();
        let _journal = jobs.bind_process_journal(
            "crash-test",
            move |name| recorder.record_windows_job(name).map_err(|error| error.to_string()),
            move |name| writer.clear_windows_job_record(name),
        );
        runtime
            .plugins
            .harness
            .tools
            .snapshot()
            .execute(
                "shell",
                serde_json::json!({
                    "command":"Set-Content -LiteralPath 'running' -Value 'yes'; Start-Sleep -Seconds 30",
                    "run_in_background":true,
                }),
                ToolContext {
                    session_id: "crash-test".into(),
                    cwd: Some(checkout.clone()),
                    policy: PermissionPolicy::Allow,
                    delegation_reads: Default::default(),
                    write_approved: false,
                    run_id: "run".into(),
                    call_id: "call".into(),
                    live_policy: None,
                    cancellation: Default::default(),
                    interactions: Default::default(),
                    sink: None,
                },
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !checkout.join("running").is_file() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(jobs.list("crash-test").iter().any(|job| job.status.is_active()));
        std::process::exit(17);
    }

    #[cfg(windows)]
    #[test]
    fn abandoned_owner_record_still_fences_a_live_named_job() {
        const REGISTRY: &str = "OPENIDEA_TEST_CRASHED_WRITER_REGISTRY";
        const JOB: &str = "OPENIDEA_TEST_CRASHED_WRITER_JOB";
        if let Some(registry) = std::env::var_os(REGISTRY) {
            let registry = PathBuf::from(registry);
            let workspace = registry.with_file_name("workspace");
            let writer = WorkspaceWrite::reserve_in(&registry, &[workspace]).unwrap();
            use minerva_plugin_sdk::delegation::NativeProcessJournal;
            let turn = writer.claim_turn().unwrap();
            for name in std::env::var(JOB).unwrap().split(',') {
                turn.record(name).unwrap();
                turn.record(name).unwrap();
                turn.confirm_stopped(name);
            }
            std::process::exit(17);
        }
        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("registry");
        let workspace = dir.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let jobs = [0, 1].map(|_| {
            let name = format!("Global\\OpenIDEA-CheckoutTest-{}", uuid::Uuid::new_v4());
            let mut command = std::process::Command::new("cmd.exe");
            ide_core::process::prepare(&mut command);
            let child = command.args(["/D", "/C", "ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
            let job = ide_core::process::Job::for_pid_named(child.id(), &name).unwrap();
            (name, child, job)
        });
        let owner = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workspace_writes::tests::abandoned_owner_record_still_fences_a_live_named_job"])
            .env(REGISTRY, &registry)
            .env(JOB, format!("{},{}", jobs[0].0, jobs[1].0))
            .output()
            .unwrap();
        assert_eq!(owner.status.code(), Some(17), "{}", String::from_utf8_lossy(&owner.stdout));
        let record =
            fs::read_dir(&registry).unwrap().map(|entry| entry.unwrap().path()).find(|path| path.is_dir()).unwrap();
        assert!(jobs.iter().all(|(name, _, _)| ide_core::process::Job::named_is_running(name).unwrap()));
        assert!(WorkspaceWrite::reserve_in(&registry, std::slice::from_ref(&workspace)).is_err());
        assert!(jobs.iter().all(|(name, _, _)| windows_job_marker(&record, name).is_file()));
        let disjoint = dir.path().join("other");
        fs::create_dir(&disjoint).unwrap();
        assert!(WorkspaceWrite::reserve_in(&registry, &[disjoint]).is_ok());
        for (index, (_, mut child, job)) in jobs.into_iter().enumerate() {
            assert!(job.terminate());
            child.wait().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while job.try_is_running().unwrap() {
                assert!(std::time::Instant::now() < deadline, "the named Job Object remains active");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(job);
            assert_eq!(WorkspaceWrite::reserve_in(&registry, std::slice::from_ref(&workspace)).is_err(), index == 0);
        }
        assert!(!record.exists());
    }

    #[test]
    fn separate_host_observes_checkout_reservation() {
        const CHILD: &str = "OPENIDEA_TEST_CHECKOUT_RESERVATION_CHILD";
        if let Some(registry) = std::env::var_os(CHILD) {
            let workspace = PathBuf::from(&registry).with_file_name("workspace");
            let held = std::env::var_os("OPENIDEA_TEST_CHECKOUT_HELD").as_deref() == Some(std::ffi::OsStr::new("1"));
            assert_eq!(WorkspaceWrite::reserve_in(Path::new(&registry), &[workspace]).is_err(), held);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("registry");
        let workspace = dir.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let writer = WorkspaceWrite::reserve_in(&registry, &[workspace]).unwrap();
        let check = |held: &str| {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "workspace_writes::tests::separate_host_observes_checkout_reservation"])
                .env(CHILD, &registry)
                .env("OPENIDEA_TEST_CHECKOUT_HELD", held)
                .output()
                .unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stdout));
        };
        check("1");
        drop(writer);
        check("0");
    }

    #[test]
    fn retained_reservation_allows_only_one_turn_at_a_time() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let registry = root.path().join("registry");
        let writer = WorkspaceWrite::reserve_in(&registry, std::slice::from_ref(&checkout)).unwrap();
        let retained = writer.clone();
        let turn = writer.claim_turn().unwrap();
        assert!(retained.claim_turn().is_err());
        drop(writer);
        assert!(WorkspaceWrite::reserve_in(&registry, std::slice::from_ref(&checkout)).is_err());
        drop(turn);
        let next = retained.claim_turn().unwrap();
        drop(retained);
        assert!(WorkspaceWrite::reserve_in(&registry, std::slice::from_ref(&checkout)).is_err());
        drop(next);
        assert!(WorkspaceWrite::reserve_in(&registry, &[checkout]).is_ok());
    }

    #[test]
    fn reserves_all_checkouts_and_holds_the_reservation_until_last_owner_drops() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let child = source.join("nested");
        let destination = dir.path().join("destination");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let guard = WorkspaceWrite::reserve(&[source.clone(), destination.clone()]).unwrap();
        let in_flight = guard.clone();
        drop(guard);
        for path in [&child, &destination] {
            assert!(WorkspaceWrite::reserve(std::slice::from_ref(path)).is_err());
        }
        drop(in_flight);
        assert!(WorkspaceWrite::reserve(&[child, destination]).is_ok());
    }

    #[test]
    fn sibling_project_folders_share_the_containing_checkout() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let _guard = WorkspaceWrite::reserve(&[first]).unwrap();
        assert!(WorkspaceWrite::reserve(&[second]).is_err());
    }
}
