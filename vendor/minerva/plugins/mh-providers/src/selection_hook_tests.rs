use super::*;
use crate::{
    credential_store::{CredentialScope, MemoryCredentialStore},
    network_policy::EndpointPolicy,
    ProviderInput,
};
use rig_core::{
    operation::Completion,
    wire::{Body, Mode, Operation, Wire},
};
use serde_json::{json, Value};

async fn step(preset_id: &str, model: &str, effort: Option<&str>) -> FrozenStep {
    let preset = crate::PRESETS.iter().find(|preset| preset.id == preset_id).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let service = ProviderService::new_with_store(
        dir.path().join("providers.json"),
        CredentialScope::new("frozen-wire-fixture").unwrap(),
        Arc::new(MemoryCredentialStore::default()),
        EndpointPolicy::Local,
    )
    .unwrap();
    service
        .save(ProviderInput {
            credit_grants: None,
            model_windows: None,
            id: "renamed-fixture".into(),
            name: "Fixture".into(),
            base_url: preset.base_url.into(),
            models: vec![model.into()],
            api_key: Some("fixture-not-used".into()),
            is_new: true,
            protocol: preset.protocol.into(),
        })
        .await
        .unwrap();
    let label = format!("api:renamed-fixture/{model}");
    let route = service.run_routes("renamed-fixture").await.unwrap()[&label].clone();
    let plan = reasoning::prepare(&route.provider, model, effort).unwrap();
    FrozenStep {
        selection: RouteSelection { revision: 7, model: label, reasoning_effort: effort.map(str::to_owned) },
        route,
        turn: 2,
        output_cap: plan.output_cap.unwrap_or(4096),
        plan,
        context_window: 32768,
        input_limit: None,
    }
}
fn body(step: &FrozenStep, request: CompletionRequest) -> Value {
    let config = crate::openai_config(&step.route.provider.id, &step.route.provider.base_url, "fixture".into());
    let wire = rig_core::providers::openai::wire::Chat::new(config, &step.route.model);
    let request = Completion::prepare(request, &wire.describe()).unwrap();
    let encoded = wire.encode(request, Mode::Unary).unwrap();
    let Body::Bytes(bytes) = encoded.request.body() else { panic!("expected JSON") };
    serde_json::from_slice(bytes).unwrap()
}
#[tokio::test]
async fn frozen_gateway_effort_replaces_old_typed_and_raw_choices_in_actual_wire() {
    for effort in ["low", "high", "max", "default"] {
        let step = step("opencode-go", "deepseek-v4.1-flash", Some(effort)).await;
        let stale = CompletionRequest::new("inspect")
            .reasoning(rig_core::completion::Reasoning::Off)
            .additional_params(json!({"reasoning_effort":"stale"}));
        let request = project_request(&stale, &step).unwrap();
        let body = body(&step, request);
        assert_eq!(body["model"], "deepseek-v4.1-flash");
        if effort == "default" {
            assert!(body.get("reasoning_effort").is_none());
        } else {
            assert_eq!(body["reasoning_effort"], effort);
        }
        assert!(body.get("thinking").is_none());
    }
}
#[tokio::test]
async fn frozen_native_off_and_low_keep_their_distinct_thinking_forms() {
    for (effort, thinking) in [("off", "disabled"), ("low", "enabled")] {
        let step = step("deepseek", "deepseek-v4-flash", Some(effort)).await;
        let request = project_request(&CompletionRequest::new("inspect"), &step).unwrap();
        assert_eq!(body(&step, request)["thinking"]["type"], thinking);
    }
}
#[tokio::test]
async fn final_budget_and_input_limit_reject_the_complete_request() {
    let mut step = step("deepseek", "deepseek-v4-flash", Some("default")).await;
    let request = CompletionRequest::new("inspect").preamble("system instructions");
    step.input_limit = Some(1);
    assert!(project_request(&request, &step).is_err());
    step.input_limit = None;
    step.context_window = step.output_cap as usize;
    assert!(project_request(&request, &step).is_err());
}
#[tokio::test]
async fn default_live_selection_does_not_bypass_known_gateway_protocol() {
    let step = step("opencode-go", "claude-haiku-5-5", Some("default")).await;
    assert!(crate::catalog_spec(&step.route.provider, &step.route.model).is_some());
    assert!(reasoning::validate_target(&step.route.provider, &step.route.model).is_err());
}
#[tokio::test]
async fn final_validator_rejects_later_document_and_image_injections() {
    let step = step("deepseek", "deepseek-v4-pro", Some("default")).await;
    assert!(!crate::catalog_spec(&step.route.provider, &step.route.model).unwrap().input.image);
    let mut request = project_request(&CompletionRequest::new("inspect"), &step).unwrap();
    request.documents.push(rig_core::completion::Document {
        id: "late".into(),
        text: "x".repeat(120000),
        additional_props: Default::default(),
    });
    assert!(validate_request(&request, &step).is_err());
    let mut turn = crate::sessions::Turn::plain("user", "inspect");
    turn.parts.push(crate::sessions::ContentPart {
        kind: "image".into(),
        data: json!({"mimeType":"image/png","data":"iVBORw0KGgo="}),
    });
    let request = CompletionRequest::new(crate::rig_history::user_message(&turn).unwrap());
    assert!(validate_request(&request, &step).is_err());
}
#[tokio::test]
async fn named_budget_keeps_its_full_reserve_instead_of_the_default_cap() {
    let step = step("anthropic", "claude-sonnet-4-5", Some("high")).await;
    let Some(rig_core::completion::Reasoning::Budget { tokens }) = step.plan.typed else {
        panic!("fixture needs budget reasoning")
    };
    assert_eq!(step.output_cap, u64::from(tokens) + 4096);
    assert!(step.output_cap > 4096);
    let request = project_request(&CompletionRequest::new("inspect"), &step).unwrap();
    assert_eq!(request.max_tokens, Some(step.output_cap));
    assert_eq!(request.options.reasoning, step.plan.typed);
}
