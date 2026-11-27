//! Translate the native context service into the IDE agent protocol.
use super::*;
use ide_minerva::minerva_runtime::context::meter::RequestMeasure;

impl Engine {
    pub(super) async fn run_compact(
        &self,
        runtime: &Runtime,
        session: &str,
        active: &ActiveRun,
    ) -> Result<Value, String> {
        if active.cancelled.load(Ordering::Acquire) {
            return Ok(json!({"stopReason":"cancelled"}));
        }
        let result = runtime.compact_context_operation(session, &active.id).await;
        if active.cancelled.load(Ordering::Acquire) {
            return Ok(json!({"stopReason":"cancelled"}));
        }
        let summary = result?;
        self.publish_context(runtime, session).await;
        self.update(
            session,
            json!({"sessionUpdate":"agent_message_chunk","content":{
                "type":"text","text":format!("Compacted {} conversation messages.",summary.prefix_message_ids.len())
            }}),
        )?;
        Ok(json!({"stopReason":"end_turn"}))
    }
    pub(super) fn usage_update(&self, session: &str, report: &RequestMeasure) -> Result<(), String> {
        self.update(session, json!({"sessionUpdate":"usage_update", "used": report.used(), "size": report.window}))
    }
    pub(super) async fn publish_context(&self, runtime: &Runtime, session: &str) {
        match runtime.context_measure(session).await {
            Ok(report) if !report.model.is_empty() => {
                if let Err(error) = self.usage_update(session, &report) {
                    tracing::debug!(%error, "Context meter disconnected");
                }
            }
            Err(error) => tracing::debug!(%error, "Context estimate unavailable"),
            _ => {}
        }
    }
    pub(super) fn compact_command(&self, session: &str) -> Result<(), String> {
        self.update(
            session,
            json!({"sessionUpdate":"available_commands_update","availableCommands":[
                {"name":"compact","description":"Compact the conversation"}
            ]}),
        )
    }
    pub(super) async fn context_details(&self, runtime: &Runtime, id: &str) -> Result<Value, String> {
        let report = runtime.context_measure(id).await?;
        Ok(json!({"model":report.model, "totalTokens":report.used(), "maxTokens":report.window,
            "isAutoCompactEnabled":report.auto, "autoCompactThreshold":report.threshold,
            "categories":[{"name":"System prompt","kind":"estimate","tokens":report.system},{"name":"System tools","kind":"estimate","tokens":report.tools},
                {"name":"Messages","kind":"estimate","tokens":report.messages}, {"name":"Free space","tokens":report.window.saturating_sub(report.used())}]}))
    }
}
