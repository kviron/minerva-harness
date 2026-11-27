//! One frozen selection binds preparation, Rig selection, and dispatch for a step.
use super::{
    project_message, reasoning,
    routes::{Route, Routes},
    step_context, AgentRunContext, ProviderService, PREAMBLE,
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
    pub compactor: super::step_compaction::StepCompactor,
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
            let plan = if route.provider.protocol == super::codex::ID {
                reasoning::prepare_codex(
                    &route.provider,
                    &route.model,
                    selection.reasoning_effort.as_deref(),
                    route.spec.as_ref().ok_or("Missing Codex model metadata")?,
                    &route.reasoning_levels,
                )?
            } else {
                reasoning::validate_target(&route.provider, &route.model)?;
                reasoning::prepare(&route.provider, &route.model, selection.reasoning_effort.as_deref())?
            };
            let declared = self.source.declared_window(&route.provider, &route.model);
            let prepared =
                self.run.context.prepare_for_model(&self.run.session_id, &self.run.messages, declared).await?;
            let preamble = format!("{}\n{}", PREAMBLE, prepared.system_prompt);
            let context_window = prepared.budget.context_window;
            let input_limit = route.spec.as_ref().and_then(|s| s.input_limit()).map(|n| n as usize);
            let output_limit = route.spec.as_ref().and_then(|s| s.max_output_tokens).map(u64::from);
            let view = step_context::project(
                &self.compactor,
                &self.source,
                &self.run,
                &selection.model,
                step_context::StepInput {
                    history: event.history,
                    prompt: event.prompt,
                    preamble: &preamble,
                    tools: self.schema_tokens,
                },
                step_context::StepLimits {
                    window: context_window,
                    input_limit,
                    required_output: plan.output_cap,
                    output_limit,
                },
            )
            .await?;
            let output_cap = view.output;
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
            Ok(RequestPatch::new().preamble(preamble).history(view.history).max_tokens(output_cap))
        }
        .await;
        match result {
            Ok(patch) => CompletionCallAction::Patch(patch),
            Err(error) => CompletionCallAction::Stop(error),
        }
    }
    async fn on_model_turn_finished(
        &self,
        _: &HookContext,
        event: rig_agent::agent::hook::ModelTurnFinished<'_>,
    ) -> rig_agent::agent::hook::ModelTurnAction {
        use rig_agent::agent::hook::ModelTurnAction;
        match self.run.context.confirm_request(&self.run.session_id, &self.run.run_id, event.usage.input_tokens) {
            Ok(()) => ModelTurnAction::Continue,
            Err(error) => ModelTurnAction::Stop(error),
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
            let mut step = step;
            let mut projected = build_request(request, &step);
            step.output_cap =
                step_context::finalize(&self.run, &mut projected, step.input_limit, step.plan.output_cap)?;
            validate_request(&projected, &step)?;
            *self.step.lock().map_err(|e| e.to_string())? = Some(step);
            Ok(EffectKind::Completion { request: Arc::new(projected), stream: *stream })
        }
        .await;
        match result {
            Ok(effect) => DispatchAction::patch(effect),
            Err(error) => DispatchAction::Deny(ErrorReport::new(ErrorKind::Denied, error)),
        }
    }
}

#[cfg(test)]
fn project_request(request: &CompletionRequest, step: &FrozenStep) -> Result<CompletionRequest, String> {
    let projected = build_request(request, step);
    validate_request(&projected, step)?;
    Ok(projected)
}

fn build_request(request: &CompletionRequest, step: &FrozenStep) -> CompletionRequest {
    let mut projected = request.clone();
    projected.chat_history = request.chat_history.iter().map(|message| Arc::new(project_message(message))).collect();
    projected.model = Some(step.route.model.clone());
    projected.options.reasoning = step.plan.typed;
    projected.additional_params = step.plan.additional_params.clone();
    projected.max_tokens = Some(step.output_cap);
    projected
}

pub(crate) fn validate_request(request: &CompletionRequest, step: &FrozenStep) -> Result<(), String> {
    if let Some(spec) = step.route.spec.as_ref() {
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
    let (system, tools, messages) = step_context::wire_counts(request)?;
    let tokens = system + tools + messages;
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
