use super::*;
use minerva_plugin_sdk::tools::PermissionPolicy;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the installed and authenticated Codex app-server"]
async fn live_codex_scoped_tools_write_through_openidea() {
    let dir = tempfile::tempdir().unwrap();
    live_codex_scoped_case(dir.path(), Approval::NotNeeded).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the installed and authenticated Codex app-server"]
async fn live_codex_scoped_tools_wait_for_parent_approval() {
    let dir = tempfile::tempdir().unwrap();
    live_codex_scoped_case(dir.path(), Approval::Answer(true)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the installed and authenticated Codex app-server"]
async fn live_codex_scoped_tools_respect_parent_denial() {
    let dir = tempfile::tempdir().unwrap();
    live_codex_scoped_case(dir.path(), Approval::Answer(false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the installed and authenticated Codex app-server"]
async fn live_codex_scoped_host_exit_fences_the_interrupted_turn() {
    const ROOT_ENV: &str = "OPENIDEA_TEST_CODEX_CRASH_ROOT";
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        live_codex_scoped_case(std::path::Path::new(&root), Approval::ExitHost).await;
        unreachable!();
    }
    let dir = tempfile::tempdir().unwrap();
    let host = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "delegation::live_codex_tests::live_codex_scoped_host_exit_fences_the_interrupted_turn",
        ])
        .env(ROOT_ENV, dir.path())
        .output()
        .unwrap();
    assert_eq!(host.status.code(), Some(17), "{}", String::from_utf8_lossy(&host.stdout));
    let checkout = dir.path().join("checkout");
    assert!(!checkout.join("fixture.txt").exists());
    let store = AdmissionStore::open(&dir.path().join("store")).unwrap();
    let attempt = store.for_delegation("parent", "parent", "fixture").unwrap();
    assert_eq!(attempt.phase, AdmissionPhase::Unknown);
    assert!(attempt.host_turn_id.is_some());
    assert!(attempt.native_session_id.is_some());
    assert!(attempt.completion.is_none());
    assert_eq!(store.claim_dispatch(&attempt.operation_id).unwrap_err().code, ErrorCode::OutcomeUnknown);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if crate::workspace_writes::WorkspaceWrite::acquire(vec![checkout.clone()]).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("checkout should be released after the Codex Job ends");
}

#[derive(Clone, Copy)]
enum Approval {
    NotNeeded,
    Answer(bool),
    ExitHost,
}

async fn live_codex_scoped_case(dir: &std::path::Path, approval: Approval) {
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

    let policy = if matches!(approval, Approval::NotNeeded) { PermissionPolicy::Allow } else { PermissionPolicy::Ask };
    let root = dir.join("checkout");
    std::fs::create_dir(&root).unwrap();
    let root = dunce::canonicalize(root).unwrap();
    let service = Arc::new(ide_minerva::Service::new(dir.join("data"), dir.join("config")));
    let manager = Arc::new(crate::AgentManager::with_minerva(
        crate::AgentPaths {
            project_root: root.clone(),
            store_dir: dir.join("store"),
            worktrees_dir: dir.join("worktrees"),
            models_file: dir.join("models.json"),
            custom_agents_dir: None,
            home_dir: None,
            env_secrets: None,
            catalogs: Arc::default(),
        },
        tokio::runtime::Handle::current(),
        service.clone(),
    ));
    let model = std::env::var("MINERVA_TEST_CODEX_MODEL").ok();
    let options = model.into_iter().map(|model| ("model", model)).collect::<std::collections::BTreeMap<_, _>>();
    let mut snapshot: DelegationSnapshot = serde_json::from_value(json!({
        "id":"fixture","workItemId":"work","activationId":"codex-scoped-write","depth":1,"state":"starting",
        "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":root},
        "request":{"operationId":"write-fixture","originRun":"parent-run","target":{"kind":"ide-agent","agentId":"codex","options":options},
            "prompt":"Call the openidea_write_file tool to create fixture.txt with the exact contents written by OpenIDEA. Use the named tool directly. Do not launch any shell or invoke apply_patch. Call it once. If the tool returns a denial, do not retry. Once the tool returns, report the outcome."}
    })).unwrap();
    snapshot.request.tool_registrations =
        service.runtime().await.unwrap().plugins.harness.tools.snapshot().delegation_writes();
    let writer = crate::workspace_writes::WorkspaceWrite::acquire(vec![root.clone()]).await.unwrap();
    let owner = Arc::new(writer.claim_turn().unwrap());
    let checkout = NativeCheckout::new(&snapshot, policy, move || policy).with_process_journal(Some(owner));
    let executor = HostExecutor::new(Arc::downgrade(&manager));
    let handle = executor.start_with_checkout(&snapshot, checkout).await.unwrap();
    snapshot.handle = Some(handle.clone());
    if !matches!(approval, Approval::NotNeeded) {
        let runtime = service.runtime().await.unwrap();
        let broker = &runtime.plugins.harness.interactions;
        let pending = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if let Some(pending) = broker.list("parent").into_iter().next() {
                    break pending;
                }
                if let Err(error) = executor.observe(&snapshot).await
                    && error.code != ErrorCode::Busy
                {
                    panic!("Codex ended before requesting approval: {error:?}");
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(pending.run_id, "parent-run");
        assert!(pending.call_id.starts_with("delegation:codex-scoped-write:"));
        assert!(pending.prompt.contains("fixture.txt"));
        assert!(!root.join("fixture.txt").exists());
        let allow = match approval {
            Approval::Answer(allow) => allow,
            Approval::ExitHost => std::process::exit(17),
            Approval::NotNeeded => unreachable!(),
        };
        assert!(broker.answer(minerva_plugin_sdk::tools::InteractionAnswer {
            session_id: pending.session_id,
            id: pending.id,
            decision: if allow { "allow" } else { "deny" }.into(),
            answer: None,
        }));
    }
    let result = wait_for_completion(&executor, &snapshot, &manager).await;
    assert_eq!(result.state, ActivationState::Succeeded, "{result:?}");
    let file = root.join("fixture.txt");
    if matches!(approval, Approval::Answer(false)) {
        assert!(!file.exists(), "Denied write changed the checkout");
    } else {
        assert_eq!(
            std::fs::read_to_string(&file)
                .unwrap_or_else(|error| { panic!("Model did not write the fixture: {result:?}, {error}") }),
            "written by OpenIDEA"
        );
    }
    drop(writer);
    assert!(crate::workspace_writes::WorkspaceWrite::acquire(vec![root.clone()]).await.is_ok());
    if matches!(approval, Approval::NotNeeded) {
        snapshot.previous_handle = Some(handle.clone());
        snapshot.handle = None;
        snapshot.activation_id = "codex-read-only-followup".into();
        snapshot.request.operation_id = "read-only-followup".into();
        snapshot.request.tool_registrations.clear();
        snapshot.request.prompt = "Read fixture.txt in the working directory and report its contents in your response. Do not modify anything.".into();
        let continued = executor.start(&snapshot).await.unwrap();
        assert_eq!(continued.session_id, handle.session_id);
        assert_ne!(continued.run_id, handle.run_id);
        snapshot.handle = Some(continued);
        let result = wait_for_completion(&executor, &snapshot, &manager).await;
        assert_eq!(result.state, ActivationState::Succeeded, "{result:?}");
        assert!(result.summary.contains("written by OpenIDEA"), "{result:?}");
        assert_eq!(std::fs::read_to_string(file).unwrap(), "written by OpenIDEA");

        snapshot.previous_handle = snapshot.handle.take();
        snapshot.activation_id = "codex-write-followup".into();
        snapshot.request.operation_id = "write-followup".into();
        snapshot.request.tool_registrations =
            service.runtime().await.unwrap().plugins.harness.tools.snapshot().delegation_writes();
        snapshot.request.prompt = "Call openidea_write_file once to create second.txt with the exact contents same thread. Use only that tool for writing.".into();
        let writer = crate::workspace_writes::WorkspaceWrite::acquire(vec![root.clone()]).await.unwrap();
        let checkout = NativeCheckout::new(&snapshot, PermissionPolicy::Allow, || PermissionPolicy::Allow)
            .with_process_journal(Some(Arc::new(writer.claim_turn().unwrap())));
        let continued = executor.start_with_checkout(&snapshot, checkout).await.unwrap();
        assert_eq!(continued.session_id, handle.session_id);
        assert_ne!(continued.run_id, snapshot.previous_handle.as_ref().unwrap().run_id);
        snapshot.handle = Some(continued);
        let result = wait_for_completion(&executor, &snapshot, &manager).await;
        assert_eq!(result.state, ActivationState::Succeeded, "{result:?}");
        assert_eq!(std::fs::read_to_string(root.join("second.txt")).unwrap(), "same thread", "{result:?}");
    }
    manager.shutdown();
    service.shutdown().await.unwrap();
}

async fn wait_for_completion(
    executor: &HostExecutor,
    snapshot: &DelegationSnapshot,
    manager: &crate::AgentManager,
) -> Completion {
    use std::time::Duration;
    let handle = snapshot.handle.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(75), async {
        loop {
            match executor.observe(snapshot).await {
                Ok(Some(result)) => break result,
                Ok(None) => {}
                Err(error) if error.code == ErrorCode::Busy => {}
                Err(error) => {
                    let task = manager.task(&handle.session_id).unwrap();
                    panic!("{error:?}, task error: {:?}", task.state.lock().task.error);
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap()
}
