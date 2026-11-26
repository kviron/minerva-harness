use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the installed Claude Code CLI; uses a local model fixture"]
async fn installed_claude_scoped_write_preserves_the_checkout_boundary() {
    use minerva_plugin_sdk::tools::PermissionPolicy;
    use serde_json::{Value, json};
    use std::{sync::Arc, time::Duration};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn receive(socket: &mut tokio::net::TcpStream) -> (String, Value) {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0 && bytes.len() < 2_000_000);
            bytes.extend_from_slice(&chunk[..n]);
            let Some(end) = bytes.windows(4).position(|chunk| chunk == b"\r\n\r\n") else { continue };
            let header = String::from_utf8_lossy(&bytes[..end]);
            let target = header.split_whitespace().nth(1).unwrap().to_owned();
            let count: usize = header
                .lines()
                .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("content-length")))
                .map_or(0, |(_, value)| value.trim().parse().unwrap());
            if bytes.len() >= end + 4 + count {
                let payload = &bytes[end + 4..end + 4 + count];
                return (target, serde_json::from_slice(payload).unwrap_or(Value::Null));
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    std::fs::create_dir(&root).unwrap();
    let root = dunce::canonicalize(root).unwrap();
    let outside = dir.path().join("outside.txt");
    std::fs::write(&outside, "untouched").unwrap();
    let inside = root.join("inside.txt");
    let second = root.join("second.txt");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture_paths = [Some(inside.clone()), Some(outside.clone()), None, Some(second.clone()), None];
    let server = tokio::spawn(async move {
        let mut step = 0;
        while step < fixture_paths.len() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (path, input) = receive(&mut socket).await;
            if !path.starts_with("/v1/messages") || input["messages"].is_null() {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
                continue;
            }
            if step == 3 {
                assert!(
                    input["messages"].to_string().contains("Use the fixture file tools"),
                    "Claude did not resume its history"
                );
            }
            let (block, delta, reason) = if let Some(path) = &fixture_paths[step] {
                (
                    json!({"type":"tool_use","id":format!("toolu_{step}"),"name":"Write","input":{}}),
                    json!({"type":"input_json_delta","partial_json":json!({"file_path":path,"content":"fixture write"}).to_string()}),
                    "tool_use",
                )
            } else {
                (json!({"type":"text","text":""}), json!({"type":"text_delta","text":"done"}), "end_turn")
            };
            let events = [
                json!({"type":"message_start","message":{"id":format!("msg_{step}"),"type":"message","role":"assistant",
                    "content":[],"model":input["model"],"stop_reason":null,"stop_sequence":null,
                    "usage":{"input_tokens":3,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":block}),
                json!({"type":"content_block_delta","index":0,"delta":delta}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":20}}),
                json!({"type":"message_stop"}),
            ];
            let body: String = events
                .iter()
                .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
                .collect();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            step += 1;
        }
    });

    let manager = Arc::new(crate::AgentManager::new(
        crate::AgentPaths {
            project_root: root.clone(),
            store_dir: dir.path().join("store"),
            worktrees_dir: dir.path().join("worktrees"),
            models_file: dir.path().join("models.json"),
            custom_agents_dir: None,
            home_dir: None,
            env_secrets: None,
            catalogs: Arc::default(),
        },
        tokio::runtime::Handle::current(),
    ));
    let profile = dir.path().join("claude-config");
    {
        let mut providers = manager.shared.providers.write();
        let claude = providers.iter_mut().find(|provider| provider.id == "claude").unwrap();
        claude.env.extend([
            ("ANTHROPIC_BASE_URL".into(), format!("http://{address}")),
            ("ANTHROPIC_API_KEY".into(), "minerva-local-fixture".into()),
            ("ANTHROPIC_AUTH_TOKEN".into(), String::new()),
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), String::new()),
            ("CLAUDE_CONFIG_DIR".into(), profile.to_string_lossy().into_owned()),
        ]);
    }
    let mut snapshot: DelegationSnapshot = serde_json::from_value(json!({
        "id":"child","workItemId":"work","activationId":"live-write","depth":1,"state":"starting",
        "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":root},
        "request":{"operationId":"operation","target":{"kind":"ide-agent","agentId":"claude","options":{}},"prompt":"Use the fixture file tools"}
    })).unwrap();
    let executor = HostExecutor::new(Arc::downgrade(&manager));
    let run = async |snapshot: &mut DelegationSnapshot| {
        let writer = crate::workspace_writes::WorkspaceWrite::acquire(vec![root.clone()]).await.unwrap();
        let owner = Arc::new(writer.claim_turn().unwrap());
        let checkout = NativeCheckout::new(snapshot, PermissionPolicy::Allow, || PermissionPolicy::Allow)
            .with_process_journal(Some(owner));
        let handle = executor.start_with_checkout(snapshot, checkout).await.unwrap();
        snapshot.handle = Some(handle);
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match executor.observe(snapshot).await {
                    Ok(Some(result)) => break result,
                    Ok(None) => {}
                    Err(error) if error.code == ErrorCode::Busy => {}
                    Err(error) => {
                        let state = manager.task(&snapshot.handle.as_ref().unwrap().session_id).unwrap();
                        panic!("{error:?}, task status: {:?}", state.state.lock().task);
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(result.state, ActivationState::Succeeded, "{result:?}");
    };
    run(&mut snapshot).await;
    assert_eq!(std::fs::read_to_string(inside).unwrap(), "fixture write");
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    let first = snapshot.handle.take().unwrap();
    snapshot.previous_handle = Some(first.clone());
    snapshot.activation_id = "live-continue".into();
    snapshot.request.operation_id = "continue-operation".into();
    snapshot.request.prompt = "Continue with the second fixture file".into();
    run(&mut snapshot).await;
    let next = snapshot.handle.as_ref().unwrap();
    assert_eq!(next.session_id, first.session_id);
    assert_ne!(next.run_id, first.run_id);
    assert_eq!(std::fs::read_to_string(second).unwrap(), "fixture write");
    tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
    manager.shutdown();
}
