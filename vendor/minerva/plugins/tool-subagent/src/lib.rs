//! Model-facing schemas and handlers; execution and persistence belong to the service.
mod schema;
use mh_tool_runtime::{ToolContext, ToolRegistry};
use minerva_delegation::{Caller, DelegationService, FollowupRequest, StartRequest, IDE_USER_OPERATION_PREFIX};
use serde::Deserialize;
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

pub trait CallerResolver: Send + Sync {
    fn resolve(&self, context: &ToolContext) -> Result<Caller, String>;
    fn questions(&self, caller: &Caller, child: &str) -> Result<Vec<Value>, String>;
    fn answer(
        &self,
        caller: &Caller,
        child: &str,
        question: &str,
        operation: &str,
        answer: &str,
    ) -> Result<Value, String>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Answer {
    id: String,
    question_id: String,
    operation_id: String,
    answer: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Lookup {
    id: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Followup {
    id: String,
    operation_id: String,
    prompt: String,
    #[serde(default)]
    writable: bool,
}

async fn write(
    context: &ToolContext,
    caller: Caller,
    request: StartRequest,
    child: Option<String>,
) -> Result<Value, String> {
    let policy = context.effective_policy()?;
    if policy == mh_tool_runtime::PermissionPolicy::ReadOnly {
        return Err("Writable delegation is unavailable in read-only mode".into());
    }
    let reader = context.live_policy.as_ref().ok_or("No host checkout lease")?;
    reader.run_writable_delegation(caller, request, child, policy, context.cancellation.clone()).await
}

pub struct Registration {
    registry: Arc<ToolRegistry>,
    owner: String,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.revoke_owner(&self.owner);
    }
}

pub fn register(
    registry: Arc<ToolRegistry>,
    service: Arc<DelegationService>,
    resolver: Arc<dyn CallerResolver>,
) -> Result<Registration, String> {
    register_tools(registry, service, resolver, true)
}

pub fn register_management(
    registry: Arc<ToolRegistry>,
    service: Arc<DelegationService>,
    resolver: Arc<dyn CallerResolver>,
) -> Result<Registration, String> {
    register_tools(registry, service, resolver, false)
}

fn register_tools(
    registry: Arc<ToolRegistry>,
    service: Arc<DelegationService>,
    resolver: Arc<dyn CallerResolver>,
    allow_admission: bool,
) -> Result<Registration, String> {
    let registration = Registration {
        registry: registry.clone(),
        owner: format!("minerva.delegation.{}", NEXT_OWNER.fetch_add(1, Ordering::Relaxed)),
    };
    for definition in schema::definitions() {
        if !allow_admission
            && matches!(definition.name.as_str(), "subagent_discover" | "subagent_start" | "subagent_send")
        {
            continue;
        }
        let service = service.clone();
        let resolver = resolver.clone();
        let name = definition.name.clone();
        let timeout = if matches!(name.as_str(), "subagent_start" | "subagent_send") {
            Duration::from_secs(900)
        } else {
            Duration::from_secs(35)
        };
        registry.register_with_timeout(&registration.owner, definition, timeout, move |context, args| {
            let service = service.clone();
            let resolver = resolver.clone();
            let name = name.clone();
            Box::pin(async move {
                context.effective_policy()?;
                if context.cancellation.is_cancelled() { return Err("Delegation call cancelled".into()); }
                let caller = resolver.resolve(&context)?;
                match name.as_str() {
                    "subagent_discover" => encode(service.discover(&caller).await.map_err(|e| e.to_string())?),
                    "subagent_start" => {
                        let mut args = args;
                        let writable = args.as_object_mut().and_then(|args| args.remove("writable"))
                            .and_then(|value| value.as_bool()).unwrap_or(false);
                        let mut request: StartRequest = serde_json::from_value(args).map_err(|e| e.to_string())?;
                        // One tool-call retry must refer to the same admission operation.
                        if request.operation_id.is_empty() { request.operation_id = context.call_id.clone(); }
                        validate_model_operation(&request.operation_id)?;
                        request.origin_run = Some(context.run_id.clone());
                        request.tool_registrations = context.delegation_reads.clone();
                        if writable {
                            return write(&context, caller, request, None).await;
                        }
                        encode(service.start_if(&caller, &request, || !context.cancellation.is_cancelled()).await.map_err(|e| e.to_string())?)
                    }
                    "subagent_list" => encode(service.list(&caller).map_err(|e| e.to_string())?),
                    "subagent_interrupt_tree" => encode(service.interrupt_tree(&caller).map_err(|e| e.to_string())?),
                    "subagent_answer" => {
                        let args: Answer = serde_json::from_value(args).map_err(|error| error.to_string())?;
                        resolver.answer(&caller, &args.id, &args.question_id, &args.operation_id, &args.answer)
                    }
                    "subagent_send" => {
                        let input: Followup = serde_json::from_value(args).map_err(|e| e.to_string())?;
                        validate_model_operation(&input.operation_id)?;
                        if input.writable {
                            let child = service.get(&caller, &input.id).map_err(|e| e.to_string())?;
                            let request = StartRequest {
                                operation_id: input.operation_id,
                                prompt: input.prompt,
                                origin_run: Some(context.run_id.clone()),
                                tool_registrations: context.delegation_reads.clone(),
                                target: child.request.target,
                            };
                            return write(&context, caller, request, Some(input.id)).await;
                        }
                        let request = FollowupRequest {
                            operation_id: input.operation_id,
                            prompt: input.prompt,
                            origin_run: Some(context.run_id.clone()),
                            tool_registrations: context.delegation_reads.clone(),
                        };
                        encode(service.followup_if(&caller, &input.id, request, || !context.cancellation.is_cancelled()).map_err(|e| e.to_string())?)
                    }
                    _ => {
                        let lookup: Lookup = serde_json::from_value(args).map_err(|e| e.to_string())?;
                        if name == "subagent_questions" {
                            return encode(resolver.questions(&caller, &lookup.id)?);
                        }
                        let snapshot = match name.as_str() {
                            "subagent_get" => service.poll(&caller, &lookup.id).await,
                            "subagent_interrupt" => service.interrupt(&caller, &lookup.id).await,
                            "subagent_wait" => {
                                let wait = service.wait(&caller, &lookup.id, Duration::from_millis(lookup.timeout_ms.unwrap_or(10_000).min(30_000)));
                                tokio::select! {
                                    result = wait => result,
                                    _ = context.cancellation.cancelled() => return Err("Delegation wait cancelled".into()),
                                    questions = wait_for_question(resolver.as_ref(), &caller, &lookup.id) => {
                                        return Ok(serde_json::json!({"disposition":"coordination-required","questions":questions?}));
                                    }
                                }
                            }
                            _ => return Err("Unknown delegation tool".into()),
                        }.map_err(|e| e.to_string())?;
                        encode(snapshot)
                    }
                }
            })
        })?;
    }
    Ok(registration)
}

async fn wait_for_question(resolver: &dyn CallerResolver, caller: &Caller, child: &str) -> Result<Vec<Value>, String> {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let questions = resolver.questions(caller, child)?;
        if !questions.is_empty() {
            return Ok(questions);
        }
    }
}

fn encode(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn validate_model_operation(operation: &str) -> Result<(), String> {
    if operation.starts_with(IDE_USER_OPERATION_PREFIX) {
        return Err("Operation ID is reserved for user follow-ups".into());
    }
    Ok(())
}
