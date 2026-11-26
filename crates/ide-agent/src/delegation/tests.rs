use super::*;
mod idle;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an installed, authenticated Codex app-server and calls the model"]
async fn live_codex_fallback_continues_the_original_thread() {
    use ide_minerva::minerva_runtime::tools::{PermissionPolicy, ToolContext};
    use serde_json::json;
    use std::{sync::Arc, time::Duration};

    let dir = tempfile::tempdir().unwrap();
    let manager = std::sync::Arc::new(crate::AgentManager::new(
        crate::AgentPaths {
            project_root: dir.path().into(),
            store_dir: dir.path().join("store"),
            worktrees_dir: dir.path().join("worktrees"),
            models_file: dir.path().join("models.json"),
            custom_agents_dir: None,
            home_dir: None,
            env_secrets: None,
            catalogs: std::sync::Arc::default(),
        },
        tokio::runtime::Handle::current(),
    ));
    let mut missing = manager.shared.spec("codex").unwrap();
    missing.id = "missing-codex".into();
    missing.command = dir.path().join("missing").join("codex").to_string_lossy().into_owned();
    manager.shared.providers.write().push(missing);
    let primary = json!({"kind":"ide-agent","agentId":"missing-codex","options":{}});
    let mut preferences = std::collections::BTreeMap::<String, String>::new();
    if let Ok(model) = std::env::var("MINERVA_TEST_CODEX_MODEL") {
        preferences.insert("model".into(), model);
    }
    if let Ok(effort) = std::env::var("MINERVA_TEST_CODEX_EFFORT") {
        preferences.insert("effort".into(), effort);
    }
    let alternative = json!({"kind":"ide-agent","agentId":"codex","options":preferences});
    let config = dir.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("minerva.conf.json"),
        json!({"version":1,"optionalPluginsEnabled":false,"services":{"delegation":{
            "enabled":true,"fallbacks":[{"primary":primary,"alternative":alternative}]
        }}})
        .to_string(),
    )
    .unwrap();
    let service = ide_minerva::Service::new(dir.path().join("data"), config);
    service.register_project_executor(dir.path(), Arc::new(HostExecutor::new(Arc::downgrade(&manager)))).unwrap();
    let workspace = service.workspace(dir.path()).await.unwrap();
    let runtime = service.runtime().await.unwrap();
    runtime.create_session("parent", Some(&workspace.id)).unwrap();
    let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
    let tools = runtime.plugins.harness.tools.snapshot();
    let context = |call: &str| ToolContext {
        delegation_reads: Default::default(),
        write_approved: false,
        session_id: "parent".into(),
        run_id: "parent_run".into(),
        call_id: call.into(),
        cwd: Some(cwd.clone().into()),
        policy: PermissionPolicy::ReadOnly,
        live_policy: None,
        cancellation: Default::default(),
        interactions: Arc::new(Default::default()),
        sink: None,
    };
    let child: DelegationSnapshot = serde_json::from_value(
        tools
            .execute(
                "subagent_start",
                json!({
                    "operationId":"first","target":primary,
                    "prompt":"Remember the verification word LANTERN. Reply with that word only."
                }),
                context("start"),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&child.request.target).unwrap(), alternative);
    assert_eq!(
        serde_json::to_value(&child.fallback).unwrap(),
        json!({
            "requestedTarget":primary, "reason":CODEX_MISSING,
        })
    );
    let followup = json!({
        "id":child.id,"operationId":"second",
        "prompt":"What verification word did I give you? Reply with that word only."
    });
    let receipt = tools.execute("subagent_send", followup.clone(), context("queue")).await.unwrap();
    assert_eq!(receipt["disposition"], "queued");
    assert!(receipt["activationId"].is_null());

    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let snapshot: DelegationSnapshot = serde_json::from_value(
                tools.execute("subagent_get", json!({"id":child.id}), context("observe")).await.unwrap(),
            )
            .unwrap();
            if matches!(snapshot.state, ActivationState::Failed | ActivationState::Cancelled | ActivationState::Unknown)
            {
                panic!("Unexpected native delegation state: {snapshot:?}");
            }
            if snapshot.activation_id != child.activation_id && snapshot.state == ActivationState::Succeeded {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Codex did not complete the queued follow-up");
    assert_eq!(result.completion.unwrap().summary.trim(), "LANTERN");
    let first = result.previous_handle.unwrap();
    let second = result.handle.unwrap();
    assert_eq!(manager.tasks.read().len(), 1);
    assert_eq!(first.session_id, second.session_id);
    assert_ne!(first.run_id, second.run_id);
    let replay = tools.execute("subagent_send", followup, context("replay")).await.unwrap();
    assert_eq!(replay["workItemId"], receipt["workItemId"]);
    assert_eq!(replay["activationId"], result.activation_id);
    service.shutdown().await.unwrap();
    manager.shutdown();
}

fn manager(dir: &std::path::Path) -> std::sync::Arc<crate::AgentManager> {
    manager_with(dir, std::sync::Arc::new(|_, _, _, _| panic!("Preparation must not start any agent process")))
}

#[tokio::test]
async fn native_discovery_marks_missing_executables_unavailable_without_launching_them() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path());
    manager.shared.providers.write().iter_mut().find(|p| p.id == "codex").unwrap().command =
        dir.path().join("missing").join("codex").to_string_lossy().into_owned();
    manager.shared.providers.write().iter_mut().find(|p| p.id == "claude").unwrap().command =
        dir.path().join("missing").join("claude").to_string_lossy().into_owned();
    let executor = HostExecutor::new(std::sync::Arc::downgrade(&manager));
    let caller = Caller {
        session_id: "parent".into(),
        root_session_id: "parent".into(),
        workspace_id: "workspace".into(),
        cwd: dir.path().to_string_lossy().into_owned(),
    };
    let candidates = executor.discover(&caller).await.unwrap();
    let codex = candidates
        .iter()
        .find(|candidate| matches!(&candidate.target, Target::IdeAgent { agent_id, .. } if agent_id == "codex"))
        .unwrap();
    assert!(codex.read_only);
    assert_eq!(codex.unavailable_reason.as_deref(), Some(CODEX_MISSING));
    let claude = candidates
        .iter()
        .find(|candidate| matches!(&candidate.target, Target::IdeAgent { agent_id, .. } if agent_id == "claude"))
        .unwrap();
    assert!(claude.read_only);
    assert_eq!(claude.unavailable_reason.as_deref(), Some(CLAUDE_MISSING));
}

fn manager_with(dir: &std::path::Path, connector: crate::Connector) -> std::sync::Arc<crate::AgentManager> {
    std::sync::Arc::new(crate::AgentManager::with_connector(
        crate::AgentPaths {
            project_root: dir.into(),
            store_dir: dir.join("store"),
            worktrees_dir: dir.join("worktrees"),
            models_file: dir.join("models.json"),
            custom_agents_dir: Some(dir.join("agents")),
            home_dir: Some(dir.into()),
            env_secrets: None,
            catalogs: std::sync::Arc::default(),
        },
        tokio::runtime::Handle::current(),
        connector,
    ))
}

#[tokio::test]
async fn native_prepare_is_durable_idempotent_and_never_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager(dir.path());
    let (events, mut observed) = tokio::sync::mpsc::unbounded_channel();
    assert!(
        manager
            .subscribe(std::sync::Arc::new(move |event| {
                let _ = events.send(event);
            }))
            .is_empty()
    );
    let executor = HostExecutor::new(std::sync::Arc::downgrade(&manager));
    let snapshot: DelegationSnapshot = serde_json::from_value(serde_json::json!({
        "id":"child","workItemId":"work","activationId":"activation", "depth":1,
        "state":"starting", "handle":null, "completion":null,
        "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":dir.path()},
        "request":{"operationId":"operation", "target":{"kind":"ide-agent","agentId":"codex","options":{}},"prompt":"Inspect only"}
    })).unwrap();
    let admission = executor.prepare_native(&snapshot, false).await.unwrap();
    assert_eq!(admission.phase, AdmissionPhase::Reserved);
    assert_eq!(executor.prepare_native(&snapshot, false).await.unwrap(), admission);
    assert_eq!(manager.tasks.read().len(), 1);
    assert!(dir.path().join("store").join(format!("{}.json", admission.task_id)).exists());
    let task = manager.task(&admission.task_id).unwrap();
    assert!(!task.state.lock().busy);
    assert!(task.live.lock().is_none());
    task.emit_attention(crate::AgentAttentionReason::Finished);
    task.state.lock().conversation.apply(None, &serde_json::json!({"sessionUpdate":"tool_call","toolCallId":"read","title":"Read file","kind":"read","status":"completed"}), 1.0);
    task.push_entry(crate::AgentEntry::Agent { text: "Native full history".into() });
    task.persist();
    task.flush();
    assert!(observed.try_recv().is_err(), "native child leaked onto the root task event stream");
    assert!(manager.subscribe(std::sync::Arc::new(|_| {})).is_empty(), "native child leaked into root task list");
    let history = manager.delegated_history("parent", "parent", "child").unwrap().unwrap();
    assert_eq!(history.len(), 2);
    assert!(matches!(&history[0], crate::AgentEntry::Tool { .. }));
    assert!(manager.delegated_history("foreign-root", "parent", "child").unwrap().is_none());
    assert!(manager.delegated_history("parent", "foreign-parent", "child").unwrap().is_none());
    manager.set_minerva_connections(vec![ide_minerva::Connection {
        id: "minerva".into(),
        name: "Minerva".into(),
        model_prefix: String::new(),
    }]);
    let parent_task = manager
        .create(crate::NewAgentTask {
            provider: "minerva".into(),
            prompt: crate::AgentPrompt { text: "Parent".into(), ..Default::default() },
            idle: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let parent = manager.task(&parent_task.id).unwrap();
    parent.state.lock().session_id = Some("parent".into());
    parent.state.lock().conversation.apply(
        None,
        &serde_json::json!({"sessionUpdate":"subagent_update","sessionId":"del_child","state":{"state":"idle"}}),
        1.0,
    );
    assert_eq!(manager.entries(&parent_task.id, Some("del_child".into())).unwrap().len(), 2);
    assert!(task.state.lock().delegated_read_only);
    assert_eq!(task.ask_permission(&serde_json::json!({})), serde_json::json!({"outcome":{"outcome":"cancelled"}}));
    assert!(task.state.lock().wanted.iter().any(|c| c.id == "mode" && c.value == "read-only"));
    assert!(
        manager
            .prompt(
                &admission.task_id,
                crate::AgentPrompt { text: "Run from the composer".into(), ..Default::default() },
                false
            )
            .await
            .is_err()
    );
    assert_eq!(executor.prepare_native(&snapshot, false).await.unwrap().phase, AdmissionPhase::Reserved);
    assert_eq!(executor.observe(&snapshot).await.unwrap(), None);
    let store = AdmissionStore::open(&manager.shared.paths.store_dir).unwrap();
    let result =
        Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "Completed".into() };
    assert_eq!(
        store.complete_turn(&admission.operation_id, "native_turn", result.clone()).unwrap_err().code,
        ErrorCode::Conflict
    );
    store.claim_dispatch(&admission.operation_id).unwrap();
    store.complete_turn(&admission.operation_id, "native_turn", result.clone()).unwrap();
    drop(store);
    assert_eq!(executor.observe(&snapshot).await.unwrap(), Some(result));
    let mut unrelated = snapshot.clone();
    unrelated.activation_id = "unrelated".into();
    assert_eq!(executor.observe(&unrelated).await.unwrap_err().code, ErrorCode::Conflict);
    let mut altered = snapshot.clone();
    altered.request.prompt = "Different work".into();
    assert_eq!(executor.observe(&altered).await.unwrap_err().code, ErrorCode::Conflict);
    manager.shutdown();
}

fn intent() -> Admission {
    Admission {
        requested_config: Default::default(),
        parent_session_id: "parent".into(),
        root_session_id: "parent".into(),
        delegation_id: "child".into(),
        operation_id: "operation".into(),
        payload_hash: "fingerprint".into(),
        task_id: "task".into(),
        activation_id: "activation".into(),
        request_id: "request".into(),
        codex_scoped_tools: false,
        host_turn_id: None,
        native_session_id: None,
        phase: AdmissionPhase::Reserved,
        completion: None,
    }
}

#[test]
fn cancellation_before_dispatch_survives_reopen_and_prevents_launch() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdmissionStore::open(dir.path()).unwrap();
    store.reserve(intent()).unwrap();
    let cancelled = store.cancel_reserved("operation").unwrap();
    assert_eq!(cancelled.completion.as_ref().unwrap().state, ActivationState::Cancelled);
    drop(store);
    let store = AdmissionStore::open(dir.path()).unwrap();
    assert_eq!(store.lookup("operation").unwrap(), cancelled);
    assert!(store.claim_dispatch("operation").is_err());
    assert!(store.complete_turn("operation", "orphan", cancelled.completion.unwrap()).is_err());
}

#[test]
fn claimed_cancellation_before_prompt_is_terminal_without_a_native_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdmissionStore::open(dir.path()).unwrap();
    store.reserve(intent()).unwrap();
    store.claim_dispatch("operation").unwrap();
    store.bind_native_session("operation", "thread").unwrap();
    let cancelled = store.cancel_before_prompt("operation").unwrap();
    assert_eq!(cancelled.completion.as_ref().unwrap().state, ActivationState::Cancelled);
    assert!(cancelled.host_turn_id.is_none());
    assert!(store.claim_dispatch("operation").is_err());
    assert!(store.complete_turn("operation", "late", cancelled.completion.unwrap()).is_err());
}

#[test]
fn a_claimed_native_attempt_is_never_dispatched_again_and_exact_result_survives() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdmissionStore::open(dir.path()).unwrap();
    store.reserve(intent()).unwrap();
    store.claim_dispatch("operation").unwrap();
    store.bind_native_session("operation", "thread-a").unwrap();
    assert!(store.bind_native_session("operation", "thread-b").is_err());
    assert_eq!(store.claim_dispatch("operation").unwrap_err().code, ErrorCode::OutcomeUnknown);
    let completion =
        Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "done".into() };
    store.complete_turn("operation", "turn-a", completion.clone()).unwrap();
    assert_eq!(store.for_execution("task", "request").unwrap().completion, Some(completion.clone()));
    assert!(store.complete_turn("operation", "turn-b", completion.clone()).is_err());
    drop(store);
    let store = AdmissionStore::open(dir.path()).unwrap();
    assert_eq!(store.for_execution("task", "request").unwrap().completion, Some(completion));
    assert_eq!(store.lookup("operation").unwrap().native_session_id.as_deref(), Some("thread-a"));
    assert!(store.claim_dispatch("operation").is_err());
}

#[test]
fn admission_survives_reopen_and_duplicate_does_not_reset_correlation() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = AdmissionStore::open(dir.path()).unwrap();
        store.reserve(intent()).unwrap();
        assert!(store.bind_native_turn("operation", "thread-a", "turn-a").is_err());
        store.claim_dispatch("operation").unwrap();
        store.bind_native_session("operation", "thread-a").unwrap();
        store.bind_native_turn("operation", "thread-a", "turn-a").unwrap();
        assert_eq!(store.reserve(intent()).unwrap().host_turn_id.as_deref(), Some("turn-a"));
        assert!(AdmissionStore::open(dir.path()).is_err());
    }
    let store = AdmissionStore::open(dir.path()).unwrap();
    assert_eq!(store.lookup("operation").unwrap().phase, AdmissionPhase::Unknown);
    assert!(store.claim_dispatch("operation").is_err());
    let completion =
        Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "done".into() };
    assert!(store.complete_turn("operation", "turn-b", completion.clone()).is_err());
    store.complete_turn("operation", "turn-a", completion).unwrap();
    assert_eq!(store.reserve(intent()).unwrap().phase, AdmissionPhase::Settled);
}

#[test]
fn payload_conflict_and_stale_turn_do_not_mutate_saved_intent() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdmissionStore::open(dir.path()).unwrap();
    store.reserve(intent()).unwrap();
    let mut conflict = intent();
    conflict.payload_hash = "different".into();
    assert!(store.reserve(conflict).is_err());
    store.claim_dispatch("operation").unwrap();
    store.bind_native_session("operation", "thread-a").unwrap();
    assert!(store.bind_native_turn("operation", "thread-b", "turn-a").is_err());
    store.bind_native_turn("operation", "thread-a", "turn-a").unwrap();
    assert!(store.bind_native_turn("operation", "thread-a", "turn-b").is_err());
    assert_eq!(store.lookup("operation").unwrap().host_turn_id.as_deref(), Some("turn-a"));
}

#[test]
fn failed_persistence_does_not_ack_or_change_in_memory_intent() {
    let dir = tempfile::tempdir().unwrap();
    let store = AdmissionStore::open(dir.path()).unwrap();
    std::fs::create_dir(dir.path().join("delegation/admissions.json")).unwrap();
    assert!(store.reserve(intent()).is_err());
    assert!(store.lookup("operation").is_none());
}

#[test]
fn corrupt_existing_ledger_is_not_replaced_with_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("delegation")).unwrap();
    let path = dir.path().join("delegation/admissions.json");
    std::fs::write(&path, b"broken").unwrap();
    assert!(AdmissionStore::open(dir.path()).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"broken");
}

#[tokio::test]
async fn reopened_host_keeps_a_dispatched_native_attempt_unknown_without_relaunching() {
    let dir = tempfile::tempdir().unwrap();
    let manager = manager_with(dir.path(), std::sync::Arc::new(|_, _, _, _| panic!("must not relaunch")));
    let snapshot: DelegationSnapshot = serde_json::from_value(serde_json::json!({
        "id":"child","workItemId":"work","activationId":"activation","depth":1,"state":"running",
        "handle":{"sessionId":"task","runId":"request"},
        "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":dir.path()},
        "request":{"operationId":"operation","target":{"kind":"ide-agent","agentId":"codex","options":{}},"prompt":"Inspect only"}
    })).unwrap();
    let (operation, payload_hash) = super::attempt_identity(&snapshot).unwrap();
    {
        let store = AdmissionStore::open(&manager.shared.paths.store_dir).unwrap();
        store.reserve(Admission { operation_id: operation.clone(), payload_hash, ..intent() }).unwrap();
        store.claim_dispatch(&operation).unwrap();
        store.bind_native_session(&operation, "thread").unwrap();
        store.bind_native_turn(&operation, "thread", "native-turn").unwrap();
    }
    let executor = HostExecutor::new(std::sync::Arc::downgrade(&manager));
    assert_eq!(executor.observe(&snapshot).await.unwrap_err().code, ErrorCode::OutcomeUnknown);
    let store = AdmissionStore::open(&manager.shared.paths.store_dir).unwrap();
    let saved = store.lookup(&operation).unwrap();
    assert_eq!(saved.phase, AdmissionPhase::Unknown);
    assert_eq!(saved.host_turn_id.as_deref(), Some("native-turn"));
    assert!(saved.completion.is_none());
    assert_eq!(store.claim_dispatch(&operation).unwrap_err().code, ErrorCode::OutcomeUnknown);
    manager.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_host_dispatch_observes_a_correlated_result_and_replays_its_handle() {
    for agent in ["codex", "claude"] {
        native_host_dispatch_case(agent).await;
    }
}

async fn native_host_dispatch_case(agent: &str) {
    use ide_jsonrpc::{Framing, Handler, Peer};
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Fixture(AtomicUsize, bool);
    impl Handler for Fixture {
        fn on_notification(&self, _: &str, _: Value) {}

        fn on_request(&self, method: &str, params: Value) -> Result<Value, (i64, String)> {
            match method {
                "initialize" => Ok(json!({"protocolVersion":1,"agentCapabilities":{}})),
                "session/new" => {
                    assert_eq!(params.pointer("/_meta/openidea/delegatedReadOnly"), Some(&json!(true)));
                    Ok(json!({"sessionId":"native-session"}))
                }
                "session/prompt" => {
                    assert_eq!(params.pointer("/_meta/openidea/delegatedReadOnly"), Some(&json!(true)));
                    let index = self.0.fetch_add(1, Ordering::SeqCst);
                    let turn = if self.1 {
                        params.pointer("/_meta/openidea/nativeTurnId").and_then(Value::as_str).unwrap().to_owned()
                    } else {
                        format!("turn-fixture-{index}")
                    };
                    Ok(
                        json!({"stopReason":"end_turn","_meta":{"openidea":{"nativeTurnId":turn,"nativeThreadId":"native-session"}}}),
                    )
                }
                _ => Ok(json!({})),
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git").args(["init", "--quiet"]).current_dir(dir.path()).status().unwrap().success()
    );
    let fixture = Arc::new(Fixture(AtomicUsize::new(0), agent == "claude"));
    let server_fixture = fixture.clone();
    let peers = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let server_peers = peers.clone();
    let manager = manager_with(
        dir.path(),
        Arc::new(move |_, _, handler, _| {
            let (a, b) = tokio::io::duplex(1 << 16);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            server_peers.lock().push(Peer::connect_framed(br, bw, server_fixture.clone(), Framing::Lines));
            Ok(Peer::connect_framed(ar, aw, handler, Framing::Lines))
        }),
    );
    let executor = HostExecutor::new(Arc::downgrade(&manager));
    let mut snapshot: DelegationSnapshot = serde_json::from_value(json!({
        "id":"child","workItemId":"work","activationId":"activation","depth":1,
        "state":"starting",
        "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":dir.path()},
        "request":{"operationId":"operation","target":{"kind":"ide-agent","agentId":agent,"options":{}},"prompt":"Inspect only"}
    })).unwrap();
    let handle = executor.start(&snapshot).await.unwrap();
    snapshot.handle = Some(handle.clone());
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match executor.observe(&snapshot).await {
                Ok(Some(outcome)) => break outcome,
                Ok(None) => {}
                Err(error) if error.code == ErrorCode::Busy => {}
                Err(error) => panic!("Native attempt failed: {error:?}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(outcome.state, ActivationState::Succeeded);
    if agent == "codex" {
        assert_eq!(executor.check_scoped_followup(&snapshot).await.unwrap_err().code, ErrorCode::Unsupported);
    } else {
        executor.check_scoped_followup(&snapshot).await.unwrap();
    }
    assert_eq!(executor.start(&snapshot).await.unwrap(), handle);
    assert_eq!(fixture.0.load(Ordering::SeqCst), 1);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while manager.task(&handle.session_id).unwrap().state.lock().busy {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(manager.task(&handle.session_id).unwrap().state.lock().turn.is_none());
    snapshot.previous_handle = Some(handle.clone());
    snapshot.handle = None;
    snapshot.activation_id = "followup-activation".into();
    snapshot.request.operation_id = "followup".into();
    snapshot.request.prompt = "Continue the review".into();
    let continued = executor.start(&snapshot).await.unwrap();
    assert_eq!(continued.session_id, handle.session_id);
    assert_ne!(continued.run_id, handle.run_id);
    snapshot.handle = Some(continued.clone());
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match executor.observe(&snapshot).await {
                Ok(Some(result)) => break result,
                Ok(None) => {}
                Err(error) if error.code == ErrorCode::Busy => {}
                Err(error) => panic!("Native follow-up failed: {error:?}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result.result_id, "followup-activation_result");
    assert_eq!(executor.start(&snapshot).await.unwrap(), continued);
    assert_eq!(fixture.0.load(Ordering::SeqCst), 2);
    manager.shutdown();
}
#[test]
fn workspace_identity_is_the_canonical_project_root() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let child = project.join("child");
    let sibling = temp.path().join("project-other");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    let resolve =
        |path: &std::path::Path| super::project_workspace(path.to_str().unwrap(), &project, ErrorCode::Unavailable);
    assert_eq!(resolve(&child.join("..")).unwrap(), dunce::canonicalize(&project).unwrap());
    assert!(resolve(&child).is_err());
    assert!(resolve(&sibling).is_err());
    assert!(resolve(&project.join("missing")).is_err());
}
