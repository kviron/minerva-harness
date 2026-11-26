use crate::{Runtime, RuntimeConfig};
use minerva_plugin_sdk::delegation::{
    ActivationState, Caller, Candidate, Capability, Completion, DelegationError, DelegationSnapshot, ErrorCode,
    ExecutionCapabilities, ExecutionFuture, ExecutionHandle, Target,
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct FailHandleWrite(Arc<dyn minerva_delegation::repository::RepositoryPort>);
impl minerva_delegation::repository::RepositoryPort for FailHandleWrite {
    fn request_stop_tree(
        &self,
        c: &minerva_delegation::Caller,
    ) -> minerva_delegation::DelegationResult<Vec<DelegationSnapshot>> {
        self.0.request_stop_tree(c)
    }
    fn lookup_operation(
        &self,
        c: &minerva_delegation::Caller,
        r: &minerva_delegation::StartRequest,
    ) -> minerva_delegation::DelegationResult<Option<DelegationSnapshot>> {
        self.0.lookup_operation(c, r)
    }
    fn admit(
        &self,
        c: &minerva_delegation::Caller,
        r: &minerva_delegation::StartRequest,
        l: &minerva_delegation::limits::Limits,
    ) -> minerva_delegation::DelegationResult<minerva_delegation::repository::Admission> {
        self.0.admit(c, r, l)
    }
    fn admit_followup(
        &self,
        c: &minerva_delegation::Caller,
        id: &str,
        r: &minerva_delegation::StartRequest,
        l: &minerva_delegation::limits::Limits,
    ) -> minerva_delegation::DelegationResult<minerva_delegation::repository::Admission> {
        self.0.admit_followup(c, id, r, l)
    }
    fn lookup_followup(
        &self,
        c: &minerva_delegation::Caller,
        id: &str,
        op: &str,
        prompt: &str,
    ) -> minerva_delegation::DelegationResult<Option<DelegationSnapshot>> {
        self.0.lookup_followup(c, id, op, prompt)
    }
    fn get(
        &self,
        c: &minerva_delegation::Caller,
        id: &str,
    ) -> minerva_delegation::DelegationResult<DelegationSnapshot> {
        self.0.get(c, id)
    }
    fn list(&self, c: &minerva_delegation::Caller) -> minerva_delegation::DelegationResult<Vec<DelegationSnapshot>> {
        self.0.list(c)
    }
    fn started(
        &self,
        _: &str,
        _: &str,
        _: &minerva_delegation::ExecutionHandle,
    ) -> minerva_delegation::DelegationResult<()> {
        Err(minerva_delegation::DelegationError::new(
            minerva_delegation::ErrorCode::StorageFailed,
            "fixture handle commit failure",
        ))
    }
    fn settle(
        &self,
        id: &str,
        a: &str,
        c: &minerva_delegation::Completion,
    ) -> minerva_delegation::DelegationResult<()> {
        self.0.settle(id, a, c)
    }
    fn mark_unknown(&self, id: &str, a: &str, r: &str) -> minerva_delegation::DelegationResult<()> {
        self.0.mark_unknown(id, a, r)
    }
    fn request_stop(
        &self,
        c: &minerva_delegation::Caller,
        id: &str,
    ) -> minerva_delegation::DelegationResult<DelegationSnapshot> {
        self.0.request_stop(c, id)
    }
}

fn config(root: &Path, enabled: bool) -> RuntimeConfig {
    let config_dir = root.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("minerva.conf.json"),
        json!({
            "version":1,"optionalPluginsEnabled":false,"services":{"delegation":{"enabled":enabled}}
        })
        .to_string(),
    )
    .unwrap();
    RuntimeConfig {
        tenant_id: "test".into(),
        workspace_id: "test".into(),
        profile: "local".into(),
        data_dir: root.join("data"),
        config_dir,
    }
}

fn context(session: &str, cwd: &str, call: &str) -> crate::tools::ToolContext {
    crate::tools::ToolContext {
        delegation_reads: Default::default(),
        write_approved: false,
        session_id: session.into(),
        run_id: "parent_run".into(),
        call_id: call.into(),
        cwd: Some(cwd.into()),
        policy: crate::tools::PermissionPolicy::ReadOnly,
        live_policy: None,
        cancellation: Default::default(),
        interactions: Arc::new(Default::default()),
        sink: None,
    }
}

async fn request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0 && bytes.len() < 2_000_000, "incomplete fixture request");
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let length: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}

async fn model() -> (String, tokio::sync::mpsc::UnboundedReceiver<Value>, tokio::task::JoinHandle<()>) {
    model_responses(|_| (json!({"role":"assistant","content":"Hello from child"}), "stop")).await
}

async fn model_responses(
    reply: impl Fn(usize) -> (Value, &'static str) + Send + 'static,
) -> (String, tokio::sync::mpsc::UnboundedReceiver<Value>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (observed, bodies) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut turn = 0;
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let body = request(&mut socket).await;
            if observed.send(body).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            let (delta, finish) = reply(turn);
            turn += 1;
            let chunks = [delta, json!({})];
            let mut response = String::new();
            for (index, delta) in chunks.into_iter().enumerate() {
                response.push_str(&format!("data: {}\n\n",json!({"id":"fixture","object":"chat.completion.chunk","created":1,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":if index==1 {json!(finish)} else {Value::Null}}]})));
            }
            response.push_str("data: [DONE]\n\n");
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
        }
    });
    (format!("http://{address}/v1"), bodies, task)
}

#[tokio::test]
async fn default_composition_does_not_register_subagent_tools() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::start_without_packages(config(dir.path(), false)).await.unwrap();
    assert!(!runtime.plugins.harness.tools.snapshot().definitions().iter().any(|t| t.name.starts_with("subagent_")));
    assert!(runtime.delegation.lock().await.is_some(), "management remains available when admission is disabled");
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabled_restart_preserves_management_and_publishes_results_without_waking_parent() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), false);
    let runtime = Runtime::start_without_packages(config.clone()).await.unwrap();
    let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
    workspaces.create_session("parent", "default").unwrap();
    let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
    let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
    runtime
        .sessions
        .accept_user_run(
            &crate::StartRunRequest {
                session_id: "parent".into(),
                request_id: "parent_run".into(),
                parent_id: None,
                message: crate::RunMessage { id: "parent_input".into(), text: "Begin".into() },
                model: "api:fixture/fixture".into(),
                options: json!({}),
            },
            "time",
        )
        .unwrap();
    runtime
        .sessions
        .commit_reply(
            "parent",
            &crate::sessions::SessionMessage {
                id: "parent_run".into(),
                parent_id: Some("parent_input".into()),
                role: "assistant".into(),
                created_at: "time".into(),
                parts: vec![crate::sessions::ContentPart::text("text", "Parent is idle")],
                extensions: Default::default(),
            },
            None,
        )
        .unwrap();
    drop(workspaces);
    runtime.shutdown().await.unwrap();
    let child = {
        let sessions = crate::sessions::SessionService::open(&config.data_dir.join("sessions.sqlite3")).unwrap();
        let repository = sessions.delegation_repository();
        let request = minerva_delegation::StartRequest {
            operation_id: "accepted-before-disable".into(),
            prompt: "Read the fixture".into(),
            origin_run: Some("parent_run".into()),
            tool_registrations: Default::default(),
            target: Target::Minerva { route: "api:fixture/fixture".into(), effort: None },
        };
        let child = repository.admit(&caller, &request, &Default::default()).unwrap().snapshot;
        let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
        repository.started(&child.id, &child.activation_id, &handle).unwrap();
        repository
            .settle(
                &child.id,
                &child.activation_id,
                &Completion {
                    result_id: "completed-before-disable".into(),
                    state: ActivationState::Succeeded,
                    summary: "Recovered result".into(),
                },
            )
            .unwrap();
        child
    };
    let runtime = Runtime::start_without_packages(config).await.unwrap();
    let tools = runtime.plugins.harness.tools.snapshot();
    let definitions = tools.definitions();
    for name in ["subagent_start", "subagent_send", "subagent_discover"] {
        assert!(!definitions.iter().any(|tool| tool.name == name));
    }
    for name in ["subagent_get", "subagent_list", "subagent_wait", "subagent_interrupt", "subagent_answer"] {
        assert!(definitions.iter().any(|tool| tool.name == name), "missing {name}");
    }
    let snapshot = tools.execute("subagent_get", json!({"id":child.id}), context("parent", &cwd, "get")).await.unwrap();
    assert_eq!(snapshot["completion"]["summary"], "Recovered result");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let events = runtime.sessions.events_after("parent", 0, 100).unwrap();
            assert!(!events.iter().any(|event| event.kind == "run.updated"
                && event.data["runId"].as_str().is_some_and(|id| id.starts_with("wake_"))));
            if events
                .iter()
                .any(|event| event.kind == "delegation.result" && event.data["resultId"] == "completed-before-disable")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("disabled runtime must publish the pending result");
    assert_eq!(runtime.sessions.delegation_unbound_results().unwrap().len(), 1);
    assert!(runtime.get_run("parent", &format!("wake_{}", child.activation_id)).is_err());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_duplicate_registration_keeps_the_original_tools() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::start_without_packages(config(dir.path(), true)).await.unwrap();
    let service = runtime.delegation.lock().await.as_ref().unwrap().service.clone();
    let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
    let resolver = Arc::new(super::Resolver {
        sessions: runtime.sessions.clone(),
        plugins: Arc::downgrade(&runtime.plugins),
        workspaces,
    });
    let registry = runtime.plugins.harness.tools.clone();
    assert!(minerva_tool_subagent::register(registry.clone(), service, resolver).is_err());
    let definitions = registry.snapshot().definitions();
    let start = definitions.iter().find(|tool| tool.name == "subagent_start").unwrap();
    let native = &start.input_schema["properties"]["target"]["oneOf"][1]["properties"]["options"];
    assert_eq!(native["additionalProperties"], false);
    let names =
        native["properties"].as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names, ["effort", "model"].into_iter().collect());
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_handle_commit_does_not_prevent_stopping_the_exact_child_run() {
    stop_child_after_failed_handle_commit(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tree_stop_interrupts_the_exact_child_run_even_without_a_persisted_handle() {
    stop_child_after_failed_handle_commit(true).await;
}

async fn stop_child_after_failed_handle_commit(tree: bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_without_packages(config(dir.path(), false)).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (reached, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let body = request(&mut socket).await;
            reached.send(body).unwrap();
            // Keep a real provider call active until cancellation drops its client.
            std::future::pending::<()>().await;
        });
        runtime
            .plugins
            .providers()
            .unwrap()
            .save(
                serde_json::from_value(json!({
                    "id":"fixture","name":"Fixture","baseUrl":url,"models":["fixture"],
                    "apiKey":null,"isNew":true,"protocol":"ollama"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
        workspaces.create_session("parent", "default").unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
        let executor = Arc::new(super::executor::MinervaExecutor {
            sessions: runtime.sessions.clone(),
            agent_loop: runtime.agent_loop.clone(),
            plugins: Arc::downgrade(&runtime.plugins),
            workspaces,
        });
        let service = minerva_delegation::DelegationService::new(
            Arc::new(FailHandleWrite(runtime.sessions.delegation_repository())),
            executor,
            minerva_delegation::limits::Limits::default(),
        );
        let accepted = service
            .start(
                &caller,
                &minerva_delegation::StartRequest {
                    origin_run: None,
                    tool_registrations: Default::default(),
                    operation_id: "lost_handle".into(),
                    target: minerva_delegation::Target::Minerva { route: "api:fixture/fixture".into(), effort: None },
                    prompt: "Wait for a reply".into(),
                },
            )
            .await
            .unwrap();
        service.drain_starts().await;
        assert_eq!(received.await.unwrap()["model"], "fixture");
        let snapshot = service.get(&caller, &accepted.id).unwrap();
        assert_eq!(snapshot.state, ActivationState::Unknown);
        assert!(snapshot.handle.is_none());
        if tree {
            let requested = service.interrupt_tree(&caller).unwrap();
            assert_eq!(requested[0].activation_id, accepted.activation_id);
            assert_eq!(requested[0].state, ActivationState::Stopping);
        } else {
            service.interrupt(&caller, &accepted.id).await.unwrap();
        }
        let stopped = service.wait(&caller, &accepted.id, Duration::from_secs(5)).await.unwrap();
        assert_eq!(stopped.state, ActivationState::Cancelled);
        let run = runtime.agent_loop.get_run(&format!("del_{}", accepted.id), &accepted.activation_id).unwrap();
        assert_eq!(run.status, "cancelled");
        server.abort();
        let _ = server.await;
        runtime.shutdown().await.unwrap();
    })
    .await
    .expect("lost handle cancellation fixture exceeded deadline");
}

#[tokio::test]
async fn restart_dispatches_a_promoted_queue_attempt_with_no_admitted_run() {
    restart_promoted_queue_attempt(false).await;
}

#[tokio::test]
async fn restart_does_not_dispatch_a_queue_attempt_with_an_admitted_run() {
    restart_promoted_queue_attempt(true).await;
}

async fn restart_promoted_queue_attempt(admitted: bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_without_packages(config(dir.path(), false)).await.unwrap();
        let (url, mut model_requests, server) = model().await;
        runtime
            .plugins
            .providers()
            .unwrap()
            .save(
                serde_json::from_value(json!({
                    "id":"fixture","name":"Fixture","baseUrl":url,"models":["fixture"],
                    "apiKey":null,"isNew":true,"protocol":"ollama"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
        workspaces.create_session("parent", "default").unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
        let repository = runtime.sessions.delegation_repository();
        let limits = minerva_delegation::limits::Limits::default();
        let mut request = minerva_delegation::StartRequest {
            operation_id: "first".into(),
            target: minerva_delegation::Target::Minerva { route: "api:fixture/fixture".into(), effort: None },
            prompt: "First task".into(),
            origin_run: None,
            tool_registrations: Default::default(),
        };
        let first = repository.admit(&caller, &request, &limits).unwrap().snapshot;
        let child_session = format!("del_{}", first.id);
        workspaces.create_session(&child_session, "default").unwrap();
        let input_id = "first_input";
        runtime
            .sessions
            .accept_run(
                &crate::StartRunRequest {
                    session_id: child_session.clone(),
                    request_id: first.activation_id.clone(),
                    parent_id: None,
                    message: crate::RunMessage { id: input_id.into(), text: request.prompt.clone() },
                    model: "api:fixture/fixture".into(),
                    options: json!({}),
                },
                "time",
            )
            .unwrap();
        runtime
            .sessions
            .commit_reply(
                &child_session,
                &crate::sessions::SessionMessage {
                    id: first.activation_id.clone(),
                    parent_id: Some(input_id.into()),
                    role: "assistant".into(),
                    created_at: "time".into(),
                    parts: vec![crate::sessions::ContentPart::text("text", "Earlier answer")],
                    extensions: Default::default(),
                },
                None,
            )
            .unwrap();
        repository
            .started(
                &first.id,
                &first.activation_id,
                &minerva_delegation::ExecutionHandle {
                    session_id: child_session.clone(),
                    run_id: first.activation_id.clone(),
                },
            )
            .unwrap();
        request.operation_id = "followup".into();
        request.prompt = "Finish after restart".into();
        let receipt = repository.enqueue(&caller, &first.id, &request, &limits).unwrap();
        repository
            .settle(
                &first.id,
                &first.activation_id,
                &minerva_delegation::Completion {
                    result_id: "first_result".into(),
                    state: ActivationState::Succeeded,
                    summary: "Earlier answer".into(),
                },
            )
            .unwrap();
        let interrupted = repository.promote(&caller, &first.id, &Default::default(), &limits).unwrap().unwrap();
        assert_eq!(interrupted.work_item_id, receipt.work_item_id);
        if admitted {
            runtime
                .sessions
                .accept_run(
                    &crate::StartRunRequest {
                        session_id: child_session.clone(),
                        request_id: interrupted.activation_id.clone(),
                        parent_id: Some(first.activation_id.clone()),
                        message: crate::RunMessage { id: "followup_input".into(), text: request.prompt.clone() },
                        model: "api:fixture/fixture".into(),
                        options: json!({}),
                    },
                    "time",
                )
                .unwrap();
        }
        runtime.shutdown().await.unwrap();
        drop(runtime);

        let recovered = Runtime::start_without_packages(config(dir.path(), true)).await.unwrap();
        let service = recovered.delegation.lock().await.as_ref().unwrap().service.clone();
        if admitted {
            let result = service.wait(&caller, &first.id, Duration::from_secs(5)).await.unwrap();
            assert_eq!(result.state, ActivationState::Unknown);
            assert_eq!(result.activation_id, interrupted.activation_id);
            assert_eq!(
                recovered.sessions.run_snapshot(&child_session, &result.activation_id).unwrap().status,
                "interrupted"
            );
            recovered.shutdown().await.unwrap();
            assert!(tokio::time::timeout(Duration::from_millis(150), model_requests.recv()).await.is_err());
            server.abort();
            return;
        }
        let wire = model_requests.recv().await.unwrap();
        assert!(wire["messages"].to_string().contains("Earlier answer"));
        assert!(wire["messages"].to_string().contains("Finish after restart"));
        let result = service.wait(&caller, &first.id, Duration::from_secs(5)).await.unwrap();
        assert_eq!(result.state, ActivationState::Succeeded);
        assert_eq!(result.activation_id, interrupted.activation_id);
        let history = recovered.sessions.load(&child_session).unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[2].parent_id.as_deref(), Some(first.activation_id.as_str()));
        recovered.shutdown().await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(150), model_requests.recv()).await.is_err());
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_wait_returns_question_and_idle_parent_wakes_only_once() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_without_packages(config(dir.path(), true)).await.unwrap();
        let (url, mut served, server) = model_responses(|turn| {
            if turn == 0 {
                (json!({"role":"assistant","tool_calls":[{"index":0,"id":"ask-1","type":"function",
                    "function":{"name":"ask_user","arguments":"{\"prompt\":\"Which approach?\",\"options\":[\"one\"]}"}}
                ]}), "tool_calls")
            } else if turn == 1 {
                (json!({"role":"assistant","content":"Parent presented the question"}), "stop")
            } else {
                (json!({"role":"assistant","content":"Choice received"}), "stop")
            }
        }).await;
        runtime
            .plugins
            .providers()
            .unwrap()
            .save(
                serde_json::from_value(json!({
                    "id":"fixture","name":"Fixture","baseUrl":url,"models":["fixture"],
                    "apiKey":null,"isNew":true,"protocol":"ollama"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
        workspaces.create_session("parent", "default").unwrap();
        workspaces.create_session("foreign", "default").unwrap();
        runtime
            .sessions
            .accept_user_run(
                &crate::StartRunRequest {
                    session_id: "parent".into(),
                    request_id: "parent_run".into(),
                    parent_id: None,
                    message: crate::RunMessage { id: "parent_input".into(), text: "Begin".into() },
                    model: "api:fixture/fixture".into(),
                    options: json!({}),
                },
                "time",
            )
            .unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let tools = runtime.plugins.harness.tools.snapshot().restrict_to(&[
            "subagent_start".into(),
            "subagent_wait".into(),
            "subagent_questions".into(),
            "subagent_answer".into(),
            "ask_user".into(),
        ]);
        let child: DelegationSnapshot = serde_json::from_value(tools.execute("subagent_start", json!({
            "operationId":"start","target":{"kind":"minerva","route":"api:fixture/fixture","effort":null},"prompt":"Ask"
        }), context("parent", &cwd, "start")).await.unwrap()).unwrap();
        let lookup = json!({"id":child.id,"timeoutMs":5000});
        let waiting = tools.execute("subagent_wait", lookup.clone(), context("parent", &cwd, "wait")).await.unwrap();
        assert_eq!(waiting["disposition"], "coordination-required", "{waiting}");
        assert_eq!(waiting["questions"][0]["prompt"], "Which approach?");
        let question_id = waiting["questions"][0]["id"].as_str().unwrap();
        let first = served.recv().await.unwrap();
        assert!(first["tools"].to_string().contains("ask_user"));
        assert!(tokio::time::timeout(Duration::from_millis(500), served.recv()).await.is_err());
        runtime
            .sessions
            .commit_reply(
                "parent",
                &crate::sessions::SessionMessage {
                    id: "parent_run".into(),
                    parent_id: Some("parent_input".into()),
                    role: "assistant".into(),
                    created_at: "time".into(),
                    parts: vec![crate::sessions::ContentPart::text("text", "Delegated inquiry")],
                    extensions: Default::default(),
                },
                None,
            )
            .unwrap();
        let wake = served.recv().await.unwrap();
        assert!(wake["messages"].to_string().contains("Which approach?"));
        assert!(wake.get("tools").is_none_or(|tools| tools.is_null() || tools.as_array().is_some_and(Vec::is_empty)));
        let wake_id = format!("wake_question_{question_id}");
        loop {
            if runtime.get_run("parent", &wake_id).unwrap().status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(tokio::time::timeout(Duration::from_millis(500), served.recv()).await.is_err());
        let answer = json!({"id":child.id,"questionId":question_id,"operationId":"answer","answer":"one"});
        assert!(tools.execute("subagent_answer", answer.clone(), context("foreign", &cwd, "foreign")).await.is_err());
        let receipt =
            tools.execute("subagent_answer", answer.clone(), context("parent", &cwd, "answer")).await.unwrap();
        assert_eq!(receipt["state"], "delivered");
        let replay = tools.execute("subagent_answer", answer, context("parent", &cwd, "replay")).await.unwrap();
        assert_eq!(replay, receipt);
        let finished = tools.execute("subagent_wait", lookup, context("parent", &cwd, "finish")).await.unwrap();
        assert_eq!(finished["completion"]["summary"], "Choice received");
        let second = served.recv().await.unwrap();
        assert!(second["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool" && message["content"].to_string().contains("one")));
        runtime.shutdown().await.unwrap();
        server.abort();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_user_send_queues_after_the_active_child_and_keeps_its_receipt() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_without_packages(config(dir.path(), true)).await.unwrap();
        let (url, mut served, server) = model().await;
        runtime.plugins.providers().unwrap().save(serde_json::from_value(json!({
            "id":"fixture","name":"Fixture","baseUrl":url,"models":["fixture"],
            "apiKey":null,"isNew":true,"protocol":"ollama"
        })).unwrap()).await.unwrap();
        let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
        workspaces.create_session("parent", "default").unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let tools = runtime.plugins.harness.tools.snapshot().restrict_to(&["subagent_start".into(), "subagent_send".into(), "subagent_wait".into()]);
        let child: DelegationSnapshot = serde_json::from_value(tools.execute("subagent_start", json!({
            "operationId":"start","target":{"kind":"minerva","route":"api:fixture/fixture","effort":null},"prompt":"First"
        }), context("parent", &cwd, "start")).await.unwrap()).unwrap();
        let args = json!({"id":child.id,"operationId":"ide-user-queued","prompt":"Second"});
        assert!(tools.execute("subagent_send", args, context("parent", &cwd, "reserved")).await.is_err());
        let forged = Caller { session_id: "foreign".into(), ..child.caller.clone() };
        assert!(runtime.followup_delegation_from_user(&forged, &child.id, "queued", "Second".into()).await.is_err());
        let queued = serde_json::to_value(runtime.followup_delegation_from_user(
            &child.caller, &child.id, "queued", "Second".into(),
        ).await.unwrap()).unwrap();
        assert_eq!(queued["disposition"], "queued");
        assert!(queued["activationId"].is_null());
        let wires = [served.recv().await.unwrap(), served.recv().await.unwrap()];
        assert!(wires[1]["messages"].to_string().contains("First"));
        assert!(wires[1]["messages"].to_string().contains("Second"));
        let finished: DelegationSnapshot = serde_json::from_value(tools.execute(
            "subagent_wait", json!({"id":child.id,"timeoutMs":5000}), context("parent", &cwd, "wait"),
        ).await.unwrap()).unwrap();
        assert_eq!(finished.state, ActivationState::Succeeded);
        assert_ne!(finished.activation_id, child.activation_id);
        assert_eq!(finished.request.origin_run, None);
        assert_eq!(finished.request.operation_id, "ide-user-queued");
        let replay = serde_json::to_value(runtime.followup_delegation_from_user(
            &child.caller, &child.id, "queued", "Second".into(),
        ).await.unwrap()).unwrap();
        assert_eq!(replay["disposition"], "queued");
        assert_eq!(replay["workItemId"], queued["workItemId"]);
        assert_eq!(replay["activationId"], finished.activation_id);
        runtime.shutdown().await.unwrap();
        server.abort();
    }).await.unwrap();
}

struct RecordingHost {
    starts: tokio::sync::mpsc::UnboundedSender<DelegationSnapshot>,
    release_first: AtomicBool,
    read_only: AtomicBool,
    writable: AtomicBool,
    reject_followup: AtomicBool,
    interrupted: std::sync::Mutex<Vec<String>>,
}

impl minerva_plugin_sdk::delegation::ExecutionPort for RecordingHost {
    fn capabilities(&self, _: &Target) -> ExecutionCapabilities {
        let supported = Capability::Supported { scope: "fixture".into() };
        ExecutionCapabilities {
            read_only: if self.read_only.load(Ordering::Acquire) {
                supported.clone()
            } else {
                Capability::Unsupported { reason: "Read-only mode unavailable".into() }
            },
            interrupt: supported.clone(),
            continuation: supported.clone(),
            nested_delegation_control: supported,
        }
    }

    fn discover<'a>(&'a self, _: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async {
            Ok(vec![
                Candidate {
                    target: Target::IdeAgent { agent_id: "missing".into(), options: Default::default() },
                    read_only: true,
                    unavailable_reason: Some("Executable unavailable".into()),
                },
                Candidate {
                    target: Target::IdeAgent { agent_id: "codex".into(), options: Default::default() },
                    read_only: true,
                    unavailable_reason: None,
                },
            ])
        })
    }

    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move {
            self.starts.send(snapshot.clone()).unwrap();
            Ok(ExecutionHandle { session_id: "native-thread".into(), run_id: snapshot.activation_id.clone() })
        })
    }

    fn supports_scoped_write(&self, _: &Caller, _: &Target) -> bool {
        self.writable.load(Ordering::Acquire)
    }

    fn check_scoped_followup<'a>(&'a self, _: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async {
            if self.reject_followup.load(Ordering::Acquire) {
                Err(DelegationError::new(ErrorCode::Unsupported, "Thread has no scoped tools"))
            } else {
                Ok(())
            }
        })
    }

    fn start_with_checkout<'a>(
        &'a self,
        snapshot: &'a DelegationSnapshot,
        checkout: minerva_plugin_sdk::delegation::NativeCheckout,
    ) -> ExecutionFuture<'a, ExecutionHandle> {
        assert_eq!(checkout.write_policy(snapshot), crate::tools::PermissionPolicy::Ask);
        let mut wrong = snapshot.clone();
        wrong.activation_id = "other".into();
        assert_eq!(checkout.write_policy(&wrong), crate::tools::PermissionPolicy::ReadOnly);
        self.start(snapshot)
    }

    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        Box::pin(async move {
            Ok(self.release_first.load(Ordering::Acquire).then(|| Completion {
                result_id: format!("{}_result", snapshot.activation_id),
                state: if self.interrupted.lock().unwrap().contains(&snapshot.activation_id) {
                    ActivationState::Cancelled
                } else {
                    ActivationState::Succeeded
                },
                summary: snapshot.request.prompt.clone(),
            }))
        })
    }

    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        self.interrupted.lock().unwrap().push(snapshot.activation_id.clone());
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn execution_deadline_interrupts_the_exact_activation_without_assuming_exit() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), true);
    std::fs::write(
        config.config_dir.join("minerva.conf.json"),
        json!({"version":1,"services":{"delegation":{"enabled":true,"executionDeadlineSeconds":1}}}).to_string(),
    )
    .unwrap();
    let (starts, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
    let adapter = Arc::new(RecordingHost {
        starts,
        release_first: false.into(),
        read_only: true.into(),
        writable: false.into(),
        reject_followup: false.into(),
        interrupted: Default::default(),
    });
    let runtime = Runtime::start_with_host_executor(config, adapter.clone()).await.unwrap();
    runtime.create_session("parent", Some("default")).unwrap();
    let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
    let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
    let service = runtime.delegation.lock().await.as_ref().unwrap().service.clone();
    let request = minerva_delegation::StartRequest {
        operation_id: "deadline".into(),
        prompt: "Wait for the deadline".into(),
        target: Target::IdeAgent { agent_id: "codex".into(), options: Default::default() },
        origin_run: None,
        tool_registrations: Default::default(),
    };
    let child = service.start(&caller, &request).await.unwrap();
    let launched = tokio::time::timeout(Duration::from_secs(3), dispatched.recv()).await.unwrap().unwrap();
    assert_eq!(launched.activation_id, child.activation_id);
    let observed = service.wait(&caller, &child.id, Duration::ZERO).await.unwrap();
    assert!(!observed.state.terminal(), "wait alone must not end execution");
    tokio::time::timeout(Duration::from_secs(5), async {
        while adapter.interrupted.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*adapter.interrupted.lock().unwrap(), [child.activation_id.clone()]);
    let stopping = service.get(&caller, &child.id).unwrap();
    assert_eq!(stopping.state, ActivationState::Stopping);
    assert!(stopping.completion.is_none());
    assert!(!runtime.sessions.delegation_active().unwrap().is_empty());
    adapter.release_first.store(true, Ordering::Release);
    let finished = service.wait(&caller, &child.id, Duration::from_secs(5)).await.unwrap();
    assert_eq!(finished.state, ActivationState::Cancelled);
    assert!(dispatched.try_recv().is_err());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_replays_a_saved_deadline_stop_that_never_reached_the_executor() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), true);
    let (starts, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
    let adapter = Arc::new(RecordingHost {
        starts,
        release_first: false.into(),
        read_only: true.into(),
        writable: false.into(),
        reject_followup: false.into(),
        interrupted: Default::default(),
    });
    let runtime = Runtime::start_with_host_executor(config.clone(), adapter.clone()).await.unwrap();
    runtime.create_session("parent", Some("default")).unwrap();
    let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
    let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
    runtime.shutdown().await.unwrap();
    let child = {
        let sessions = crate::sessions::SessionService::open(&config.data_dir.join("sessions.sqlite3")).unwrap();
        let repository = sessions.delegation_repository();
        let request = minerva_delegation::StartRequest {
            operation_id: "saved-deadline".into(),
            prompt: "Read the fixture".into(),
            target: Target::IdeAgent { agent_id: "codex".into(), options: Default::default() },
            origin_run: None,
            tool_registrations: Default::default(),
        };
        let child = repository.admit(&caller, &request, &Default::default()).unwrap().snapshot;
        let handle = ExecutionHandle { session_id: "native-thread".into(), run_id: child.activation_id.clone() };
        repository.started(&child.id, &child.activation_id, &handle).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let pending = sessions.delegation_deadline_stops(1).unwrap();
        assert_eq!(pending[0].activation_id, child.activation_id);
        child
    };
    let runtime = Runtime::start_with_host_executor(config, adapter.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while adapter.interrupted.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*adapter.interrupted.lock().unwrap(), [child.activation_id]);
    assert!(dispatched.try_recv().is_err());
    assert_eq!(runtime.sessions.delegation_active().unwrap()[0].state, ActivationState::Stopping);
    adapter.release_first.store(true, Ordering::Release);
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_queued_send_continues_the_completed_child_once() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let dir = tempfile::tempdir().unwrap();
        let (starts, mut launched) = tokio::sync::mpsc::unbounded_channel();
        let host = Arc::new(RecordingHost {
            starts,
            release_first: false.into(),
            read_only: true.into(),
            writable: false.into(),
            reject_followup: false.into(),
            interrupted: Default::default(),
        });
        let runtime_config = config(dir.path(), true);
        let primary = json!({"kind":"ide-agent","agentId":"missing","options":{}});
        let alternative = json!({"kind":"ide-agent","agentId":"codex","options":{}});
        std::fs::write(
            runtime_config.config_dir.join("minerva.conf.json"),
            json!({
                "version":1,"optionalPluginsEnabled":false,"services":{"delegation":{"enabled":true,"fallbacks":[
                    {"primary":primary,"alternative":alternative}
                ]}}
            })
            .to_string(),
        )
        .unwrap();
        let runtime = Runtime::start_with_host_executor(runtime_config.clone(), host.clone()).await.unwrap();
        let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
        workspaces.create_session("parent", "default").unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let tools = runtime.plugins.harness.tools.snapshot().restrict_to(&[
            "subagent_start".into(),
            "subagent_send".into(),
            "subagent_wait".into(),
        ]);
        let request = json!({"operationId":"start","target":primary,"prompt":"First"});
        let child: DelegationSnapshot = serde_json::from_value(
            tools.execute("subagent_start", request.clone(), context("parent", &cwd, "start")).await.unwrap(),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&child.request.target).unwrap(), alternative);
        assert_eq!(
            serde_json::to_value(&child.fallback).unwrap(),
            json!({
                "requestedTarget":primary,"reason":"Executable unavailable"
            })
        );
        let start_replay =
            tools.execute("subagent_start", request.clone(), context("parent", &cwd, "start_replay")).await.unwrap();
        assert_eq!(start_replay["activationId"], child.activation_id);
        let changed_target = json!({"operationId":"start","target":alternative,"prompt":"First"});
        let conflict =
            tools.execute("subagent_start", changed_target, context("parent", &cwd, "conflict")).await.unwrap_err();
        assert!(conflict.contains("different payload"), "{conflict}");
        let first = launched.recv().await.unwrap();
        assert_eq!(first.activation_id, child.activation_id);
        let args = json!({"id": child.id, "operationId": "queued", "prompt": "Second"});
        let queued = tools.execute("subagent_send", args.clone(), context("parent", &cwd, "queue")).await.unwrap();
        assert_eq!(queued["disposition"], "queued");
        assert!(queued["activationId"].is_null());
        assert!(tokio::time::timeout(Duration::from_millis(250), launched.recv()).await.is_err());
        host.read_only.store(false, Ordering::Release);
        host.release_first.store(true, Ordering::Release);
        let completed = tools
            .execute("subagent_wait", json!({"id": child.id, "timeoutMs": 5000}), context("parent", &cwd, "first"))
            .await
            .unwrap();
        assert_eq!(completed["state"], "succeeded");
        assert_eq!(completed["activationId"], first.activation_id);
        assert!(tokio::time::timeout(Duration::from_millis(500), launched.recv()).await.is_err());
        let deferred = tools.execute("subagent_send", args.clone(), context("parent", &cwd, "deferred")).await.unwrap();
        assert_eq!(deferred, queued);

        runtime.shutdown().await.unwrap();
        drop(runtime);
        let runtime = Runtime::start_with_host_executor(runtime_config, host.clone()).await.unwrap();
        let tools =
            runtime.plugins.harness.tools.snapshot().restrict_to(&["subagent_send".into(), "subagent_wait".into()]);
        let restored = tools.execute("subagent_send", args.clone(), context("parent", &cwd, "restored")).await.unwrap();
        assert_eq!(restored, queued);
        host.read_only.store(true, Ordering::Release);
        let second = launched.recv().await.unwrap();
        assert_eq!(second.id, child.id);
        assert_eq!(second.request.prompt, "Second");
        assert_eq!(
            second.previous_handle.unwrap(),
            ExecutionHandle { session_id: "native-thread".into(), run_id: first.activation_id }
        );
        let result: DelegationSnapshot = serde_json::from_value(
            tools
                .execute("subagent_wait", json!({"id": child.id, "timeoutMs": 5000}), context("parent", &cwd, "wait"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result.state, ActivationState::Succeeded);
        assert_eq!(result.activation_id, second.activation_id);
        let replay = tools.execute("subagent_send", args, context("parent", &cwd, "replay")).await.unwrap();
        assert_eq!(replay["workItemId"], queued["workItemId"]);
        assert_eq!(replay["activationId"], second.activation_id);
        host.read_only.store(false, Ordering::Release);
        let rejected = tools
            .execute(
                "subagent_send",
                json!({"id": child.id, "operationId": "blocked", "prompt": "Third"}),
                context("parent", &cwd, "blocked"),
            )
            .await
            .unwrap_err();
        assert!(rejected.contains("read-only mode"), "{rejected}");
        runtime.shutdown().await.unwrap();
        assert!(launched.try_recv().is_err());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_tool_runs_a_real_readonly_child_and_persists_result() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_without_packages(config(dir.path(), true)).await.unwrap();
        let (url, mut served, server) = model().await;
        runtime
            .plugins
            .providers()
            .unwrap()
            .save(
                serde_json::from_value(json!({
                    "id":"fixture","name":"Fixture","baseUrl":url,"models":["fixture"],
                    "apiKey":null,"isNew":true,"protocol":"ollama"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
        let workspace = workspaces.list().unwrap().into_iter().find(|w| w.id == "default").unwrap();
        workspaces.create_session("parent", &workspace.id).unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let tools = runtime.plugins.harness.tools.snapshot().restrict_to(&[
            "subagent_start".into(),
            "subagent_send".into(),
            "subagent_wait".into(),
            "read_file".into(),
        ]);
        let receipt = tools
            .execute(
                "subagent_start",
                json!({
                    "operationId":"integration","target":{"kind":"minerva","route":"api:fixture/fixture","effort":null},
                    "prompt":"Say hello"
                }),
                context("parent", &cwd, "start"),
            )
            .await
            .unwrap();
        let initial: DelegationSnapshot = serde_json::from_value(receipt).unwrap();
        let waiting = context("parent", &cwd, "cancelled_wait");
        let cancellation = waiting.cancellation.clone();
        let (cancelled, ()) =
            tokio::join!(tools.execute("subagent_wait", json!({"id":initial.id,"timeoutMs":20000}), waiting), async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                cancellation.cancel();
            });
        assert!(cancelled.is_err());
        let result = tools
            .execute("subagent_wait", json!({"id":initial.id,"timeoutMs":20000}), context("parent", &cwd, "wait"))
            .await
            .unwrap();
        let completed: DelegationSnapshot = serde_json::from_value(result).unwrap();
        assert_eq!(completed.state, ActivationState::Succeeded);
        assert_eq!(completed.completion.as_ref().unwrap().summary, "Hello from child");
        let child = completed.handle.unwrap().session_id;
        assert_eq!(runtime.sessions.internal_delegation_ancestors(&child).unwrap(), ["parent"]);
        let settings = runtime.plugins.harness.context.workspace_get(&child).unwrap();
        assert_eq!(settings.permission_policy, crate::tools::PermissionPolicy::ReadOnly);
        let write_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = write_called.clone();
        runtime
            .plugins
            .harness
            .tools
            .register(
                "fixture",
                crate::tools::ToolDefinition {
                    name: "fixture_write".into(),
                    description: "Test write marker".into(),
                    input_schema: json!({"type":"object"}),
                    output_schema: json!({}),
                    side_effect: true,
                },
                move |_, _| {
                    marker.store(true, std::sync::atomic::Ordering::SeqCst);
                    Box::pin(async { Ok(json!({"written":true})) })
                },
            )
            .unwrap();
        assert!(runtime
            .plugins
            .harness
            .tools
            .snapshot()
            .execute("fixture_write", json!({}), context(&child, &cwd, "write"))
            .await
            .is_err());
        assert!(!write_called.load(std::sync::atomic::Ordering::SeqCst));
        let history = runtime.sessions.load(&child).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert!(history[1].parts.iter().any(|p| p.data["text"] == "Hello from child"));
        assert!(runtime.sessions.load("parent").unwrap().is_empty());
        let direct = runtime
            .agent_loop
            .start_run(crate::StartRunRequest {
                session_id: child.clone(),
                request_id: "unadmitted".into(),
                parent_id: Some(history[1].id.clone()),
                message: crate::RunMessage { id: "unadmitted_input".into(), text: "Untrusted direct start".into() },
                model: "api:fixture/fixture".into(),
                options: json!({}),
            })
            .await;
        assert!(matches!(direct, Err(error) if error.contains("require a delegation activation")));
        assert_eq!(runtime.sessions.load(&child).unwrap().len(), 2);
        let args = json!({"id":initial.id,"operationId":"followup","prompt":"Say goodbye"});
        let followup: DelegationSnapshot = serde_json::from_value(
            tools.execute("subagent_send", args.clone(), context("parent", &cwd, "send")).await.unwrap(),
        )
        .unwrap();
        let replay: DelegationSnapshot = serde_json::from_value(
            tools.execute("subagent_send", args.clone(), context("parent", &cwd, "retry_send")).await.unwrap(),
        )
        .unwrap();
        assert_eq!(replay.activation_id, followup.activation_id);
        let continued: DelegationSnapshot = serde_json::from_value(
            tools
                .execute(
                    "subagent_wait",
                    json!({"id":initial.id,"timeoutMs":20000}),
                    context("parent", &cwd, "wait_again"),
                )
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(continued.state, ActivationState::Succeeded);
        assert_eq!(continued.handle.unwrap().session_id, child);
        let history = runtime.sessions.load(&child).unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[2].parent_id.as_deref(), Some(history[1].id.as_str()));
        let wires = [served.recv().await.unwrap(), served.recv().await.unwrap()];
        assert!(wires[1]["messages"].to_string().contains("Hello from child"));
        assert!(wires[1]["messages"].to_string().contains("Say goodbye"));
        let wire = &wires[0];
        assert_eq!(wire["model"], "fixture");
        assert!(wire["messages"].to_string().contains("Say hello"));
        let names: Vec<_> =
            wire["tools"].as_array().unwrap().iter().filter_map(|tool| tool["function"]["name"].as_str()).collect();
        assert_eq!(names, ["read_file"]);
        runtime.delegation.lock().await.as_ref().unwrap().service.close_admission();
        let after_close: DelegationSnapshot = serde_json::from_value(
            tools.execute("subagent_send", args, context("parent", &cwd, "retry_after_close")).await.unwrap(),
        )
        .unwrap();
        assert_eq!(after_close.activation_id, followup.activation_id);
        let mut expected_results =
            [format!("{}_result", initial.activation_id), format!("{}_result", followup.activation_id)];
        expected_results.sort();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let events = runtime.sessions.events_after("parent", 0, 100).unwrap();
                let mut results: Vec<_> = events
                    .iter()
                    .filter(|event| event.kind == "delegation.result")
                    .map(|event| event.data["resultId"].as_str().unwrap())
                    .collect();
                if results.len() >= expected_results.len() {
                    results.sort();
                    assert_eq!(results, expected_results);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("parent did not receive the durable result event");
        runtime.shutdown().await.unwrap();
        server.abort();
    })
    .await
    .expect("delegation integration fixture exceeded deadline");
}

#[tokio::test]
async fn leased_child_writes_only_with_exact_activation_and_inherited_permission() {
    use crate::run_supervisor::CheckoutWriteLease;
    use crate::tools::PermissionPolicy::{Allow, ReadOnly};
    struct Lease {
        session: String,
        activation: String,
    }
    impl CheckoutWriteLease for Lease {
        fn may_write(&self) -> bool {
            true
        }
        fn authorizes_child(&self, session: &str, activation: &str) -> bool {
            self.session == session && self.activation == activation
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let primary = Target::IdeAgent { agent_id: "codex".into(), options: Default::default() };
    let alternative = Target::Minerva { route: "api:fixture/fixture".into(), effort: None };
    let settings = config(dir.path(), true);
    std::fs::write(
        settings.config_dir.join("minerva.conf.json"),
        json!({
            "version":1,"optionalPluginsEnabled":false,"services":{"delegation":{
                "enabled":true,"fallbacks":[{"primary":primary,"alternative":alternative}]
            }}
        })
        .to_string(),
    )
    .unwrap();
    let runtime = Arc::new(Runtime::start_without_packages(settings).await.unwrap());
    let (url, _served, server) = model_responses(|step| {
        if step % 2 == 0 {
            let path = format!("child-{step}.txt");
            let arguments = json!({"path":path,"content":"from child"}).to_string();
            (json!({"role":"assistant","tool_calls":[{"index":0,"id":"write","type":"function","function":{"name":"write_file","arguments":arguments}}]}), "tool_calls")
        } else {
            (json!({"role":"assistant","content":"Done"}), "stop")
        }
    }).await;
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":"fixture","name":"Fixture","baseUrl":url,"models":["fixture"],
                "apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
    let workspace: crate::workspaces::WorkspaceInfo =
        serde_json::from_value(runtime.harness_call("workspaces.create", json!({"path":dir.path()})).await.unwrap())
            .unwrap();
    workspaces.create_session("parent", &workspace.id).unwrap();
    let context = &runtime.plugins.harness.context;
    let cwd = context.workspace_get("parent").unwrap().cwd.unwrap();
    let caller = runtime.sessions.delegation_caller("parent", &workspace.id, &cwd).unwrap();
    let all_writes = runtime.plugins.harness.tools.snapshot().delegation_writes();
    let registrations = std::collections::BTreeMap::from([("write_file".to_string(), all_writes["write_file"])]);
    for (step, parent_policy) in [(0, Allow), (2, ReadOnly)] {
        context.workspace_set_permission_policy("parent", parent_policy).unwrap();
        let request = minerva_delegation::StartRequest {
            operation_id: format!("operation-{step}"),
            prompt: "Write the file".into(),
            origin_run: None,
            tool_registrations: registrations.clone(),
            target: if step == 0 { primary.clone() } else { alternative.clone() },
        };
        let scoped = runtime
            .start_scoped_delegation(
                &caller,
                &request,
                None,
                || true,
                |snapshot| async move {
                    Ok(super::ChildCheckout {
                        ceiling: Allow,
                        lease: Arc::new(Lease {
                            session: format!("del_{}", snapshot.id),
                            activation: snapshot.activation_id,
                        }),
                    })
                },
            )
            .await
            .unwrap();
        assert_eq!(scoped.admitted.request.target, alternative);
        assert_eq!(scoped.admitted.fallback.is_some(), step == 0);
        let replay = runtime
            .start_scoped_delegation(
                &caller,
                &request,
                None,
                || true,
                |_| async { panic!("a duplicate must not obtain another grant") },
            )
            .await
            .unwrap();
        assert_eq!(replay.admitted.activation_id, scoped.admitted.activation_id);
        let completed = scoped.wait(Duration::from_secs(15)).await.unwrap();
        assert_eq!(completed.state, ActivationState::Succeeded, "{completed:?}");
        assert_eq!(completed.handle.unwrap().run_id, scoped.admitted.activation_id);
        let file = dir.path().join(format!("child-{step}.txt"));
        if parent_policy == Allow {
            assert_eq!(std::fs::read_to_string(file).unwrap(), "from child");
        } else {
            assert!(!file.exists());
        }
    }
    runtime.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn native_scoped_admission_requires_and_delivers_the_exact_grant() {
    use crate::run_supervisor::CheckoutWriteLease;
    use crate::tools::PermissionPolicy::{Allow, Ask};

    struct Lease(String, String);
    impl CheckoutWriteLease for Lease {
        fn may_write(&self) -> bool {
            true
        }

        fn authorizes_child(&self, session: &str, activation: &str) -> bool {
            self.0 == session && self.1 == activation
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let (starts, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
    let adapter = Arc::new(RecordingHost {
        starts,
        release_first: false.into(),
        read_only: true.into(),
        writable: false.into(),
        reject_followup: false.into(),
        interrupted: Default::default(),
    });
    let runtime = Runtime::start_with_host_executor(config(dir.path(), true), adapter.clone()).await.unwrap();
    let workspaces = runtime.plugins.context().try_service::<crate::workspaces::WorkspaceService>().unwrap();
    workspaces.create_session("parent", "default").unwrap();
    let context = &runtime.plugins.harness.context;
    let cwd = context.workspace_get("parent").unwrap().cwd.unwrap();
    let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
    context.workspace_set_permission_policy("parent", Allow).unwrap();
    let request = minerva_delegation::StartRequest {
        operation_id: "native-scoped".into(),
        prompt: "Edit the fixture".into(),
        origin_run: None,
        tool_registrations: Default::default(),
        target: Target::IdeAgent { agent_id: "codex".into(), options: Default::default() },
    };
    let grant = |snapshot: DelegationSnapshot| async move {
        Ok(super::ChildCheckout {
            ceiling: Ask,
            lease: Arc::new(Lease(format!("del_{}", snapshot.id), snapshot.activation_id)),
        })
    };
    assert!(runtime.start_scoped_delegation(&caller, &request, None, || true, grant).await.is_err());
    assert!(dispatched.try_recv().is_err());
    adapter.writable.store(true, Ordering::Release);
    let receipt = runtime.start_scoped_delegation(&caller, &request, None, || true, grant).await.unwrap();
    let started = tokio::time::timeout(Duration::from_secs(5), dispatched.recv()).await.unwrap().unwrap();
    assert_eq!(started.activation_id, receipt.admitted.activation_id);
    tokio::time::timeout(Duration::from_secs(5), async {
        while receipt.wait(Duration::ZERO).await.unwrap().handle.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    receipt.interrupt().await.unwrap();
    assert_eq!(*adapter.interrupted.lock().unwrap(), [receipt.admitted.activation_id.clone()]);
    adapter.release_first.store(true, Ordering::Release);
    let result = receipt.wait(Duration::from_secs(5)).await.unwrap();
    assert_eq!(result.state, ActivationState::Cancelled);
    receipt.interrupt().await.unwrap();
    assert_eq!(adapter.interrupted.lock().unwrap().len(), 1);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn scoped_followup_keeps_the_native_thread_and_allocates_one_new_grant() {
    use crate::run_supervisor::CheckoutWriteLease;
    use crate::tools::PermissionPolicy::{Allow, Ask};

    struct Lease(String, String);
    impl CheckoutWriteLease for Lease {
        fn may_write(&self) -> bool {
            true
        }
        fn authorizes_child(&self, session: &str, activation: &str) -> bool {
            self.0 == session && self.1 == activation
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let (starts, mut dispatched) = tokio::sync::mpsc::unbounded_channel();
    let adapter = Arc::new(RecordingHost {
        starts,
        release_first: true.into(),
        read_only: true.into(),
        writable: true.into(),
        reject_followup: true.into(),
        interrupted: Default::default(),
    });
    let runtime = Runtime::start_with_host_executor(config(dir.path(), true), adapter.clone()).await.unwrap();
    runtime.create_session("parent", Some("default")).unwrap();
    let context = &runtime.plugins.harness.context;
    let cwd = context.workspace_get("parent").unwrap().cwd.unwrap();
    context.workspace_set_permission_policy("parent", Allow).unwrap();
    let caller = runtime.sessions.delegation_caller("parent", "default", &cwd).unwrap();
    runtime
        .sessions
        .accept_user_run(
            &crate::StartRunRequest {
                session_id: "parent".into(),
                request_id: "parent-run".into(),
                parent_id: None,
                message: crate::RunMessage { id: "input".into(), text: "Delegate".into() },
                model: "api:fixture/fixture".into(),
                options: json!({}),
            },
            "time",
        )
        .unwrap();
    let mut request = minerva_delegation::StartRequest {
        operation_id: "scoped-first".into(),
        prompt: "First write".into(),
        origin_run: Some("parent-run".into()),
        tool_registrations: runtime.plugins.harness.tools.snapshot().delegation_writes(),
        target: Target::IdeAgent { agent_id: "codex".into(), options: Default::default() },
    };
    let grant = |snapshot: DelegationSnapshot| async move {
        Ok(super::ChildCheckout {
            ceiling: Ask,
            lease: Arc::new(Lease(format!("del_{}", snapshot.id), snapshot.activation_id)),
        })
    };
    let first = runtime.start_scoped_delegation(&caller, &request, None, || true, grant).await.unwrap();
    let first = first.wait(Duration::from_secs(5)).await.unwrap();
    assert_eq!(first.state, ActivationState::Succeeded);
    assert_eq!(dispatched.try_recv().unwrap().activation_id, first.activation_id);
    request.operation_id = "scoped-followup".into();
    request.prompt = "Second write".into();
    let failure = runtime
        .start_scoped_delegation(
            &caller,
            &request,
            Some(&first.id),
            || true,
            |_| async { panic!("ineligible thread must not acquire a grant") },
        )
        .await
        .err()
        .unwrap();
    assert_eq!(failure.code, ErrorCode::Unsupported);
    assert_eq!(runtime.sessions.delegation_repository().get(&caller, &first.id).unwrap(), first);
    assert!(dispatched.try_recv().is_err());
    adapter.reject_followup.store(false, Ordering::Release);
    let second = runtime.start_scoped_delegation(&caller, &request, Some(&first.id), || true, grant).await.unwrap();
    let second = second.wait(Duration::from_secs(5)).await.unwrap();
    assert_eq!(second.state, ActivationState::Succeeded);
    assert_eq!(second.id, first.id);
    assert_eq!(second.previous_handle, first.handle);
    assert_ne!(second.activation_id, first.activation_id);
    assert_eq!(dispatched.try_recv().unwrap().activation_id, second.activation_id);
    let duplicate = runtime
        .start_scoped_delegation(
            &caller,
            &request,
            Some(&first.id),
            || false,
            |_| async { panic!("replay must not acquire a new grant") },
        )
        .await
        .unwrap();
    assert_eq!(duplicate.admitted.activation_id, second.activation_id);
    assert!(dispatched.try_recv().is_err());
    runtime.shutdown().await.unwrap();
}
