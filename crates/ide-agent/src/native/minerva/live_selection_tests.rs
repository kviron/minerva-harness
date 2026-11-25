use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0 && bytes.len() < 2_000_000);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, length) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| length.trim().parse::<usize>().unwrap())
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
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_live_selection_changes_provider_on_the_next_model_call_after_a_tool() {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
    let runtime = service.runtime().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = format!("live-{}", uuid::Uuid::new_v4());
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":provider,"name":"Live fixture","baseUrl":format!("http://{address}/v1"),
                "models":["model-a","model-b","model-c"],"apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    runtime
        .plugins
        .harness
        .tools
        .register(
            "live-fixture",
            ide_minerva::minerva_runtime::tools::ToolDefinition {
                name: "inspect_fixture".into(),
                description: "inspect fixture".into(),
                input_schema: json!({"type":"object"}),
                output_schema: json!({}),
                side_effect: false,
            },
            {
                let executions = executions.clone();
                let entered = entered.clone();
                let release = release.clone();
                move |_, _| {
                    let executions = executions.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        entered.notify_one();
                        release.notified().await;
                        Ok(json!({"observation":"fixture-result"}))
                    })
                }
            },
        )
        .unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(20), async move {
            let (mut first,_) = listener.accept().await.unwrap();
            let initial=request(&mut first).await;
            response(&mut first,json!({"role":"assistant","tool_calls":[{"index":0,"id":"fixture-call","type":"function","function":{"name":"inspect_fixture","arguments":"{}"}}]}),"tool_calls").await;
            let (mut second,_) = listener.accept().await.unwrap();
            let replacement=request(&mut second).await;
            response(&mut second,json!({"role":"assistant","content":"fixture-final"}),"stop").await;
            (initial,replacement)
        }).await.expect("the native run must issue exactly two requests")
    });
    let foreign = format!("foreign-{}", uuid::Uuid::new_v4());
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":foreign,"name":"Foreign fixture","baseUrl":format!("http://{address}/v1"),
                "models":["other"],"apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let peer = connect(service.clone(), dir.path(), Arc::new(super::tests::Client(seen.clone())), None);
    let new: Value = peer.request("session/new", json!({}), None).await.unwrap();
    let session = new["sessionId"].as_str().unwrap().to_owned();
    let _: Value = peer
        .request(
            "session/set_config_option",
            json!({
                "sessionId":session,"configId":"model","value":format!("api:{provider}/model-a")
            }),
            None,
        )
        .await
        .unwrap();
    let running = tokio::spawn({
        let peer = peer.clone();
        let session = session.clone();
        async move {
            peer.request::<_, Value>(
                "session/prompt",
                json!({"sessionId":session,"prompt":[{"type":"text","text":"inspect fixture"}]}),
                None,
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(20), entered.notified()).await.expect("tool must start");
    peer.request::<_, Value>(
        "session/set_config_option",
        json!({"sessionId":session,"configId":"model","value":format!("api:{foreign}/other")}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(runtime.sessions.selection(&session).unwrap().unwrap().model, format!("api:{foreign}/other"));
    let changed: Value = peer
        .request(
            "session/set_config_option",
            json!({"sessionId":session,"configId":"model","value":format!("api:{provider}/model-b")}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(changed["configOptions"][0]["currentValue"], format!("api:{provider}/model-b"));
    let latest: Value = peer
        .request(
            "session/set_config_option",
            json!({
                "sessionId":session,"configId":"model","value":format!("api:{provider}/model-c")
            }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(latest["configOptions"][0]["currentValue"], format!("api:{provider}/model-c"));
    peer.request::<_, Value>(
        "session/set_config_option",
        json!({"sessionId":session,"configId":"model","value":format!("api:{foreign}/other")}),
        None,
    )
    .await
    .unwrap();
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(20), running).await.unwrap().unwrap().unwrap();
    assert_eq!(result["stopReason"], "end_turn");
    let (initial, replacement) = server.await.unwrap();
    assert_eq!(initial["model"], "model-a");
    assert_eq!(replacement["model"], "other");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert!(replacement.to_string().contains("fixture-result"));
    assert_eq!(runtime.sessions.list().unwrap().len(), 1);
    let desired = runtime.sessions.selection(&session).unwrap().unwrap();
    assert_eq!(desired.model, format!("api:{foreign}/other"));
    let events = runtime.sessions.events_after(&session, 0, 1000).unwrap();
    let dispatches: Vec<_> = events.iter().filter(|event| event.kind == "run.selection_dispatch_intent").collect();
    assert_eq!(dispatches.len(), 2);
    assert_eq!(dispatches[0].data["selection"]["model"], format!("api:{provider}/model-a"));
    assert_eq!(dispatches[1].data["selection"]["model"], desired.model);
    assert_eq!(dispatches[1].data["selection"]["revision"], desired.revision);
    let history = runtime.sessions.load(&session).unwrap();
    let notices = history
        .iter()
        .flat_map(|message| &message.parts)
        .filter(|part| part.data["source"] == "model-selection")
        .count();
    assert_eq!(notices, 1);
    assert!(!replacement.to_string().contains("Next model request:"));
    peer.kill();
    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_request_keeps_its_explicit_model_without_live_selection() {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
    let runtime = service.runtime().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = format!("headless-{}", uuid::Uuid::new_v4());
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":provider,"name":"Headless fixture","baseUrl":format!("http://{address}/v1"),
                "models":["model-a","model-b"],"apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    runtime.create_session("headless", None).unwrap();
    runtime.sessions.request_selection("headless", &format!("api:{provider}/model-b"), None).unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(20), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let received = request(&mut socket).await;
            response(&mut socket, json!({"role":"assistant","content":"headless answer"}), "stop").await;
            received
        })
        .await
        .unwrap()
    });
    runtime
        .start_run(StartRunRequest {
            session_id: "headless".into(),
            request_id: "headless-run".into(),
            parent_id: None,
            message: RunMessage { id: "headless-input".into(), text: "headless question".into() },
            model: format!("api:{provider}/model-a"),
            options: json!({}),
        })
        .await
        .unwrap();
    let snapshot = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let snapshot = runtime.get_run("headless", "headless-run").unwrap();
            if snapshot.status != "running" {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(snapshot.status, "completed", "{:?}", snapshot.error);
    assert_eq!(server.await.unwrap()["model"], "model-a");
    assert!(
        runtime
            .sessions
            .events_after("headless", 0, 1000)
            .unwrap()
            .iter()
            .all(|event| event.kind != "run.selection_dispatch_intent")
    );
    service.shutdown().await.unwrap();
}
