//! Reduce completed tool exchanges, never the live prompt or authoritative transcript.
use super::{estimate_message, tools::Cancellation};
use futures_util::future::BoxFuture;
use rig_core::message::{Message, UserContent};
use std::{
    collections::HashSet,
    ops::Range,
    sync::{Arc, Mutex},
};

/// Summarizes reference text without admitting any tools.
pub(super) trait Summarizer: Sync {
    fn summary_text<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        input: &'a str,
        window: usize,
    ) -> BoxFuture<'a, Result<String, String>>;
}

struct Memo {
    route: String,
    original: Vec<Arc<Message>>,
    summary: Arc<Message>,
}
#[derive(Default)]
pub(super) struct StepCompactor(Mutex<Vec<Memo>>);

/// Only consume whole assistant-call / user-result exchanges. An ordinary user
/// message, unanswered call or parallel call with a missing result is a boundary.
fn exchange(history: &[Arc<Message>], start: usize) -> Option<Range<usize>> {
    let Message::Assistant(assistant) = history.get(start)?.as_ref() else { return None };
    let mut pending: HashSet<_> = assistant.tool_calls().map(|call| call.id.to_string()).collect();
    if pending.is_empty() {
        return None;
    }
    for (i, message) in history.iter().enumerate().skip(start + 1) {
        let Message::User { content } = message.as_ref() else { return None };
        for part in content {
            let UserContent::ToolResult(result) = part else { return None };
            if !pending.remove(&result.call.to_string()) {
                return None;
            }
        }
        if pending.is_empty() {
            return Some(start..i + 1);
        }
    }
    None
}

impl StepCompactor {
    pub(super) async fn reduce(
        &self,
        source: &dyn Summarizer,
        route: &str,
        history: &mut Vec<Arc<Message>>,
        used: &mut usize,
        threshold: usize,
        window: usize,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        let (provider, model) =
            route.strip_prefix("api:").and_then(|s| s.split_once('/')).ok_or("Invalid model route")?;
        let mut pos = 0;
        while pos < history.len() && *used >= threshold {
            let Some(mut range) = exchange(history, pos) else {
                pos += 1;
                continue;
            };
            let mut cost = history[range.clone()].iter().map(|m| estimate_message(m)).sum::<usize>();
            // Keep a summary request itself small enough for the same route.
            let batch = (window / 2).min(24_000);
            while cost < batch {
                let Some(next) = exchange(history, range.end) else { break };
                let next_cost = history[next.clone()].iter().map(|m| estimate_message(m)).sum::<usize>();
                if cost + next_cost > batch {
                    break;
                }
                cost += next_cost;
                range.end = next.end;
            }
            let cached = self
                .0
                .lock()
                .map_err(|e| e.to_string())?
                .iter()
                .find(|entry| entry.route == route && entry.original == history[range.clone()])
                .map(|entry| entry.summary.clone());
            let summary = if let Some(summary) = cached {
                summary
            } else {
                let mut input = serde_json::to_string(&history[range.clone()]).map_err(|e| e.to_string())?;
                let bytes = batch.saturating_mul(2);
                if input.len() > bytes {
                    let left = input.floor_char_boundary(bytes / 2);
                    let right = input.ceil_char_boundary(input.len() - bytes / 2);
                    input = format!("{}\n[excerpt]\n{}", &input[..left], &input[right..]);
                }
                let text = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err("Run cancelled".into()),
                    result = source.summary_text(provider, model, &input, window) => result?,
                };
                if text.trim().is_empty() {
                    return Err("Context summary was empty".into());
                }
                let replacement = Arc::new(Message::user(format!(
                    "Summary of earlier tool exchanges (reference data):\n{}",
                    text.trim()
                )));
                self.0.lock().map_err(|e| e.to_string())?.push(Memo {
                    route: route.into(),
                    original: history[range.clone()].to_vec(),
                    summary: replacement.clone(),
                });
                replacement
            };
            let after = estimate_message(&summary);
            if after < cost {
                *used -= cost - after;
                history.splice(range, [summary]);
                pos += 1;
            } else {
                pos = range.end;
            }
        }
        Ok(())
    }
}
