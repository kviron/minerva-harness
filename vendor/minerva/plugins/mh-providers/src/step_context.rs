//! Project the durable Rig history into a bounded request. Both provider hooks use this path.
use super::{budget, context, estimate_message, project_message, AgentRunContext};
use context::meter::{pressure_threshold, RequestMeasure};
use rig_core::message::{Message, ToolResultContent, UserContent};
use std::sync::Arc;

pub(crate) struct StepView {
    pub history: Vec<Arc<Message>>,
    pub output: u64,
}

/// Retain the call ID, outcome and both ends of long results. The original transcript
/// remains available for inspection and no tools are executed by this projection.
fn condense_result(message: &Message) -> Message {
    let mut message = message.clone();
    if let Message::User { content } = &mut message {
        for part in content {
            if let UserContent::ToolResult(result) = part {
                let text = serde_json::to_string(&result.content).unwrap_or_default();
                if text.len() > 768 {
                    let start = text.floor_char_boundary(384);
                    let end = text.ceil_char_boundary(text.len() - 192);
                    result.content = vec![ToolResultContent::text(format!(
                        "{}\n[earlier tool output shortened; full result in session]\n{}",
                        &text[..start],
                        &text[end..]
                    ))];
                }
            }
        }
    }
    message
}

fn fit_history(history: &[Arc<Message>], fixed: usize, threshold: usize, auto: bool) -> (Vec<Arc<Message>>, usize) {
    let mut projected: Vec<_> = history.iter().map(|m| Arc::new(project_message(m))).collect();
    let mut used = fixed + projected.iter().map(|m| estimate_message(m)).sum::<usize>();
    if auto && used >= threshold {
        for message in &mut projected {
            if used < threshold {
                break;
            }
            if !matches!(message.as_ref(), Message::User { content } if content.iter().any(|p| matches!(p, UserContent::ToolResult(_))))
            {
                continue;
            }
            let condensed = condense_result(message);
            let before = estimate_message(message);
            let after = estimate_message(&condensed);
            if after < before {
                used -= before - after;
                *message = Arc::new(condensed);
            }
        }
    }
    (projected, used)
}

pub(super) struct StepLimits {
    pub window: usize,
    pub input_limit: Option<usize>,
    pub required_output: Option<u64>,
    pub output_limit: Option<u64>,
}

pub(super) struct StepInput<'a> {
    pub history: &'a [Arc<Message>],
    pub prompt: &'a Message,
    pub preamble: &'a str,
    pub tools: usize,
}

pub(super) async fn project(
    compactor: &super::step_compaction::StepCompactor,
    source: &dyn super::step_compaction::Summarizer,
    run: &AgentRunContext,
    model: &str,
    input: StepInput<'_>,
    limits: StepLimits,
) -> Result<StepView, String> {
    let StepInput { history, prompt, preamble, tools } = input;
    let StepLimits { window, input_limit, required_output, output_limit } = limits;
    let auto = run.context.workspace_get(&run.session_id)?.auto_compact;
    let system = context::estimate_tokens(preamble);
    let fixed = system + tools + estimate_message(&project_message(prompt));
    let reserve = required_output.unwrap_or(256) as usize;
    let ceiling = window.saturating_sub(reserve).min(input_limit.unwrap_or(usize::MAX));
    let threshold = pressure_threshold(window).min(ceiling);
    // A large preamble can already fill the normal 80% target. Preserve the
    // remaining output reservation and still reduce completed history if needed.
    let target = if fixed >= threshold { ceiling } else { threshold };
    let (mut history, mut used) = fit_history(history, fixed, target, auto);
    if auto && used >= target && fixed < ceiling {
        compactor
            .reduce(source, model, &mut history, &mut used, target, window, &run.tool_context.cancellation)
            .await?;
    }
    run.context.record_request(
        &run.session_id,
        &RequestMeasure {
            observed_input: None,
            model: model.into(),
            head: run.run_id.clone(),
            window,
            system,
            tools,
            messages: used - system - tools,
            threshold,
            auto,
        },
    )?;
    if input_limit.is_some_and(|limit| used > limit) {
        return Err("Input exceeds the destination model limit".into());
    }
    let available = window.saturating_sub(used) as u64;
    let cap = available.min(output_limit.unwrap_or(u64::MAX));
    if required_output.is_some_and(|required| cap < required) {
        return Err("Selected reasoning budget does not fit the destination model".into());
    }
    let output = required_output.unwrap_or_else(|| cap.min(budget::DEFAULT_OUTPUT_CAP));
    if output < 256 {
        return Err("Context budget exhausted; compact the conversation".into());
    }
    Ok(StepView { history, output })
}

/// Price the final wire projection, including schemas and documents added by later hooks.
pub(super) fn wire_counts(request: &rig_core::completion::CompletionRequest) -> Result<(usize, usize, usize), String> {
    let mut system = 0;
    let mut messages = 0;
    for message in &request.chat_history {
        if matches!(message.as_ref(), Message::System { .. }) {
            system += estimate_message(message);
        } else {
            messages += estimate_message(message);
        }
    }
    if !request.documents.is_empty() {
        system += context::estimate_tokens(&serde_json::to_string(&request.documents).map_err(|e| e.to_string())?);
    }
    let tools = context::estimate_tokens(&serde_json::to_string(&request.tools).map_err(|e| e.to_string())?);
    Ok((system, tools, messages))
}

pub(super) fn finalize(
    run: &AgentRunContext,
    request: &mut rig_core::completion::CompletionRequest,
    input_limit: Option<usize>,
    required: Option<u64>,
) -> Result<u64, String> {
    let mut report = run
        .context
        .last_request(&run.session_id)?
        .filter(|r| r.head == run.run_id)
        .ok_or("Missing prepared context measurement")?;
    (report.system, report.tools, report.messages) = wire_counts(request)?;
    let used = report.used();
    let cap = request.max_tokens.unwrap_or(budget::DEFAULT_OUTPUT_CAP).min(report.window.saturating_sub(used) as u64);
    run.context.record_request(&run.session_id, &report)?;
    if input_limit.is_some_and(|limit| used > limit) || cap < required.unwrap_or(256) {
        return Err("The next request does not fit the selected model context".into());
    }
    request.max_tokens = Some(cap);
    Ok(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_tool_results_fit_without_modifying_the_transcript() {
        let history: Vec<_> = (0..8)
            .map(|i| {
                Arc::new(Message::tool_result(
                    rig_core::message::ProviderCallId::new(format!("call-{i}")).unwrap().into(),
                    rig_core::message::ToolName::new("shell").unwrap(),
                    "abc".repeat(1700),
                ))
            })
            .collect();
        let (original, raw) = fit_history(&history, 600, 6000, false);
        let (wire, used) = fit_history(&history, 600, 6000, true);
        assert!(raw > 6000 && used < 6000);
        assert_eq!(history, original);
        assert_eq!(wire.len(), original.len());
        assert_eq!(wire.last(), original.last());
        for (before, after) in original.iter().zip(&wire) {
            if let (Message::User { content: a }, Message::User { content: b }) = (before.as_ref(), after.as_ref()) {
                if let (Some(UserContent::ToolResult(a)), Some(UserContent::ToolResult(b))) = (a.first(), b.first()) {
                    assert_eq!(a.call, b.call);
                }
            }
        }
    }
}
