//! Build and drive one Rig run for direct and live-selected provider routes.
use super::*;

impl ProviderService {
    pub(super) async fn chat_inner(
        &self,
        id: &str,
        model: &str,
        messages: Vec<Turn>,
        sink: crate::transcript::TranscriptSink,
        run: Option<ProviderRunContext>,
        response_cap: Option<u64>,
    ) -> Result<crate::transcript::Transcript, String> {
        let (provider, key) = {
            let providers = self.providers.lock().await;
            let provider = providers.iter().find(|p| p.id == id).cloned().ok_or("Provider not found")?;
            if !provider.models.iter().any(|m| m == model) {
                return Err("Model not found".into());
            }
            self.endpoint_policy.require_safe_transport(&provider.base_url)?;
            let key = if provider.protocol == codex::ID {
                String::new()
            } else {
                self.credentials
                    .get(&self.scope, id)?
                    .or_else(|| self.env_key(id, &provider.base_url))
                    .or_else(|| (provider.protocol == "ollama").then(|| "ollama".into()))
                    .ok_or("API key not configured")?
            };
            (provider, key)
        };
        let codex_snapshot = if provider.protocol == codex::ID {
            Some(self.codex.model_snapshot(model).ok_or("Codex model is unavailable; refresh its catalog")?)
        } else {
            None
        };
        let (last, prev) = messages.split_last().ok_or("Сообщение пустое.")?;
        if last.role != "user" || (last.text().trim().is_empty() && !last.parts.iter().any(|p| p.kind == "image")) {
            return Err("Нужно сообщение пользователя.".into());
        }
        let history = crate::rig_history::replay(prev)?;
        let prompt = crate::rig_history::user_message(last)?;
        let hook = crate::rig_history::HistoryHook::new(sink);
        let mut dynamic_tools = Vec::new();
        let mut step_hook = None;
        let mut identity_hook = crate::tools::AdmittedTools::default().identity_hook();
        let streaming = run.is_some();
        let reasoning_effort = run.as_ref().and_then(|r| r.reasoning_effort.clone());
        let cancellation = run.as_ref().map(|r| r.tool_context.cancellation.clone());
        if let Some(mut run) = run {
            run.tool_context.sink = Some(Arc::new(hook.clone()));
            dynamic_tools = run.tools.rig_tools(run.tool_context.clone());
            identity_hook = run.tools.identity_hook();
            let spec = self.model_spec(&provider, model);
            step_hook = Some(ProviderStepHook {
                compactor: Default::default(),
                source: self.clone(),
                model: format!("api:{id}/{model}"),
                window: self.declared_window(&provider, model),
                input_limit: spec.as_ref().and_then(|s| s.input_limit()).map(|v| v as usize),
                output_limit: spec.as_ref().and_then(|s| s.max_output_tokens).map(u64::from),
                required_output: None,
                followups: followups::StepFollowups::new(run.followups.clone()),
                history: hook.clone(),
                schema_tokens: serde_json::to_string(&run.tools.definitions())
                    .map_err(|e| e.to_string())?
                    .len()
                    .div_ceil(3),
                run,
            });
        }
        let has_tools = !dynamic_tools.is_empty();
        let selection_control = step_hook.as_ref().and_then(|hook| hook.run.selection.clone());
        let routes = if selection_control.is_some() { Some(self.run_routes(&provider.id).await?) } else { None };
        let step: selection_hook::StepState = Default::default();
        let routing_hook = match (step_hook.take(), routes.as_ref()) {
            (Some(hook), Some(routes)) => Some(selection_hook::SelectionHook {
                run: hook.run,
                compactor: hook.compactor,
                source: self.clone(),
                routes: routes.clone(),
                step: step.clone(),
                schema_tokens: hook.schema_tokens,
            }),
            (hook, _) => {
                step_hook = hook;
                None
            }
        };
        let reasoning = if selection_control.is_some() {
            reasoning::Plan::default()
        } else {
            match &codex_snapshot {
                Some((spec, levels)) => {
                    reasoning::prepare_codex(&provider, model, reasoning_effort.as_deref(), spec, levels)?
                }
                None => reasoning::prepare(&provider, model, reasoning_effort.as_deref())?,
            }
        };
        let budget_cap = response_cap.or(reasoning.output_cap);
        if let Some(hook) = &mut step_hook {
            hook.required_output = budget_cap;
        }
        macro_rules! run_agent {
            ($model:expr) => {{
                let builder = AgentBuilder::new($model).preamble(PREAMBLE).dynamic_tools(dynamic_tools);
                let builder = if let Some(cap) = budget_cap {
                    builder.max_tokens(cap)
                } else if provider.protocol == "anthropic-messages" || (streaming && !has_tools) {
                    builder.max_tokens(budget::DEFAULT_OUTPUT_CAP)
                } else {
                    builder
                };
                let builder = match reasoning.typed {
                    Some(reasoning) => builder.reasoning(reasoning),
                    None => builder,
                };
                let builder = match &reasoning.additional_params {
                    Some(params) => builder.additional_params(params.clone()),
                    None => builder,
                };
                let mut builder = builder;
                let signed_route = provider.protocol == codex::ID && selection_control.is_none();
                if signed_route {
                    builder = builder.model_route_handler(
                        "minerva-codex",
                        self.codex.handler_with_spec(
                            codex_snapshot.as_ref().ok_or("Missing Codex model metadata")?.0.clone(),
                        )?,
                    );
                }
                if let (Some(routes), Some(control), Some(routing)) = (&routes, &selection_control, &routing_hook) {
                    for route in routes.values() {
                        builder = builder.model_route_handler(
                            route.label.clone(),
                            route_handler::RouteHandler {
                                inner: route.handler.clone(),
                                label: route.label.clone(),
                                source: self.clone(),
                                step: step.clone(),
                                control: control.clone(),
                                history: hook.clone(),
                                run_id: routing.run.run_id.clone(),
                                cancellation: routing.run.tool_context.cancellation.clone(),
                            },
                        );
                    }
                }
                let agent = builder.build();
                let mut request = agent.prompt(prompt).history(history).add_hook(hook.clone());
                if signed_route {
                    request = request.using_model("minerva-codex");
                }
                if let Some(step_hook) = step_hook {
                    request = request.add_hook(step_hook);
                }
                let request =
                    if let Some(routing_hook) = routing_hook { request.add_hook(routing_hook) } else { request };
                let timeout = timeout::ModelTimeout::new();
                let request = request.add_hook(identity_hook).add_hook(timeout.clone());
                let request = if streaming { request.max_turns(if has_tools { 32 } else { 1 }) } else { request };
                if !streaming {
                    let response =
                        timeout.run(async { request.await.map_err(|e| format!("Provider run failed: {e}")) }).await?;
                    return hook.finish(response.messages.get(1..).ok_or("Rig returned no response history")?);
                }
                let mut stream = request.stream();
                let drive = async {
                    use futures_util::StreamExt;
                    while let Some(item) = stream.next().await {
                        match item.map_err(|e| format!("Provider run failed: {e}"))? {
                            rig_agent::agent::MultiTurnStreamItem::FinalResponse(response) => {
                                return hook
                                    .finish(response.messages.get(1..).ok_or("Rig returned no response history")?);
                            }
                            _ => {}
                        }
                    }
                    Err("Provider stream ended without a final response".into())
                };
                let bounded = timeout.run(drive);
                if let Some(cancellation) = cancellation {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => Err("Run cancelled".into()),
                        result = bounded => result,
                    }
                } else {
                    bounded.await
                }
            }};
        }
        match provider.protocol.as_str() {
            "openai-codex" => {
                run_agent!(openai_config(codex::ID, codex::BASE_URL, String::new()).client().responses(model))
            }
            "openai-chat" | "ollama" => {
                run_agent!(openai_config(&provider.id, &provider.base_url, key).client().chat(model))
            }
            "openai-responses" => {
                run_agent!(openai_config(&provider.id, &provider.base_url, key).client().responses(model))
            }
            "anthropic-messages" => {
                run_agent!(AnthropicConfig::new(key).with_base_url(provider.base_url).client().completion(model))
            }
            _ => Err("Неизвестный протокол API.".into()),
        }
    }
}
