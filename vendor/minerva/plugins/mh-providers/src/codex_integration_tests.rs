use super::*;
use crate::credential_store::{CredentialScope, MemoryCredentialStore};
use rig_core::{
    http_client::DynHttpClient,
    test_utils::{MockHttpResponse, SequencedHttpClient},
    tool::{Tool, ToolContext},
};
use serde_json::{json, Value};
use std::sync::Arc;

struct Echo;
impl Tool for Echo {
    const NAME: &'static str = "echo";
    type Args = Value;
    type Output = String;
    type Error = std::convert::Infallible;
    fn description(&self) -> String {
        "Echo a test value".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false})
    }
    async fn call(&self, _: &mut ToolContext, args: Value) -> Result<String, Self::Error> {
        Ok(args["value"].as_str().unwrap().into())
    }
}

#[tokio::test]
async fn refreshed_removed_codex_route_does_not_block_other_providers() {
    let dir = tempfile::tempdir().unwrap();
    let service = crate::ProviderService::new_with_store(
        dir.path().join("providers.json"),
        CredentialScope::new("fixture").unwrap(),
        Arc::new(MemoryCredentialStore::default()),
        crate::network_policy::EndpointPolicy::Local,
    )
    .unwrap();
    service.codex.store.save("chatgpt", &json!({"access_token":"fixture-token"}).to_string()).await.unwrap();
    service.codex_configure(vec!["gpt-5.4".into()]).await.unwrap();
    service.codex.models.publish(Vec::new()).unwrap();
    service
        .save(crate::ProviderInput {
            credit_grants: None,
            model_windows: None,
            id: "other".into(),
            name: "Other".into(),
            base_url: "http://127.0.0.1:12345/v1".into(),
            models: vec!["model".into()],
            api_key: Some("fixture-key".into()),
            is_new: true,
            protocol: "openai-chat".into(),
        })
        .await
        .unwrap();
    let routes = service.run_routes("other").await.unwrap();
    assert!(routes.contains_key("api:other/model"));
    assert!(!routes.contains_key("api:openai-codex/gpt-5.4"));
}

#[tokio::test]
#[ignore = "requires explicitly authorized Minerva subscription generation"]
async fn live_subscription_generates_nonce_without_tools_or_history() {
    use crate::tools::{
        Cancellation, InteractionBroker, PermissionPolicy, ToolContext as HostToolContext, ToolRegistry,
    };
    let config =
        std::path::PathBuf::from(std::env::var_os("MINERVA_CODEX_VERIFY_CONFIG_DIR").expect("explicit account config"));
    let dir = tempfile::tempdir().unwrap();
    let mut service = crate::ProviderService::new_with_store(
        dir.path().join("providers.json"),
        CredentialScope::new("local").unwrap(),
        Arc::new(MemoryCredentialStore::default()),
        crate::network_policy::EndpointPolicy::Local,
    )
    .unwrap();
    service.codex = Service::new(
        config.join("providers.json").with_extension("oauth-locks"),
        CredentialScope::new("local").unwrap(),
        Arc::new(crate::credential_store::LocalKeyring),
        true,
    );
    assert!(service.codex.models().iter().any(|id| id == "gpt-6.2-sol"));
    service.providers.lock().await.push(crate::Provider {
        credit_grants: None,
        model_windows: Default::default(),
        id: ID.into(),
        name: "Codex".into(),
        base_url: BASE_URL.into(),
        models: vec!["gpt-6.2-sol".into()],
        protocol: ID.into(),
    });
    let run = crate::AgentRunContext {
        session_id: "live-verification".into(),
        run_id: "nonce".into(),
        tools: ToolRegistry::default().snapshot(),
        tool_context: HostToolContext {
            delegation_reads: Default::default(),
            write_approved: false,
            session_id: "live-verification".into(),
            run_id: "nonce".into(),
            call_id: String::new(),
            cwd: None,
            policy: PermissionPolicy::ReadOnly,
            live_policy: None,
            cancellation: Cancellation::default(),
            interactions: Arc::new(InteractionBroker::default()),
            sink: None,
        },
        context: Arc::new(crate::context::ContextService::open(&dir.path().join("context.sqlite")).unwrap()),
        messages: Vec::new(),
        reasoning_effort: Some("low".into()),
        selection: None,
        followups: None,
    };
    let result = service
        .chat_agent(
            ID,
            "gpt-6.2-sol",
            vec![crate::sessions::Turn::plain("user", "Reply with exactly MINERVA_MODELS_OK")],
            Arc::new(|_| Ok(())),
            run,
        )
        .await
        .unwrap();
    assert!(result.text().contains("MINERVA_MODELS_OK"));
    println!("MINERVA_MODELS_OK");
}

fn response(output: Value) -> MockHttpResponse {
    let event = json!({"type":"response.completed","sequence_number":1,"response":{"id":"fixture-response","object":"response","created_at":1,"status":"completed","model":"gpt-5.4","output":output,"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}});
    MockHttpResponse::success_typed(format!("event: response.completed\ndata: {event}\n\n"), "text/event-stream")
}

#[tokio::test]
async fn signed_in_rig_agent_executes_host_tool_and_replays_result() {
    let http = SequencedHttpClient::new([
        response(
            json!([{"type":"function_call","id":"fixture-item","call_id":"fixture-call","name":"echo","arguments":"{\"value\":\"host-tool-result\"}","status":"completed"}]),
        ),
        response(
            json!([{"type":"message","id":"fixture-message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"done","annotations":[]}]}]),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let mut service = Service::new(
        dir.path().join("locks"),
        CredentialScope::new("fixture").unwrap(),
        Arc::new(MemoryCredentialStore::default()),
        true,
    );
    service
        .store
        .save(
            "chatgpt",
            &json!({"access_token":"fixture-token","account_id":"fixture-account","expires_at":4102444800i64})
                .to_string(),
        )
        .await
        .unwrap();
    service.http = DynHttpClient::new(http.clone());
    let placeholder = crate::openai_config(ID, BASE_URL, String::new()).client().responses("gpt-5.4");
    let agent = rig_agent::AgentBuilder::new(placeholder)
        .tool(Echo)
        .model_route_handler("subscription", service.handler("gpt-5.4").unwrap())
        .build();
    let result = agent.prompt("call echo then finish").using_model("subscription").max_turns(3).await.unwrap();
    assert!(format!("{result:?}").contains("done"));
    let requests = http.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].headers.get("chatgpt-account-id").unwrap(), "fixture-account");
    let body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(body["store"], false);
    assert!(body["input"].as_array().unwrap().iter().any(|item| item["type"] == "function_call_output"
        && item["output"].as_str().is_some_and(|text| text.contains("host-tool-result"))));
}

#[tokio::test(start_paused = true)]
async fn provider_service_routes_signed_in_calls_through_admission_and_host_tools() {
    use crate::{
        network_policy::EndpointPolicy,
        selection::SelectionFuture,
        sessions::Turn,
        tools::{
            Cancellation, InteractionBroker, PermissionPolicy, ToolContext as HostToolContext, ToolDefinition,
            ToolRegistry,
        },
        AgentRunContext, ProviderService, SelectionControl,
    };
    use mh_sessions::{DispatchAdmission, RouteDispatch, RouteSelection};
    struct Control(std::sync::Mutex<Vec<RouteDispatch>>);
    impl SelectionControl for Control {
        fn selection(&self) -> SelectionFuture<'_, RouteSelection> {
            Box::pin(async {
                Ok(RouteSelection {
                    revision: 1,
                    model: format!("api:{ID}/gpt-6.2-astra"),
                    reasoning_effort: Some("ultra".into()),
                })
            })
        }
        fn record_dispatch(&self, value: RouteDispatch) -> SelectionFuture<'_, DispatchAdmission> {
            Box::pin(async move {
                self.0.lock().unwrap().push(value);
                Ok(DispatchAdmission::Recorded { changed: false })
            })
        }
    }
    let http = SequencedHttpClient::new([
        MockHttpResponse::success_typed(json!({"models":[{"slug":"gpt-6.2-astra","visibility":"list","context_window":500000,"effective_context_window_percent":95,"supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}]}]}).to_string(), "application/json"),
        response(
            json!([{"type":"function_call","id":"fixture-item","call_id":"fixture-call","name":"inspect","arguments":"{}","status":"completed"}]),
        ),
        response(
            json!([{"type":"message","id":"fixture-message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"done","annotations":[]}]}]),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let mut service = ProviderService::new_with_store(
        dir.path().join("providers.json"),
        CredentialScope::new("fixture").unwrap(),
        Arc::new(MemoryCredentialStore::default()),
        EndpointPolicy::Local,
    )
    .unwrap();
    service
        .codex
        .store
        .save(
            "chatgpt",
            &json!({"access_token":"fixture-token","account_id":"fixture-account","expires_at":4102444800i64})
                .to_string(),
        )
        .await
        .unwrap();
    service.codex.http = DynHttpClient::new(http.clone());
    assert_eq!(service.codex.discover_models().await.unwrap(), vec!["gpt-6.2-astra"]);
    service.codex_configure(vec!["gpt-6.2-astra".into()]).await.unwrap();
    assert_eq!(service.codex.model_spec("gpt-6.2-astra").unwrap().context_window, Some(475000));
    let public = serde_json::to_value(service.list().await).unwrap();
    assert!(public[0]["reasoning"]["gpt-6.2-astra"].as_array().unwrap().contains(&json!("ultra")));
    let tools = ToolRegistry::default();
    tools
        .register_with_timeout(
            "fixture",
            ToolDefinition {
                name: "inspect".into(),
                description: "inspect".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                output_schema: json!({"type":"object"}),
                side_effect: false,
            },
            std::time::Duration::from_secs(1800),
            |_, _| {
                Box::pin(async {
                    tokio::time::sleep(std::time::Duration::from_secs(900)).await;
                    Ok(json!({"value":"HOST_TOOL_RESULT"}))
                })
            },
        )
        .unwrap();
    let control = Arc::new(Control(std::sync::Mutex::new(Vec::new())));
    let context = Arc::new(crate::context::ContextService::open(&dir.path().join("context.sqlite")).unwrap());
    let run = AgentRunContext {
        session_id: "session".into(),
        run_id: "run".into(),
        tools: tools.snapshot(),
        tool_context: HostToolContext {
            delegation_reads: Default::default(),
            write_approved: false,
            session_id: "session".into(),
            run_id: "run".into(),
            call_id: String::new(),
            cwd: None,
            policy: PermissionPolicy::ReadOnly,
            live_policy: None,
            cancellation: Cancellation::default(),
            interactions: Arc::new(InteractionBroker::default()),
            sink: None,
        },
        context: context.clone(),
        messages: Vec::new(),
        reasoning_effort: Some("ultra".into()),
        selection: Some(control.clone()),
        followups: None,
    };
    let result = service
        .chat_agent(ID, "gpt-6.2-astra", vec![Turn::plain("user", "inspect")], Arc::new(|_| Ok(())), run)
        .await
        .unwrap();
    assert!(result.text().contains("done"));
    let reported = context.last_request("session").unwrap().unwrap();
    assert_eq!(reported.window, 475000);
    assert_eq!(reported.observed_input, Some(1));
    assert_eq!(control.0.lock().unwrap().len(), 2);
    let requests = http.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].uri.to_string().contains("/models?client_version="));
    let first: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first["model"], "gpt-6.2-astra");
    assert_eq!(first["reasoning"]["effort"], "ultra");
    assert!(String::from_utf8_lossy(&requests[2].body).contains("HOST_TOOL_RESULT"));
    assert!(!serde_json::to_string(&service.codex.status().unwrap()).unwrap().contains("fixture-token"));
}
