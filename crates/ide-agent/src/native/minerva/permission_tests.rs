use super::test_http::{request, response};
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_mode_changes_apply_to_the_next_tool_without_changing_the_workspace() {
    for initial_mode in ["read-only", "ask"] {
        policy_case(true, initial_mode, false).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_run_cannot_claim_native_policy_authority_through_json() {
    policy_case(false, "read-only", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transferred_lease_blocks_parent_tool_dispatch() {
    policy_case(true, "allow", true).await;
}

async fn policy_case(trusted_native: bool, initial_mode: &str, transfer_lease: bool) {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
    let runtime = service.runtime().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = format!("policy-{}", uuid::Uuid::new_v4());
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":provider,"name":"Policy fixture","baseUrl":format!("http://{address}/v1"),
                "models":["model"],"apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    runtime
        .plugins
        .harness
        .tools
        .register(
            "policy-fixture",
            ide_minerva::minerva_runtime::tools::ToolDefinition {
                name: "policy_effect".into(),
                description: "Count a fixture side effect".into(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({}),
                side_effect: true,
            },
            {
                let executions = executions.clone();
                move |_, _| {
                    let executions = executions.clone();
                    Box::pin(async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({"done":true}))
                    })
                }
            },
        )
        .unwrap();
    let (first_seen, first_wait) = tokio::sync::oneshot::channel();
    let (release_first, first_release) = tokio::sync::oneshot::channel();
    let (second_seen, second_wait) = tokio::sync::oneshot::channel();
    let (release_second, second_release) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(20), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            first_seen.send(()).unwrap();
            first_release.await.unwrap();
            response(&mut socket, json!({"role":"assistant","tool_calls":[{"index":0,"id":"effect-one","type":"function","function":{"name":"policy_effect","arguments":"{}"}}]}), "tool_calls").await;
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            second_seen.send(()).unwrap();
            second_release.await.unwrap();
            response(&mut socket, json!({"role":"assistant","tool_calls":[{"index":0,"id":"effect-two","type":"function","function":{"name":"policy_effect","arguments":"{}"}}]}), "tool_calls").await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let last = request(&mut socket).await;
            response(&mut socket, json!({"role":"assistant","content":"finished"}), "stop").await;
            last
        }).await.expect("policy fixture must complete within its deadline")
    });
    let peer =
        connect(service.clone(), dir.path(), Arc::new(ide_jsonrpc::NoopHandler), Some(format!("api:{provider}/")));
    let new: Value = peer.request("session/new", json!({}), None).await.unwrap();
    let session = new["sessionId"].as_str().unwrap();
    let mode = |value| json!({"sessionId":session,"configId":"mode","value":value});
    let _: Value = peer.request("session/set_config_option", mode(initial_mode), None).await.unwrap();
    let mut before = runtime.harness_call("workspace.get", json!({"sessionId":session})).await.unwrap();
    before["autoCompact"] = json!(false);
    before["modelEffort"] = json!([format!("api:{provider}/model"), "default"]);
    runtime.harness_call("workspace.save", json!({"sessionId":session,"settings":before})).await.unwrap();
    let native_run = uuid::Uuid::new_v4().to_string();
    if trusted_native {
        let error = peer.request::<_, Value>(
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":"Unbound"}],"_meta":{"openidea":{"nativeRunId":native_run}}}),
            None,
        ).await.unwrap_err();
        assert!(error.to_string().contains("no active checkout lease"));
    }
    let _lease = service.bind_native_run_lease(session, &native_run, Arc::new(())).unwrap();
    let prompt = tokio::spawn({
        let native_run = native_run.clone();
        let peer = peer.clone();
        let runtime = runtime.clone();
        let session = session.to_owned();
        let model = format!("api:{provider}/model");
        async move {
            if trusted_native {
                peer.request::<_, Value>(
                    "session/prompt",
                    json!({"sessionId":session,"prompt":[{"type":"text","text":"Exercise two policy boundaries"}],"_meta":{"openidea":{"nativeRunId":native_run}}}),
                    None,
                )
                .await
                .map_err(|e| e.to_string())
            } else {
                let request = serde_json::from_value(json!({
                    "sessionId":session,"requestId":"ordinary-run","parentId":null,
                    "message":{"id":"ordinary-user","text":"Exercise two policy boundaries"},"model":model,
                    "options":{"nativePolicy":true,"nativeUserMutable":true,"permissionPolicy":"allow"}
                }))
                .unwrap();
                let handle = runtime.start_run(request).await?;
                loop {
                    let snapshot = runtime.get_run(&session, &handle.run_id)?;
                    if !matches!(snapshot.status.as_str(), "accepted" | "running") {
                        return Ok(json!({"status":snapshot.status}));
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(15), first_wait).await.unwrap().unwrap();
    let run = runtime.sessions.baseline(session).unwrap().active_run.unwrap().run_id;
    if trusted_native {
        assert_eq!(run, native_run);
    }
    let allowed: Value = peer.request("session/set_config_option", mode("allow"), None).await.unwrap();
    assert_eq!(allowed["configOptions"][1]["currentValue"], "allow");
    let mut expected = before.clone();
    expected["permissionPolicy"] = json!("allow");
    assert_eq!(runtime.harness_call("workspace.get", json!({"sessionId":session})).await.unwrap(), expected);
    assert!(
        runtime
            .harness_call("workspace.save", json!({"sessionId":session,"settings":expected}))
            .await
            .unwrap_err()
            .contains("active run")
    );
    release_first.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), second_wait).await.unwrap().unwrap();
    assert_eq!(executions.load(Ordering::SeqCst), usize::from(trusted_native));
    let handoff = if transfer_lease {
        Some(service.begin_native_run_handoff(session, &run, "del_fixture", "fixture-activation").await.unwrap())
    } else {
        let _: Value = peer.request("session/set_config_option", mode("read-only"), None).await.unwrap();
        expected["permissionPolicy"] = json!("read-only");
        None
    };
    assert_eq!(runtime.sessions.baseline(session).unwrap().active_run.unwrap().run_id, run);
    assert_eq!(runtime.harness_call("workspace.get", json!({"sessionId":session})).await.unwrap(), expected);
    release_second.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), prompt).await.unwrap().unwrap().unwrap();
    if let Some(handoff) = handoff {
        handoff.finish_handoff("fixture-activation").unwrap();
    }
    if trusted_native {
        assert!(peer.request::<_, Value>(
            "session/prompt",
            json!({"sessionId":session,"prompt":[{"type":"text","text":"Duplicate"}],"_meta":{"openidea":{"nativeRunId":native_run}}}),
            None,
        ).await.unwrap_err().to_string().contains("already used"));
    }
    let last = server.await.unwrap();
    assert!(
        last["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "effect-two")
    );
    assert_eq!(executions.load(Ordering::SeqCst), usize::from(trusted_native));
    assert_eq!(runtime.get_run(session, &run).unwrap().status, "completed");
    let events = runtime.sessions.events_after(session, 0, 1000).unwrap();
    let run_ids: std::collections::HashSet<_> = events
        .iter()
        .filter(|event| event.kind == "run.updated")
        .filter_map(|event| event.data["runId"].as_str())
        .collect();
    assert_eq!(run_ids, std::collections::HashSet::from([run.as_str()]));
    assert!(runtime.plugins.harness.interactions.list(session).is_empty());
    peer.kill();
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn minerva_runtime_owns_checkout_lease_after_host_releases_registration() {
    scoped_write_case(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_file_writes_use_child_scoped_approval() {
    scoped_write_case(Some("allow")).await;
    scoped_write_case(Some("deny")).await;
    scoped_write_case(Some("cancel")).await;
}

async fn scoped_write_case(decision: Option<&'static str>) {
    struct Approval {
        decision: Option<&'static str>,
        requests: Arc<Mutex<Vec<Value>>>,
        answer_after_cancel: Arc<AtomicBool>,
    }
    impl Handler for Approval {
        fn on_notification(&self, _: &str, _: Value) {}

        fn on_request(&self, method: &str, params: Value) -> Result<Value, (i64, String)> {
            assert_eq!(method, "session/request_permission");
            self.requests.lock().push(params);
            let mut option = self.decision.expect("allow mode does not request approval");
            if option == "cancel" {
                let until = Instant::now() + Duration::from_secs(10);
                while !self.answer_after_cancel.load(Ordering::Acquire) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                option = "allow";
            }
            Ok(json!({"outcome":{"outcome":"selected","optionId":option}}))
        }
    }
    use crate::workspace_writes::WorkspaceWrite;

    let dir = tempfile::tempdir().unwrap();
    let checkout = dir.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let config = dir.path().join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("minerva.conf.json"),
        json!({
            "version":1,"optionalPluginsEnabled":false,"services":{"delegation":{"enabled":true}}
        })
        .to_string(),
    )
    .unwrap();
    let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
    let runtime = service.runtime().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = format!("lease-{}", uuid::Uuid::new_v4());
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":provider,"name":"Lease fixture","baseUrl":format!("http://{address}/v1"),
                "models":["model"],"apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let approvals = Arc::new(Mutex::new(Vec::new()));
    let answer_after_cancel = Arc::new(AtomicBool::new(false));
    let handler =
        Arc::new(Approval { decision, requests: approvals.clone(), answer_after_cancel: answer_after_cancel.clone() });
    let peer = connect(service.clone(), &checkout, handler, Some(format!("api:{provider}/")));
    let new: Value = peer.request("session/new", json!({}), None).await.unwrap();
    let session = new["sessionId"].as_str().unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    let writer = WorkspaceWrite::acquire(vec![checkout.clone()]).await.unwrap();
    let turn = Arc::new(writer.claim_turn().unwrap());
    let registration = service.bind_native_run_lease(session, &run, turn.clone()).unwrap();
    let prompt = tokio::spawn({
        let peer = peer.clone();
        let request = json!({"sessionId":session,"prompt":[{"type":"text","text":"Keep the model request open"}],"_meta":{"openidea":{"nativeRunId":run}}});
        async move { peer.request::<_, Value>("session/prompt", request, None).await }
    });
    let mut parent_socket = tokio::time::timeout(Duration::from_secs(15), async {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        socket
    })
    .await
    .unwrap();
    use ide_minerva::minerva_runtime::tools::PermissionPolicy;
    let policy = if decision.is_some() { PermissionPolicy::Ask } else { PermissionPolicy::Allow };
    runtime.plugins.harness.context.workspace_set_permission_policy(session, policy).unwrap();
    let arguments = json!({
        "operationId":"child-write",
        "prompt":"Write the marker file",
        "target":{"kind":"minerva","route":format!("api:{provider}/model"),"effort":null},
        "writable":true,
    })
    .to_string();
    response(&mut parent_socket, json!({"role":"assistant","tool_calls":[{"index":0,"id":"delegate","type":"function","function":{"name":"subagent_start","arguments":arguments}}]}), "tool_calls").await;
    let (mut child_socket, _) =
        tokio::time::timeout(Duration::from_secs(15), listener.accept()).await.unwrap().unwrap();
    request(&mut child_socket).await;
    assert!(!registration.parent_holds_write());
    assert!(WorkspaceWrite::acquire(vec![checkout.clone()]).await.is_err());
    let arguments = json!({"path":"child.txt","content":"child write"}).to_string();
    response(&mut child_socket, json!({"role":"assistant","tool_calls":[{"index":0,"id":"write","type":"function","function":{"name":"write_file","arguments":arguments}}]}), "tool_calls").await;
    if decision == Some("cancel") {
        let approval = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(approval) = approvals.lock().first().cloned() {
                    break approval;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let owner = ChildApproval::from_permission(&approval).unwrap();
        assert!(runtime.cancel_run(owner.session, owner.run).unwrap());
        tokio::time::timeout(Duration::from_secs(10), async {
            while !runtime.plugins.harness.interactions.list(owner.session).is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        answer_after_cancel.store(true, Ordering::Release);
    } else {
        let (mut child_socket, _) =
            tokio::time::timeout(Duration::from_secs(15), listener.accept()).await.unwrap().unwrap();
        request(&mut child_socket).await;
        response(&mut child_socket, json!({"role":"assistant","content":"Done"}), "stop").await;
    }
    let (mut _parent_socket, _) =
        tokio::time::timeout(Duration::from_secs(15), listener.accept()).await.unwrap().unwrap();
    let next = request(&mut _parent_socket).await;
    let outcome = if decision == Some("cancel") { "cancelled" } else { "succeeded" };
    assert!(next.to_string().contains(outcome), "{next}");
    let file = checkout.join("child.txt");
    if matches!(decision, Some("deny" | "cancel")) {
        assert!(!file.exists());
    } else {
        assert_eq!(std::fs::read_to_string(file).unwrap(), "child write");
    }
    let requests = approvals.lock().clone();
    assert_eq!(requests.len(), usize::from(decision.is_some()));
    if let Some(request) = requests.first() {
        assert_eq!(request["sessionId"], session);
        assert!(request["toolCall"]["toolCallId"].as_str().unwrap().starts_with("del_"));
        let title = request["toolCall"]["title"].as_str().unwrap();
        assert!(title.contains(&format!("{provider}/model")));
        assert!(title.contains("write_file"));
        assert!(title.contains("child.txt"));
    }
    assert!(registration.parent_holds_write());
    drop(registration);
    drop(turn);
    drop(writer);
    assert!(WorkspaceWrite::acquire(vec![checkout.clone()]).await.is_err());
    assert_eq!(runtime.sessions.baseline(session).unwrap().active_run.unwrap().run_id, run);

    peer.kill();
    assert!(tokio::time::timeout(Duration::from_secs(10), prompt).await.unwrap().unwrap().is_err());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(writer) = WorkspaceWrite::acquire(vec![checkout.clone()]).await {
                let status = runtime.get_run(session, &run).unwrap().status;
                assert!(matches!(status.as_str(), "cancelled" | "failed"), "{status}");
                break writer;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}
