use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0 && bytes.len() < 2_000_000);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}

async fn response(socket: &mut tokio::net::TcpStream, delta: Value, finish: &str) {
    let mut body = String::new();
    for (delta, reason) in [(delta, Value::Null), (json!({}), json!(finish))] {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"id":"test","object":"chat.completion.chunk","created":1,
            "model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":reason}]})
        ));
    }
    body.push_str("data: [DONE]\n\n");
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_mode_changes_apply_to_the_next_tool_without_changing_the_workspace() {
    for initial_mode in ["read-only", "ask"] {
        policy_case(true, initial_mode).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_run_cannot_claim_native_policy_authority_through_json() {
    policy_case(false, "read-only").await;
}

async fn policy_case(trusted_native: bool, initial_mode: &str) {
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
    let prompt = tokio::spawn({
        let peer = peer.clone();
        let runtime = runtime.clone();
        let session = session.to_owned();
        let model = format!("api:{provider}/model");
        async move {
            if trusted_native {
                peer.request::<_, Value>(
                    "session/prompt",
                    json!({"sessionId":session,"prompt":[{"type":"text","text":"Exercise two policy boundaries"}]}),
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
    let _: Value = peer.request("session/set_config_option", mode("read-only"), None).await.unwrap();
    assert_eq!(runtime.sessions.baseline(session).unwrap().active_run.unwrap().run_id, run);
    expected["permissionPolicy"] = json!("read-only");
    assert_eq!(runtime.harness_call("workspace.get", json!({"sessionId":session})).await.unwrap(), expected);
    release_second.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), prompt).await.unwrap().unwrap().unwrap();
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
