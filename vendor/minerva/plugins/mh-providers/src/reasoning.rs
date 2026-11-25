//! Reasoning choices are admitted by the same wire that sends them.
use super::{catalog_spec, openai_config, Provider};
use rig_core::{
    completion::{CompletionRequest, Reasoning},
    operation::Completion,
    wire::{Body, Encoded, Mode, Operation, Wire},
};
use serde_json::{json, Value};

#[derive(Clone, Default)]
pub(super) struct Plan {
    pub typed: Option<Reasoning>,
    pub additional_params: Option<Value>,
    pub output_cap: Option<u64>,
}

pub(super) fn prepare_codex(
    provider: &Provider,
    model: &str,
    choice: Option<&str>,
    spec: &rig_core::catalog::ModelSpec,
    levels: &[String],
) -> Result<Plan, String> {
    validate_protocol(provider, spec)?;
    let Some(choice) = choice.filter(|choice| *choice != "default") else { return Ok(Plan::default()) };
    let effort = if choice == "off" { "none" } else { choice };
    if !levels.iter().any(|level| level == choice || level == effort) {
        return Err("The Codex model does not support this reasoning effort".into());
    }
    let plan = Plan { additional_params: Some(json!({"reasoning":{"effort":effort}})), ..Plan::default() };
    if encoded(provider, model, &plan)?["reasoning"]["effort"] != effort {
        return Err("The Codex route did not encode the selected reasoning effort".into());
    }
    Ok(plan)
}

pub(super) fn codex_choices(
    provider: &Provider,
    model: &str,
    spec: &rig_core::catalog::ModelSpec,
    levels: &[String],
) -> Vec<String> {
    let mut choices = vec!["default".into()];
    for level in levels {
        let choice = if level == "none" { "off" } else { level.as_str() };
        if !choices.iter().any(|value| value == choice)
            && prepare_codex(provider, model, Some(choice), spec, levels).is_ok()
        {
            choices.push(choice.to_owned());
        }
    }
    if choices.len() > 1 {
        choices
    } else {
        Vec::new()
    }
}

pub(super) fn prepare(provider: &Provider, model: &str, choice: Option<&str>) -> Result<Plan, String> {
    if matches!(choice, None | Some("default")) {
        return Ok(Plan::default());
    }
    let spec = catalog_spec(provider, model).ok_or("Reasoning controls are unavailable for this API route")?;
    validate_protocol(provider, spec)?;
    let gateway = super::route_preset(provider).is_some_and(|preset| matches!(preset.id, "opencode" | "opencode-go"));
    let typed = spec.reasoning.named(choice.unwrap()).map_err(|error| error.to_string())?.reasoning;
    let mut plan = Plan { typed, additional_params: None, output_cap: None };
    if let Some(Reasoning::Budget { tokens }) = plan.typed {
        plan.output_cap = Some(u64::from(tokens) + super::budget::THINKING_OUTPUT_RESERVE);
    }

    if gateway {
        if provider.protocol != "openai-chat" || !model.starts_with("deepseek-v4") {
            return Err("Reasoning serialization is not verified for this gateway model and protocol".into());
        }
        let effort = match plan.typed {
            Some(Reasoning::Effort(effort)) => Some(effort.as_str()),
            Some(Reasoning::Off) => Some("none"),
            Some(Reasoning::Budget { .. }) => {
                return Err("This gateway protocol takes effort levels, not a token budget".into())
            }
            _ => None,
        };
        if let Some(effort) = effort {
            plan.additional_params = Some(json!({"reasoning_effort":effort}));
            plan.typed = None;
        }
    }
    let actual = encoded(provider, model, &plan)?;
    let baseline = encoded(provider, model, &Plan { output_cap: plan.output_cap, ..Plan::default() })?;
    if actual == baseline {
        return Err("The API route does not encode the selected reasoning setting".into());
    }
    Ok(plan)
}

pub(super) fn validate_target(provider: &Provider, model: &str) -> Result<(), String> {
    if let Some(spec) = catalog_spec(provider, model) {
        validate_protocol(provider, spec)?;
    }
    Ok(())
}

fn validate_protocol(provider: &Provider, spec: &rig_core::catalog::ModelSpec) -> Result<(), String> {
    let expected = match provider.protocol.as_str() {
        "anthropic-messages" => rig_core::providers::registry::Format::Anthropic,
        _ => rig_core::providers::registry::Format::OpenAi,
    };
    if spec.format() != Some(expected) {
        return Err("This model uses a different API protocol on the selected provider".into());
    }
    let gateway = super::route_preset(provider).is_some_and(|preset| matches!(preset.id, "opencode" | "opencode-go"));
    if gateway {
        if let Some(details) = spec.details.provider.as_ref() {
            let responses = matches!(
                (details.npm.as_deref(), details.shape.as_deref()),
                (_, Some("responses")) | (Some("@ai-sdk/openai"), None)
            );
            let chat = matches!(
                (details.npm.as_deref(), details.shape.as_deref()),
                (_, Some("completions")) | (Some("@ai-sdk/openai-compatible"), None)
            );
            if (provider.protocol == "openai-chat" && responses) || (provider.protocol == "openai-responses" && chat) {
                return Err("This model uses a different endpoint on the selected provider".into());
            }
        }
    }
    Ok(())
}

pub(super) fn choices(provider: &Provider, model: &str) -> Vec<String> {
    let Some(spec) = catalog_spec(provider, model) else { return Vec::new() };
    let choices: Vec<_> = spec
        .reasoning
        .choices()
        .iter()
        .filter(|choice| prepare(provider, model, Some(choice.name)).is_ok())
        .map(|choice| choice.name.to_owned())
        .collect();
    if choices.len() > 1 {
        choices
    } else {
        Vec::new()
    }
}

pub(super) fn encoded(provider: &Provider, model: &str, plan: &Plan) -> Result<Value, String> {
    let mut request = CompletionRequest::new("Reasoning capability check")
        .max_tokens(plan.output_cap.unwrap_or(super::budget::DEFAULT_OUTPUT_CAP));
    if let Some(reasoning) = plan.typed {
        request = request.reasoning(reasoning);
    }
    if let Some(params) = &plan.additional_params {
        request = request.additional_params(params.clone());
    }
    let config = openai_config(&provider.id, &provider.base_url, "capability-check".into());
    match provider.protocol.as_str() {
        "openai-chat" | "ollama" => body(&rig_core::providers::openai::wire::Chat::new(config, model), request),
        "openai-responses" | "openai-codex" => {
            body(&rig_core::providers::openai::responses_api::wire::Responses::new(config, model), request)
        }
        "anthropic-messages" => body(
            &rig_core::providers::anthropic::AnthropicConfig::new("capability-check")
                .with_base_url(&provider.base_url)
                .completion(model),
            request,
        ),
        _ => Err("Unknown API protocol".into()),
    }
}

fn body<W: Wire<Op = Completion, Payload = Encoded>>(wire: &W, request: CompletionRequest) -> Result<Value, String> {
    let request = Completion::prepare(request, &wire.describe()).map_err(|error| error.to_string())?;
    let encoded = wire.encode(request, Mode::Unary).map_err(|error| error.to_string())?;
    match encoded.request.body() {
        Body::Bytes(bytes) => serde_json::from_slice(bytes).map_err(|error| error.to_string()),
        _ => Err("The reasoning request is not JSON".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_only_exposes_choices_its_exact_wire_encodes() {
        let catalog = rig_core::catalog::Catalog::builtin();
        for preset in super::super::PRESETS {
            let provider = Provider {
                id: format!("fixture-{}", preset.id),
                name: preset.name.into(),
                base_url: preset.base_url.into(),
                models: Vec::new(),
                protocol: preset.protocol.into(),
            };
            let vendor = if preset.id == "zai-coding" { "zai" } else { preset.id };
            let mut models = 0;
            let mut admitted = 0;
            for protocol in ["openai-chat", "openai-responses", "anthropic-messages", "ollama"] {
                let provider = Provider { protocol: protocol.into(), ..provider.clone() };
                for spec in catalog.iter().filter(|spec| spec.provider.vendor() == vendor) {
                    models += 1;
                    for choice in choices(&provider, &spec.id) {
                        let plan = prepare(&provider, &spec.id, Some(&choice))
                            .unwrap_or_else(|error| panic!("{}/{}/{}: {error}", preset.id, spec.id, choice));
                        if choice != "default" {
                            admitted += 1;
                            assert_ne!(
                                encoded(&provider, &spec.id, &plan).unwrap(),
                                encoded(&provider, &spec.id, &Plan { output_cap: plan.output_cap, ..Plan::default() })
                                    .unwrap(),
                                "{}/{}/{} was omitted",
                                preset.id,
                                spec.id,
                                choice
                            );
                        }
                    }
                }
            }
            eprintln!("{}: {} catalog model/protocol pairs, {} encoded explicit choices", preset.id, models, admitted);
        }
    }

    #[test]
    fn native_controls_are_nonempty_and_unverified_routes_reject_explicit_values() {
        let catalog = rig_core::catalog::Catalog::builtin();
        for id in ["openai", "anthropic", "deepseek", "zai-coding"] {
            let preset = super::super::PRESETS.iter().find(|preset| preset.id == id).unwrap();
            let provider = Provider {
                id: "renamed-provider".into(),
                name: "Fixture".into(),
                base_url: preset.base_url.into(),
                protocol: preset.protocol.into(),
                models: Vec::new(),
            };
            let vendor = if id == "zai-coding" { "zai" } else { id };
            assert!(
                catalog
                    .iter()
                    .filter(|spec| spec.provider.vendor() == vendor)
                    .any(|spec| !choices(&provider, &spec.id).is_empty()),
                "{id} lost every control"
            );
            assert!(choices(&provider, "not-a-known-model").is_empty());
            assert!(prepare(&provider, "not-a-known-model", Some("low")).is_err());
            assert!(
                prepare(&provider, preset.models.first().copied().unwrap_or("unknown"), Some("not-an-effort")).is_err()
            );
        }
        let preset = super::super::PRESETS.iter().find(|preset| preset.id == "opencode-go").unwrap();
        let mut provider = Provider {
            id: "renamed-gateway".into(),
            name: "Fixture".into(),
            base_url: preset.base_url.into(),
            protocol: preset.protocol.into(),
            models: Vec::new(),
        };
        assert!(choices(&provider, "claude-sonnet-4-6").is_empty());
        assert!(prepare(&provider, "claude-sonnet-4-6", Some("high")).is_err());
        provider.protocol = "openai-responses".into();
        assert!(choices(&provider, "deepseek-v4.1-flash").is_empty());
        assert!(prepare(&provider, "deepseek-v4.1-flash", Some("low")).is_err());
    }

    #[test]
    fn untrusted_lookalike_or_edited_routes_do_not_inherit_vendor_controls() {
        for base in
            ["https://evil.test/path/deepseek", "https://api.deepseek.com.evil.test", "http://127.0.0.1:12345/v1"]
        {
            let provider = Provider {
                id: "deepseek".into(),
                name: "Custom".into(),
                base_url: base.into(),
                models: vec!["deepseek-v4.1-flash".into()],
                protocol: "openai-chat".into(),
            };
            assert!(choices(&provider, "deepseek-v4.1-flash").is_empty());
            assert!(prepare(&provider, "deepseek-v4.1-flash", Some("low")).is_err());
            assert!(prepare(&provider, "deepseek-v4.1-flash", Some("default")).is_ok());
        }
    }

    #[test]
    fn coding_preset_uses_zai_grammar_at_the_coding_endpoint() {
        let config = super::super::openai_config("zai-coding", "https://api.z.ai/api/coding/paas/v4", "fixture".into());
        assert_eq!(config.dialect.name, rig_core::providers::openai::wire::ZAI.name);
        assert_eq!(config.base_url, "https://api.z.ai/api/coding/paas/v4");
    }
}
