//! Split the summarizer input to leave room for instructions, previous summary and response.
use crate::sessions::SessionMessage;

pub(super) fn chunks(messages: &[SessionMessage], window: usize) -> Result<Vec<String>, String> {
    let limit = window.min(96_000).max(256);
    let mut result = Vec::new();
    let mut chunk = String::new();
    for message in messages {
        for part in &message.parts {
            let mut text = serde_json::to_string(&serde_json::json!({"role":message.role,"part":part}))
                .map_err(|e| e.to_string())?;
            if part.kind == "tool-result" {
                if let Some(short) = crate::context::prune_text(&text) {
                    text = short;
                }
            }
            let mut rest = text.as_str();
            while !rest.is_empty() {
                let n = rest.floor_char_boundary(rest.len().min(limit - chunk.len()));
                chunk.push_str(&rest[..n]);
                rest = &rest[n..];
                if !rest.is_empty() || chunk.len() == limit {
                    result.push(std::mem::take(&mut chunk));
                }
            }
            chunk.push('\n');
        }
    }
    if !chunk.trim().is_empty() {
        result.push(chunk);
    }
    Ok(result)
}
