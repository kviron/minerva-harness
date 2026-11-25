use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) async fn request(socket: &mut tokio::net::TcpStream) -> Value {
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

pub(super) async fn response(socket: &mut tokio::net::TcpStream, delta: Value, finish: &str) {
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
