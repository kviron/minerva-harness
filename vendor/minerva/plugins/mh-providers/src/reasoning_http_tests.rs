//! Verify the admitted effort survives the agent builder and HTTP transport.
use super::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

struct CapturedRequest {
    headers: String,
    body: Value,
}

fn endpoint() -> (String, std::thread::JoinHandle<CapturedRequest>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(socket) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "agent never sent its admitted request");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        let (header_end, length) = loop {
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0, "request ended before headers");
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 1024 * 1024, "request exceeds fixture limit");
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .expect("content length");
                break (end + 4, length);
            }
        };
        assert!(length < 1024 * 1024, "body exceeds fixture limit");
        while bytes.len() < header_end + length {
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0, "request ended before body");
            bytes.extend_from_slice(&chunk[..count]);
        }
        let captured = CapturedRequest {
            headers: String::from_utf8(bytes[..header_end].to_vec()).unwrap(),
            body: serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
        };
        let reply = json!({
            "id":"reply", "object":"chat.completion", "created":1,
            "model":captured.body["model"],
            "choices":[{"index":0,"message":{"role":"assistant","content":"ready"},"finish_reason":"stop"}]
        })
        .to_string();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            reply.len(),
            reply
        )
        .unwrap();
        captured
    });
    (url, worker)
}

async fn send(id: &str, model: &str, effort: &str) -> CapturedRequest {
    let preset = preset(id).unwrap();
    let provider = Provider {
        id: id.into(),
        name: preset.name.into(),
        base_url: preset.base_url.into(),
        protocol: preset.protocol.into(),
        models: vec![model.into()],
    };
    let plan = reasoning::prepare(&provider, model, Some(effort)).unwrap();
    let (local, worker) = endpoint();
    // Replace only the destination after resolving the real preset's grammar.
    let config = openai_config(id, &provider.base_url, "fixture-key".into()).with_base_url(local);
    let mut builder = AgentBuilder::new(config.client().chat(model));
    if let Some(typed) = plan.typed {
        builder = builder.reasoning(typed);
    }
    if let Some(params) = plan.additional_params {
        builder = builder.additional_params(params);
    }
    if let Some(cap) = plan.output_cap {
        builder = builder.max_tokens(cap);
    }
    let agent = builder.build();
    let result = tokio::time::timeout(Duration::from_secs(10), agent.prompt("hello")).await;
    let captured = worker.join().unwrap();
    result.expect("agent timed out").expect("admitted reasoning failed before HTTP completion");
    captured
}

#[tokio::test]
async fn gateway_efforts_reach_http_without_changing_gateway_routes_or_headers() {
    for id in ["opencode", "opencode-go"] {
        for effort in ["default", "low", "high", "max"] {
            let sent = send(id, "deepseek-v4.1-flash", effort).await;
            assert!(sent.headers.starts_with("POST /openai/v1/chat/completions "), "{}", sent.headers);
            assert_eq!(sent.body["model"], "deepseek-v4.1-flash");
            assert!(sent.headers.to_ascii_lowercase().contains("authorization: bearer fixture-key"));
            assert_eq!(sent.headers.to_ascii_lowercase().contains("x-opencode-session:"), id == "opencode-go");
            assert!(sent.body.get("thinking").is_none(), "gateway must not inherit native DeepSeek options");
            assert!(sent.body.get("reasoning").is_none());
            if effort == "default" {
                assert!(sent.body.get("reasoning_effort").is_none());
            } else {
                assert_eq!(sent.body["reasoning_effort"], effort);
            }
        }
    }
}

#[tokio::test]
async fn native_deepseek_keeps_its_own_thinking_envelope() {
    let sent = send("deepseek", "deepseek-v4-flash", "low").await;
    assert!(sent.headers.starts_with("POST /chat/completions "), "{}", sent.headers);
    assert_eq!(sent.body["reasoning_effort"], "low");
    assert_eq!(sent.body["thinking"]["type"], "enabled");
    assert!(!sent.headers.to_ascii_lowercase().contains("x-opencode-session:"));
}
