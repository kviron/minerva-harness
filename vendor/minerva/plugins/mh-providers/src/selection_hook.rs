//! One frozen selection binds preparation, Rig selection, and dispatch for a step.
use super::{
    catalog_spec, estimate_message, project_message, reasoning,
    routes::{Route, Routes},
    AgentRunContext, ProviderService, PREAMBLE,
};
use mh_sessions::RouteSelection;
use rig_agent::agent::hook::{
    AgentHook, CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent, HookContext, ModelSelection,
    ModelSelectionAction, RequestPatch,
};
use rig_core::{
    completion::CompletionRequest,
    effect::EffectKind,
    error::{ErrorKind, ErrorReport},
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct FrozenStep {
    pub selection: RouteSelection,
    pub route: Arc<Route>,
    pub turn: u64,
    pub plan: reasoning::Plan,
    pub context_window: usize,
    pub input_limit: Option<usize>,
    pub output_cap: u64,
}
pub(crate) type StepState = Arc<Mutex<Option<FrozenStep>>>;

pub(crate) struct SelectionHook {
    pub run: AgentRunContext,
    pub source: ProviderService,
    pub routes: Routes,
    pub step: StepState,
    pub schema_tokens: usize,
}
impl SelectionHook {
    fn stopped(&self) -> bool {
        self.run.tool_context.cancellation.is_cancelled()
    }
    fn frozen(&self) -> Result<FrozenStep, String> {
        self.step.lock().map_err(|e| e.to_string())?.clone().ok_or_else(|| "No route was prepared for this step".into())
    }
}
impl AgentHook for SelectionHook {
    async fn on_completion_call(&self, _: &HookContext, event: CompletionCallEvent<'_>) -> CompletionCallAction {
        let result: Result<RequestPatch, String> = async {
            if self.stopped() {
                return Err("Run cancelled".into());
            }
            let selection = self.run.selection.as_ref().ok_or("Missing selection coordinator")?.selection().await?;
            let route = self
                .routes
                .get(&selection.model)
                .cloned()
                .ok_or("This route was not registered at run start; start a new turn")?;
            self.source.validate_route(&route).await?;
            reasoning::validate_target(&route.provider, &route.model)?;
            let plan = reasoning::prepare(&route.provider, &route.model, selection.reasoning_effort.as_deref())?;
            let prepared = self.run.context.prepare_enriched(&self.run.session_id, &self.run.messages).await?;
            let history = event.history.iter().map(|message| Arc::new(project_message(message))).collect::<Vec<_>>();
            let preamble = format!("{}\n{}", PREAMBLE, prepared.system_prompt);
            let estimated = history
                .iter()
                .map(|m| estimate_message(m))
                .sum::<usize>()
                .saturating_add(estimate_message(&project_message(event.prompt)))
                .saturating_add(crate::context::estimate_tokens(&preamble))
                .saturating_add(self.schema_tokens);
            let spec = catalog_spec(&route.provider, &route.model);
            let context_window = spec
                .as_ref()
                .and_then(|spec| spec.context_window)
                .map(|v| v as usize)
                .map_or(prepared.budget.context_window, |limit| limit.min(prepared.budget.context_window));
            let input_limit = spec.as_ref().and_then(|spec| spec.input_limit()).map(|limit| limit as usize);
            if input_limit.is_some_and(|limit| estimated > limit) {
                return Err("Input exceeds the destination model limit".into());
            }
            let available = context_window.saturating_sub(estimated);
            let output_limit = spec.as_ref().and_then(|spec| spec.max_output_tokens).map(u64::from).unwrap_or(u64::MAX);
            let output_cap = if let Some(required) = plan.output_cap {
                if required > available as u64 || required > output_limit {
                    return Err("Selected reasoning budget does not fit the destination model".into());
                }
                required
            } else {
                (available as u64).min(super::budget::DEFAULT_OUTPUT_CAP).min(output_limit)
            };
            if output_cap < 256 {
                return Err("Context budget exhausted; compact the conversation".into());
            }
            if self.stopped() {
                return Err("Run cancelled".into());
            }
            *self.step.lock().map_err(|e| e.to_string())? = Some(FrozenStep {
                selection,
                route,
                turn: event.turn as u64,
                plan,
                context_window,
                input_limit,
                output_cap,
            });
            Ok(RequestPatch::new().preamble(preamble).history(history).max_tokens(output_cap))
        }
        .await;
        match result {
            Ok(patch) => CompletionCallAction::Patch(patch),
            Err(error) => CompletionCallAction::Stop(error),
        }
    }
    fn on_model_select(&self, _: &HookContext, _: ModelSelection<'_>) -> ModelSelectionAction {
        if self.stopped() {
            return ModelSelectionAction::Stop("Run cancelled".into());
        }
        match self.frozen() {
            Ok(step) => ModelSelectionAction::select(step.route.label.clone()),
            Err(error) => ModelSelectionAction::Stop(error),
        }
    }
    async fn on_dispatch(&self, _: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        let EffectKind::Completion { request, stream } = event.kind else { return DispatchAction::proceed() };
        let result: Result<EffectKind, String> = async {
            if self.stopped() {
                return Err("Run cancelled".into());
            }
            let step = self.frozen()?;
            if step.turn != event.turn as u64 {
                return Err("Route selection belongs to another step".into());
            }
            self.source.validate_route(&step.route).await?;
            let projected = project_request(request, &step)?;
            Ok(EffectKind::Completion { request: Arc::new(projected), stream: *stream })
        }
        .await;
        match result {
            Ok(effect) => DispatchAction::patch(effect),
            Err(error) => DispatchAction::Deny(ErrorReport::new(ErrorKind::Denied, error)),
        }
    }
}

fn project_request(request: &CompletionRequest, step: &FrozenStep) -> Result<CompletionRequest, String> {
    let mut projected = request.clone();
    projected.chat_history = request.chat_history.iter().map(|message| Arc::new(project_message(message))).collect();
    projected.model = Some(step.route.model.clone());
    projected.options.reasoning = step.plan.typed;
    projected.additional_params = step.plan.additional_params.clone();
    projected.max_tokens = Some(step.output_cap);
    validate_request(&projected, step)?;
    Ok(projected)
}

pub(crate) fn validate_request(request: &CompletionRequest, step: &FrozenStep) -> Result<(), String> {
    if let Some(spec) = catalog_spec(&step.route.provider, &step.route.model) {
        if !spec.tools && !request.tools.is_empty() {
            return Err("The selected model does not accept tools".into());
        }
        let has_images = request.chat_history.iter().any(|message| {
            let mut message = (**message).clone();
            let has_images = message.images_mut().next().is_some();
            has_images
        });
        if has_images && !spec.input.image {
            return Err("The selected model does not accept images".into());
        }
    }
    check_budget(request, step)
}

fn check_budget(request: &CompletionRequest, step: &FrozenStep) -> Result<(), String> {
    let tokens = request
        .chat_history
        .iter()
        .map(|m| estimate_message(m))
        .sum::<usize>()
        .saturating_add(serde_json::to_string(&request.tools).map_err(|e| e.to_string())?.len().div_ceil(3));
    let tokens =
        tokens.saturating_add(serde_json::to_string(&request.documents).map_err(|e| e.to_string())?.len().div_ceil(3));
    if step.input_limit.is_some_and(|limit| tokens > limit) {
        return Err("Input exceeds the destination model limit".into());
    }
    if tokens.saturating_add(step.output_cap as usize) > step.context_window {
        return Err("The next request does not fit the selected model context".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "selection_hook_tests.rs"]
mod tests;
