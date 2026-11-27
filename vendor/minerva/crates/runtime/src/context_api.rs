//! Context operations shared by the gateway and the native IDE adapter.
use crate::{context::*, Runtime};

impl Runtime {
    pub async fn compact_context(&self, session: &str) -> Result<ContextSummary, String> {
        self.compact_context_operation(session, "manual-compact").await
    }
    pub(crate) fn selected_context_model(&self, session: &str) -> Result<String, String> {
        Ok(self.sessions.selection(session)?.map(|s| s.model).or(self.sessions.get(session)?.model).unwrap_or_default())
    }
    pub async fn compact_context_operation(&self, session: &str, operation: &str) -> Result<ContextSummary, String> {
        let mut guard = self.agent_loop.reserve_compaction(session, operation).await?;
        let selected = self.selected_context_model(session)?;
        if selected.is_empty() {
            return Err("Select a model before compacting history".into());
        }
        let messages = self.current_branch(session)?;
        self.plugins.compact_messages_cancellable(session, &selected, &messages, &mut guard.cancel).await
    }
    pub async fn context_measure(&self, session: &str) -> Result<meter::RequestMeasure, String> {
        let model = self.selected_context_model(session)?;
        let context = &self.plugins.harness.context;
        let settings = context.workspace_get(session)?;
        let declared =
            if model.starts_with("api:") { self.plugins.providers()?.route_window(&model).await } else { None };
        let window = settings.effective_window(declared);
        let messages = self.current_branch(session)?;
        if let Some(report) = context.last_request(session)? {
            if report.model == model
                && report.window == window
                && report.auto == settings.auto_compact
                && (messages.last().is_some_and(|m| m.id == report.head)
                    || self.get_run(session, &report.head).is_ok_and(|r| r.status == "running"))
            {
                return Ok(report);
            }
        }
        let prepared = context.prepare_for_model(session, &messages, declared).await?;
        let tools = self.plugins.current_tools(session)?;
        let schema = serde_json::to_string(&tools.definitions()).map_err(|e| e.to_string())?;
        let system = estimate_tokens(&prepared.system_prompt);
        Ok(meter::RequestMeasure {
            observed_input: None,
            model,
            window,
            head: messages.last().map(|m| m.id.clone()).unwrap_or_default(),
            auto: settings.auto_compact,
            threshold: meter::pressure_threshold(window),
            messages: prepared.budget.estimated_tokens.saturating_sub(system),
            tools: estimate_tokens(&schema),
            system: estimate_tokens(&format!("{}\n{}", crate::providers::PREAMBLE, prepared.system_prompt)),
        })
    }
}
