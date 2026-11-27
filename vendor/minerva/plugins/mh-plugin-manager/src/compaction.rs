//! Durable conversation summaries, including incremental updates and cancellation.
use super::plugins::PluginManager;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

impl PluginManager {
    pub async fn compact_messages(
        &self,
        session: &str,
        model: &str,
        messages: &[crate::sessions::SessionMessage],
    ) -> Result<crate::context::ContextSummary, String> {
        let (_sender, mut cancel) = tokio::sync::watch::channel(false);
        self.compact_messages_cancellable(session, model, messages, &mut cancel).await
    }
    pub async fn compact_messages_cancellable(
        &self,
        session: &str,
        model: &str,
        messages: &[crate::sessions::SessionMessage],
        cancel: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<crate::context::ContextSummary, String> {
        if *cancel.borrow() {
            return Err("Run cancelled".into());
        }
        let declared = if model.starts_with("api:") { self.providers()?.route_window(model).await } else { None };
        let window = self.harness.context.workspace_get(session)?.effective_window(declared);
        let previous = self.harness.context.anchored_summary(session, messages)?;
        let start = previous.as_ref().map_or(0, |summary| summary.prefix_message_ids.len());
        let additional = crate::context::summary_prefix(&messages[start..], window);
        if additional.is_empty() {
            return previous.ok_or_else(|| "There are not enough complete turns to compact".into());
        }
        let prefix = &messages[..start + additional.len()];
        let mut summary = previous.map(|summary| summary.text).unwrap_or_default();
        let max_summary_bytes = (window / 2).min(8192);
        summary.truncate(summary.floor_char_boundary(max_summary_bytes.min(summary.len())));
        let chunks = super::summary_input::chunks(additional, window)?;
        for chunk in chunks {
            let content = format!("Previous summary:\n{summary}\nConversation data:\n{chunk}");
            summary = if let Some(route) = model.strip_prefix("api:") {
                let (provider, model) = route.split_once('/').ok_or("Invalid model route")?;
                let providers = self.providers()?;
                tokio::select! {
                    biased;
                    _=cancel.changed()=>return Err("Run cancelled".into()),
                    summary=providers.summarize(provider, model, &content, window)=>summary?,
                }
            } else {
                static NEXT: AtomicU64 = AtomicU64::new(1);
                let run = format!("summary:{session}:{}", NEXT.fetch_add(1, Ordering::SeqCst));
                let prompt=format!("Summarize this earlier conversation for continuation. Preserve decisions, changed files, tool outcomes and open tasks. Do not execute tools. Return only the summary.\n{content}");
                crate::engine::run(
                    self,
                    &run,
                    model,
                    vec![crate::sessions::Turn::plain("user", &prompt)],
                    None,
                    Arc::new(|_| Ok(())),
                    cancel,
                )
                .await?
                .0
                .text()
            };
            if summary.trim().is_empty() {
                return Err("Context summary was empty".into());
            }
            summary.truncate(summary.floor_char_boundary(max_summary_bytes.min(summary.len())));
        }
        if *cancel.borrow() {
            return Err("Run cancelled".into());
        }
        self.harness.context.save_summary(session, prefix, &summary)
    }
}
