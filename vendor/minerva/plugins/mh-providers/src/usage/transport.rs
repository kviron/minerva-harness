use futures_util::StreamExt;
use rig_reqwest::reqwest;
use serde_json::Value;
use std::time::Duration;

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_LIMIT: usize = 1024 * 1024;

pub(crate) async fn get(url: &str, key: &str, account: Option<&str>) -> Result<Value, &'static str> {
    let work = async {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| "network")?;
        let mut request = client
            .get(url)
            .bearer_auth(key)
            .header("Accept", "application/json")
            .header("User-Agent", "Minerva subscription usage");
        if let Some(account) = account {
            request = request.header("ChatGPT-Account-Id", account);
        }
        let response = request.send().await.map_err(|e| if e.is_timeout() { "timeout" } else { "network" })?;
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err("auth"),
            429 => return Err("rate-limited"),
            _ => return Err("network"),
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "network")?;
            if bytes.len() + chunk.len() > BODY_LIMIT {
                return Err("invalid-response");
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "invalid-response")
    };
    tokio::time::timeout(REQUEST_TIMEOUT, work).await.map_err(|_| "timeout")?
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn server(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        let reply=format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/secret\r\nConnection: close\r\n\r\n{body}",body.len());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = vec![0; 8192];
            let size = socket.read(&mut data).await.unwrap();
            socket.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&data[..size]).to_string()
        });
        (url, task)
    }
    #[tokio::test]
    async fn headers_are_scoped_and_errors_are_redacted() {
        let (url, task) = server("200 OK", "{}").await;
        assert!(get(&url, "test-secret", Some("account-id")).await.is_ok());
        let headers = task.await.unwrap().to_lowercase();
        assert!(headers.contains("authorization: bearer test-secret"));
        assert!(headers.contains("chatgpt-account-id: account-id"));
        for (status, expected) in
            [("401 Unauthorized", "auth"), ("429 Too Many Requests", "rate-limited"), ("302 Found", "network")]
        {
            let (url, task) = server(status, "private server response").await;
            assert_eq!(get(&url, "test-secret", None).await.unwrap_err(), expected);
            task.await.unwrap();
        }
    }
}
