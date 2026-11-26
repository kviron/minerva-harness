//! Integration gate for using Rig's existing route selection inside one run.
use super::*;
use rig_agent::agent::hook::{
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, ModelSelection, ModelSelectionAction,
    RequestPatch,
};
use serde_json::Value;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct PendingSelection(Arc<AtomicBool>);
impl AgentHook for PendingSelection {
    fn on_model_select(&self, _: &HookContext, _: ModelSelection<'_>) -> ModelSelectionAction {
        if self.0.load(Ordering::Acquire) {
            ModelSelectionAction::select("replacement")
        } else {
            ModelSelectionAction::continue_run()
        }
    }
    async fn on_completion_call(&self, _: &HookContext, event: CompletionCallEvent<'_>) -> CompletionCallAction {
        if self.0.load(Ordering::Acquire) {
            let mut history = event.history.to_vec();
            history.push(Arc::new(rig_core::message::Message::user("STEERING: inspect only parser")));
            return CompletionCallAction::patch(RequestPatch::new().history(history));
        }
        CompletionCallAction::continue_run()
    }
}

fn endpoint(reply: Value) -> (String, std::thread::JoinHandle<Value>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "provider was never called");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        let (header_end, content_length) = loop {
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0, "request ended before headers");
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 1024 * 1024);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let size = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .expect("content length");
                break (end + 4, size);
            }
        };
        while bytes.len() < header_end + content_length {
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
        }
        let request = serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap();
        let body = reply.to_string();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        request
    });
    (url, worker)
}

#[tokio::test]
async fn switches_registered_route_and_delivers_steering_after_tool_once() {
    let (first_url, first) = endpoint(serde_json::json!({
        "id":"first","object":"chat.completion","created":1,"model":"route-a",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call1","type":"function","function":{"name":"inspect","arguments":"{}"}}]},"finish_reason":"tool_calls"}]
    }));
    let (second_url, second) = endpoint(serde_json::json!({
        "id":"second","object":"chat.completion","created":1,"model":"route-b",
        "choices":[{"index":0,"message":{"role":"assistant","content":"inspection finished"},"finish_reason":"stop"}]
    }));
    let selected = Arc::new(AtomicBool::new(false));
    let executions = Arc::new(AtomicUsize::new(0));
    let tool_selection = selected.clone();
    let tool_executions = executions.clone();
    let tool = rig_core::tool::DynamicTool::new_with_context(
        rig_core::message::ToolName::new("inspect").unwrap(),
        "inspect parser",
        serde_json::json!({"type":"object","properties":{}}),
        move |_: &mut rig_core::tool::ToolContext, _| {
            tool_executions.fetch_add(1, Ordering::SeqCst);
            tool_selection.store(true, Ordering::Release);
            Box::pin(async { Ok(rig_core::tool::ToolOutput::json(serde_json::json!({"result":"TOOL_RESULT"}))) })
        },
    );
    let agent = AgentBuilder::new(openai_config("fixture-a", &first_url, "fixture".into()).client().chat("route-a"))
        .model_route("replacement", openai_config("fixture-b", &second_url, "fixture".into()).client().chat("route-b"))
        .dynamic_tools(vec![tool])
        .build();
    agent.prompt("inspect code").add_hook(PendingSelection(selected)).max_turns(3).await.unwrap();
    let initial = first.join().unwrap();
    let replacement = second.join().unwrap();
    assert_eq!(initial["model"], "route-a");
    assert_eq!(replacement["model"], "route-b");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let messages = replacement["messages"].as_array().unwrap();
    assert_eq!(messages.iter().filter(|message| message["role"] == "tool").count(), 1);
    assert_eq!(messages.iter().filter(|message| message.to_string().contains("STEERING:")).count(), 1);
    let result = messages.iter().position(|message| message["role"] == "tool").unwrap();
    let steering = messages.iter().position(|message| message.to_string().contains("STEERING:")).unwrap();
    assert!(result < steering, "tool results must precede the next instruction: {messages:?}");
    assert!(replacement.to_string().contains("TOOL_RESULT"));
    assert!(!initial.to_string().contains("STEERING:"));
}
