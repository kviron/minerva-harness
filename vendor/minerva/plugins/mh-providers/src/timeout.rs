//! Bound an inactive model request without timing host tools or user approvals.
use rig_agent::agent::hook::{
    AgentHook, DispatchAction, DispatchEvent, HookContext, ModelTurnAction, ModelTurnFinished, ObservationAction,
    ReasoningDelta, TextDelta, ToolCallDelta,
};
use rig_core::effect::EffectKind;
use std::{future::Future, time::Duration};
use tokio::{sync::watch, time::Instant};

const INACTIVITY: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub(super) struct ModelTimeout(watch::Sender<Option<Instant>>);
impl ModelTimeout {
    pub fn new() -> Self {
        Self(watch::channel(None).0)
    }
    fn start(&self) {
        self.0.send_replace(Some(Instant::now() + INACTIVITY));
    }
    fn progress(&self) {
        self.0.send_if_modified(|deadline| {
            if deadline.is_some() {
                *deadline = Some(Instant::now() + INACTIVITY);
                true
            } else {
                false
            }
        });
    }
    fn finish(&self) {
        self.0.send_replace(None);
    }
    async fn expired(&self) {
        let mut deadline = self.0.subscribe();
        loop {
            let current = *deadline.borrow_and_update();
            if let Some(current) = current {
                tokio::select! {
                    biased;
                    _ = deadline.changed() => {},
                    _ = tokio::time::sleep_until(current) => {
                        if deadline.borrow().is_some_and(|latest| latest <= Instant::now()) {
                            return;
                        }
                    }
                }
            } else {
                let _ = deadline.changed().await;
            }
        }
    }
    pub async fn run<T>(&self, work: impl Future<Output = Result<T, String>>) -> Result<T, String> {
        tokio::select! {
            biased;
            result = work => result,
            _ = self.expired() => Err("Provider timed out after 300 seconds without model output".into()),
        }
    }
}
impl AgentHook for ModelTimeout {
    async fn on_dispatch(&self, _: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        if matches!(event.kind, EffectKind::Completion { .. }) {
            self.start();
        }
        DispatchAction::proceed()
    }
    async fn on_text_delta(&self, _: &HookContext, _: TextDelta<'_>) -> ObservationAction {
        self.progress();
        ObservationAction::Continue
    }
    async fn on_reasoning_delta(&self, _: &HookContext, _: ReasoningDelta<'_>) -> ObservationAction {
        self.progress();
        ObservationAction::Continue
    }
    async fn on_tool_call_delta(&self, _: &HookContext, _: ToolCallDelta<'_>) -> ObservationAction {
        self.progress();
        ObservationAction::Continue
    }
    async fn on_model_turn_finished(&self, _: &HookContext, _: ModelTurnFinished<'_>) -> ModelTurnAction {
        self.finish();
        ModelTurnAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn healthy_output_can_continue_beyond_the_old_whole_turn_deadline() {
        let timeout = ModelTimeout::new();
        let began = Instant::now();
        timeout
            .run(async {
                timeout.start();
                for _ in 0..8 {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    timeout.progress();
                }
                timeout.finish();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(began.elapsed(), Duration::from_secs(480));
    }

    #[tokio::test(start_paused = true)]
    async fn silent_model_is_bounded_by_its_last_progress() {
        let timeout = ModelTimeout::new();
        let began = Instant::now();
        let result = timeout
            .run(async {
                timeout.start();
                tokio::time::sleep(Duration::from_secs(100)).await;
                timeout.progress();
                std::future::pending::<Result<(), String>>().await
            })
            .await;
        assert!(result.unwrap_err().contains("300 seconds"));
        assert_eq!(began.elapsed(), Duration::from_secs(400));
    }

    #[tokio::test(start_paused = true)]
    async fn tool_execution_and_approval_wait_do_not_consume_the_model_deadline() {
        let timeout = ModelTimeout::new();
        let began = Instant::now();
        timeout
            .run(async {
                timeout.start();
                tokio::time::sleep(Duration::from_secs(10)).await;
                timeout.finish();
                tokio::time::sleep(Duration::from_secs(900)).await;
                timeout.start();
                tokio::time::sleep(Duration::from_secs(10)).await;
                timeout.finish();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(began.elapsed(), Duration::from_secs(920));
    }
}
