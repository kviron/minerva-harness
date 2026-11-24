//! Production provider entry exercises Rig's single run and real loopback HTTP.
use super::*;
use crate::{
    selection::SelectionFuture,
    tools::{Cancellation, InteractionBroker, PermissionPolicy, ToolContext, ToolDefinition, ToolRegistry},
};
use mh_sessions::{DispatchAdmission, RouteDispatch, RouteSelection};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    sync::Mutex as StdMutex,
    time::{Duration, Instant},
};

struct Control {
    requested: StdMutex<RouteSelection>,
    dispatched: StdMutex<Vec<RouteDispatch>>,
    duplicate: bool,
}
impl SelectionControl for Control {
    fn selection(&self) -> SelectionFuture<'_, RouteSelection> {
        Box::pin(async { Ok(self.requested.lock().unwrap().clone()) })
    }
    fn record_dispatch(&self, value: RouteDispatch) -> SelectionFuture<'_, DispatchAdmission> {
        Box::pin(async move {
            if self.duplicate {
                return Ok(DispatchAdmission::Duplicate);
            }
            let mut dispatched = self.dispatched.lock().unwrap();
            let changed = !dispatched.is_empty();
            dispatched.push(value);
            Ok(DispatchAdmission::Recorded { changed })
        })
    }
}
fn server() -> (String, std::thread::JoinHandle<Vec<Value>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let mut captured = Vec::new();
        let mut deadline = Instant::now() + Duration::from_secs(3);
        while captured.len() < 2 {
            let (mut socket, _) = match listener.accept() {
                Ok(socket) => socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("accept failed: {error}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 4096];
            let (end, length) = loop {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(pos) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    let length = String::from_utf8_lossy(&bytes[..pos])
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (pos + 4, length);
                }
            };
            while bytes.len() < end + length {
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            captured.push(serde_json::from_slice(&bytes[end..end + length]).unwrap());
            let (delta, finish) = if captured.len() == 1 {
                (
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":"once","type":"function","function":{"name":"inspect","arguments":"{}"}}]}),
                    "tool_calls",
                )
            } else {
                (json!({"role":"assistant","content":"done"}), "stop")
            };
            let chunk = json!({"id":"fixture","object":"chat.completion.chunk","created":1,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
            let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            deadline = Instant::now() + Duration::from_millis(500);
        }
        captured
    });
    (url, worker)
}

async fn exercise(
    revoke: bool,
    duplicate: bool,
) -> (Result<transcript::Transcript, String>, Vec<Value>, Arc<Control>, usize) {
    let (url, worker) = server();
    let dir = tempfile::tempdir().unwrap();
    let id = format!(
        "fixture-{}",
        dir.path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    );
    let service = ProviderService::new_with_store(
        dir.path().join("providers.json"),
        CredentialScope::new(&id).unwrap(),
        Arc::new(credential_store::MemoryCredentialStore::default()),
        EndpointPolicy::Local,
    )
    .unwrap();
    service
        .save(ProviderInput {
            id: id.clone(),
            name: "Fixture".into(),
            base_url: url,
            models: vec!["a".into(), "b".into(), "c".into()],
            api_key: Some("fixture".into()),
            is_new: true,
            protocol: "openai-chat".into(),
        })
        .await
        .unwrap();
    let control = Arc::new(Control {
        requested: StdMutex::new(RouteSelection { revision: 1, model: format!("api:{id}/a"), reasoning_effort: None }),
        dispatched: StdMutex::new(Vec::new()),
        duplicate,
    });
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tools = ToolRegistry::default();
    let tool_control = control.clone();
    let tool_source = service.clone();
    let tool_id = id.clone();
    let tool_calls = calls.clone();
    tools
        .register(
            "fixture",
            ToolDefinition {
                name: "inspect".into(),
                description: "inspect".into(),
                input_schema: json!({"type":"object","properties":{}}),
                output_schema: json!({"type":"object"}),
                side_effect: false,
            },
            move |_, _| {
                let control = tool_control.clone();
                let source = tool_source.clone();
                let id = tool_id.clone();
                let calls = tool_calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    *control.requested.lock().unwrap() =
                        RouteSelection { revision: 2, model: format!("api:{id}/b"), reasoning_effort: None };
                    *control.requested.lock().unwrap() =
                        RouteSelection { revision: 3, model: format!("api:{id}/c"), reasoning_effort: None };
                    if revoke {
                        source.delete(&id).await?;
                    }
                    Ok(json!({"result":"TOOL_ONCE"}))
                })
            },
        )
        .unwrap();
    let cancellation = Cancellation::default();
    let context = Arc::new(context::ContextService::open(&dir.path().join("context.sqlite")).unwrap());
    let run = AgentRunContext {
        session_id: "session".into(),
        run_id: "run".into(),
        tools: tools.snapshot(),
        tool_context: ToolContext {
            session_id: "session".into(),
            run_id: "run".into(),
            call_id: String::new(),
            cwd: None,
            policy: PermissionPolicy::ReadOnly,
            live_policy: None,
            cancellation,
            interactions: Arc::new(InteractionBroker::default()),
            sink: None,
        },
        context,
        messages: Vec::new(),
        reasoning_effort: None,
        selection: Some(control.clone()),
    };
    let result = service.chat_agent(&id, "a", vec![Turn::plain("user", "inspect")], Arc::new(|_| Ok(())), run).await;
    (result, worker.join().unwrap(), control, calls.load(std::sync::atomic::Ordering::SeqCst))
}
#[tokio::test]
async fn provider_entry_switches_latest_model_after_tool_without_replaying_it() {
    let (result, requests, control, calls) = exercise(false, false).await;
    assert!(result.is_ok(), "{result:?}");
    let transcript = result.unwrap();
    assert_eq!(transcript.parts.iter().filter(|part| part.data["source"] == "model-selection").count(), 1);
    assert!(!transcript.replay["rig-v1"].to_string().contains("Next model request:"));
    assert_eq!(calls, 1);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "a");
    assert_eq!(requests[1]["model"], "c");
    assert_eq!(
        requests[1]["messages"].as_array().unwrap().iter().filter(|message| message["role"] == "tool").count(),
        1
    );
    assert!(requests[1].to_string().contains("TOOL_ONCE"));
    assert!(!requests[1].to_string().contains("Next model request:"));
    let issued = control.dispatched.lock().unwrap();
    assert_eq!(issued.len(), 2);
    assert_eq!(issued[0].selection.revision, 1);
    assert_eq!(issued[1].selection.revision, 3);
}
#[tokio::test]
async fn revoked_provider_stops_next_completion_after_recorded_tool() {
    let (result, requests, control, calls) = exercise(true, false).await;
    assert!(result.is_err());
    assert_eq!(calls, 1);
    assert_eq!(requests.len(), 1);
    assert_eq!(control.dispatched.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn duplicate_handler_admission_never_reissues_http() {
    let (result, requests, _, calls) = exercise(false, true).await;
    assert!(result.is_err());
    assert!(requests.is_empty());
    assert_eq!(calls, 0);
}
