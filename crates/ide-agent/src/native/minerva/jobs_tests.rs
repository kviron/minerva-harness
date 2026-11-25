use super::test_http::{request, response};
use super::*;

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_handoff_continues_the_model_turn_and_uses_the_native_background_controls() {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
    let runtime = service.runtime().await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let provider = format!("jobs-{}", uuid::Uuid::new_v4());
    runtime
        .plugins
        .providers()
        .unwrap()
        .save(
            serde_json::from_value(json!({
                "id":provider,"name":"Jobs fixture","baseUrl":format!("http://{address}/v1"),
                "models":["model"],"apiKey":null,"isNew":true,"protocol":"ollama"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let command = if cfg!(windows) {
                "Write-Output 'background-ready'; Start-Sleep -Seconds 30"
            } else {
                "printf 'background-ready\\n'; sleep 30"
            };
            response(&mut socket,json!({"role":"assistant","tool_calls":[{"index":0,"id":"start-server", "type":"function",
                "function":{"name":"shell","arguments":json!({"command":command,"label":"Dev server","run_in_background":true}).to_string()}}]}),"tool_calls").await;
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = request(&mut socket).await;
            assert!(request["messages"].as_array().unwrap().iter().any(|message| {
                message["role"] == "tool" && message["content"].as_str().is_some_and(|text| text.contains("jobId"))
            }), "the next model call must receive the job handle, not wait for the server to exit");
            response(&mut socket,json!({"role":"assistant","content":"The dev server is running in the background."}),"stop").await;
        }).await.unwrap();
    });
    struct Client(Arc<Mutex<Vec<Value>>>);
    impl Handler for Client {
        fn on_notification(&self, _: &str, value: Value) {
            self.0.lock().push(value);
        }
        fn on_request(&self, _: &str, _: Value) -> Result<Value, (i64, String)> {
            Err((codes::METHOD_NOT_FOUND, "unexpected".into()))
        }
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let peer = connect(service.clone(), dir.path(), Arc::new(Client(events.clone())), Some(format!("api:{provider}/")));
    let initialized: Value = peer.request("initialize", json!({}), None).await.unwrap();
    assert_eq!(initialized.pointer("/agentCapabilities/_meta/openidea/backgroundTasks"), Some(&json!(true)));
    let created: Value = peer.request("session/new", json!({}), None).await.unwrap();
    let session = created["sessionId"].as_str().unwrap();
    let mut settings = runtime.harness_call("workspace.get", json!({"sessionId":session})).await.unwrap();
    settings["autoCompact"] = json!(false);
    runtime.harness_call("workspace.save", json!({"sessionId":session,"settings":settings})).await.unwrap();
    let _: Value = peer
        .request("session/set_config_option", json!({"sessionId":session,"configId":"mode","value":"allow"}), None)
        .await
        .unwrap();
    let answer: Value = tokio::time::timeout(
        Duration::from_secs(15),
        peer.request(
            "session/prompt",
            json!({
                "sessionId":session,"prompt":[{"type":"text","text":"Start a managed dev server"}]
            }),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(answer["stopReason"], "end_turn");
    server.await.unwrap();
    let jobs = runtime.plugins.jobs().unwrap();
    until(|| jobs.list(session).iter().any(|job| job.stdout.contains("background-ready"))).await;
    let job = jobs.list(session).pop().unwrap();
    assert!(job.status.is_active(), "normal turn cleanup must not kill the handed-off process");
    until(|| {
        events.lock().iter().any(|event| {
            event["update"]["tasks"].as_array().is_some_and(|tasks| {
                tasks.iter().any(|task| {
                    task["task_id"] == job.id
                        && task["output"].as_str().is_some_and(|output| output.contains("background-ready"))
                })
            })
        })
    })
    .await;
    let other: Value = peer.request("session/new", json!({}), None).await.unwrap();
    assert!(
        peer.request::<_, Value>("_openidea/stop_task", json!({"sessionId":other["sessionId"],"taskId":job.id}), None)
            .await
            .is_err()
    );
    let _: Value = peer
        .request("session/set_config_option", json!({"sessionId":session,"configId":"mode","value":"read-only"}), None)
        .await
        .unwrap();
    let _: Value =
        peer.request("_openidea/stop_task", json!({"sessionId":session,"taskId":job.id}), None).await.unwrap();
    until(|| jobs.list(session).iter().all(|job| !job.status.is_active())).await;
    until(|| {
        events.lock().iter().any(|event| {
            event["update"]["tasks"]
                .as_array()
                .is_some_and(|tasks| tasks.iter().any(|task| task["task_id"] == job.id && task["status"] == "stopped"))
        })
    })
    .await;
    peer.kill();
    service.shutdown().await.unwrap();
}
