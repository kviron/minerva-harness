//! Scoped admission, policy, cancellation and canonical tool validation.
//! DSH contracts/pipeline/executor mapped to owned Rust snapshots and futures;
//! approval/questions use per-call oneshots rather than Cordis waterfalls.
#[cfg(test)]
mod delegation_tests;
pub mod files;
mod interaction;
#[cfg(test)]
mod permission_tests;
pub use interaction::InteractionBroker;
pub use minerva_plugin_sdk::tools::*;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::watch;
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;
pub type ToolHandler = dyn Fn(ToolContext, Value) -> ToolFuture + Send + Sync;
#[derive(Clone)]
pub struct Cancellation {
    local: watch::Sender<bool>,
    parent: Option<Arc<Cancellation>>,
}
impl Default for Cancellation {
    fn default() -> Self {
        Self { local: watch::channel(false).0, parent: None }
    }
}
impl Cancellation {
    fn child(parent: Cancellation) -> Self {
        Self { parent: Some(Arc::new(parent)), ..Self::default() }
    }
    pub fn cancel(&self) {
        self.local.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.local.borrow() || self.parent.as_ref().is_some_and(|p| p.is_cancelled())
    }
    pub fn cancelled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut rx = self.local.subscribe();
            if self.is_cancelled() {
                return;
            }
            let own = async move {
                while rx.changed().await.is_ok() {
                    if *rx.borrow() {
                        return;
                    }
                }
            };
            let parent = async {
                match &self.parent {
                    Some(parent) => parent.cancelled().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {_=own=>{},_=parent=>{}}
        })
    }
}
pub trait ToolEventSink: Send + Sync {
    fn emit(
        &self,
        context: &ToolContext,
        name: &str,
        args: &Value,
        status: &str,
        result: Option<&Result<Value, String>>,
    ) -> Result<(), String>;
}
pub trait LivePermissionPolicy: Send + Sync {
    fn read_policy(&self, session_id: &str) -> Result<PermissionPolicy, String>;

    fn run_writable_delegation(
        &self,
        _caller: minerva_plugin_sdk::delegation::Caller,
        _request: minerva_plugin_sdk::delegation::StartRequest,
        _child: Option<String>,
        _policy: PermissionPolicy,
        _cancel: Cancellation,
    ) -> ToolFuture {
        Box::pin(async { Err("Writable delegation requires an OpenIDEA checkout lease".into()) })
    }
}
#[derive(Clone)]
pub struct ToolContext {
    /// Populated by the admitted snapshot, never by model arguments.
    pub delegation_reads: BTreeMap<String, u64>,
    /// Bound to this call by the registry after a positive interactive approval.
    pub write_approved: bool,
    pub session_id: String,
    pub run_id: String,
    pub call_id: String,
    pub cwd: Option<PathBuf>,
    /// Admission upper bound; a live reader may further restrict this call.
    pub policy: PermissionPolicy,
    pub live_policy: Option<Arc<dyn LivePermissionPolicy>>,
    pub cancellation: Cancellation,
    pub interactions: Arc<InteractionBroker>,
    pub sink: Option<Arc<dyn ToolEventSink>>,
}
impl ToolContext {
    pub fn verify_admitted_write(&self) -> Result<(), String> {
        match self.effective_policy()? {
            PermissionPolicy::ReadOnly => Err("tool writes are forbidden in read-only mode".into()),
            PermissionPolicy::Ask if !self.write_approved => Err("tool policy changed; approval required".into()),
            _ => Ok(()),
        }
    }

    pub fn effective_policy(&self) -> Result<PermissionPolicy, String> {
        let current = match &self.live_policy {
            Some(reader) => reader.read_policy(&self.session_id)?,
            None => self.policy,
        };
        Ok(self.policy.restrict(current))
    }
}

struct Registration {
    id: u64,
    owner: String,
    definition: ToolDefinition,
    handler: Arc<ToolHandler>,
    active: AtomicBool,
    input: jsonschema::Validator,
    output: jsonschema::Validator,
    timeout: Duration,
}
#[derive(Default)]
pub struct ToolRegistry(Mutex<BTreeMap<String, Arc<Registration>>>);
impl cordis::Service for ToolRegistry {
    const NAME: &'static str = "studio.tools";
}
#[derive(Clone, Default)]
pub struct AdmittedTools(BTreeMap<String, Arc<Registration>>, IdentityHook, Arc<tokio::sync::RwLock<()>>);
static NEXT_REGISTRATION: AtomicU64 = AtomicU64::new(1);
impl ToolRegistry {
    pub fn register<F>(&self, owner: &str, definition: ToolDefinition, handler: F) -> Result<(), String>
    where
        F: Fn(ToolContext, Value) -> ToolFuture + Send + Sync + 'static,
    {
        self.register_with_timeout(owner, definition, Duration::from_secs(120), handler)
    }
    pub fn register_with_timeout<F>(
        &self,
        owner: &str,
        definition: ToolDefinition,
        timeout: Duration,
        handler: F,
    ) -> Result<(), String>
    where
        F: Fn(ToolContext, Value) -> ToolFuture + Send + Sync + 'static,
    {
        if owner.is_empty()
            || definition.name.is_empty()
            || definition.name.len() > 128
            || !definition.name.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
        {
            return Err("invalid tool name or owner".into());
        }
        let input = jsonschema::validator_for(&definition.input_schema).map_err(|e| e.to_string())?;
        let output = jsonschema::validator_for(&definition.output_schema).map_err(|e| e.to_string())?;
        let mut tools = self.0.lock().map_err(|e| e.to_string())?;
        if tools.contains_key(&definition.name) {
            return Err("tool already registered".into());
        }
        tools.insert(
            definition.name.clone(),
            Arc::new(Registration {
                id: NEXT_REGISTRATION.fetch_add(1, Ordering::Relaxed),
                owner: owner.into(),
                definition,
                handler: Arc::new(handler),
                active: AtomicBool::new(true),
                input,
                output,
                timeout,
            }),
        );
        Ok(())
    }
    pub fn revoke_owner(&self, owner: &str) {
        if let Ok(mut tools) = self.0.lock() {
            tools.retain(|_, tool| {
                if tool.owner == owner {
                    tool.active.store(false, Ordering::SeqCst);
                    false
                } else {
                    true
                }
            })
        }
    }
    pub fn snapshot(&self) -> AdmittedTools {
        {
            let tools = self.0.lock().expect("tool registry poisoned").clone();
            let hook = IdentityHook { names: tools.keys().cloned().collect(), pending: Arc::default() };
            AdmittedTools(tools, hook, Arc::default())
        }
    }
}
impl AdmittedTools {
    pub fn try_quiesce_writes(&self) -> Result<tokio::sync::OwnedRwLockWriteGuard<()>, String> {
        self.2.clone().try_write_owned().map_err(|_| "Tool writes are still active".into())
    }

    pub fn delegation_reads(&self) -> BTreeMap<String, u64> {
        self.0.iter().filter(|(_, tool)| delegation_reader(tool)).map(|(name, tool)| (name.clone(), tool.id)).collect()
    }
    pub fn delegation_writes(&self) -> BTreeMap<String, u64> {
        self.0.iter().filter(|(_, tool)| delegation_writer(tool)).map(|(name, tool)| (name.clone(), tool.id)).collect()
    }

    pub fn restrict_to_registrations(self, allowed: &BTreeMap<String, u64>) -> Self {
        self.restrict_delegation(allowed, false)
    }

    pub fn restrict_delegation(mut self, allowed: &BTreeMap<String, u64>, writable: bool) -> Self {
        self.0.retain(|name, tool| {
            (delegation_reader(tool) || writable && delegation_writer(tool)) && allowed.get(name) == Some(&tool.id)
        });
        self.1 = IdentityHook { names: self.0.keys().cloned().collect(), pending: Arc::default() };
        self
    }

    pub fn restrict_to(mut self, names: &[String]) -> Self {
        self.0.retain(|name, _| names.contains(name));
        self.1 = IdentityHook { names: self.0.keys().cloned().collect(), pending: Arc::default() };
        self
    }
    pub fn identity_hook(&self) -> IdentityHook {
        self.1.clone()
    }
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.0.values().map(|r| r.definition.clone()).collect()
    }
    pub fn without_workspace(mut self) -> Self {
        self.0.retain(|_, t| t.owner != "native.workspace" && t.owner != "native.skills" && t.owner != "native.lsp");
        self
    }
    pub fn without_owner(mut self, owner: &str) -> Self {
        self.0.retain(|_, t| t.owner != owner);
        self
    }
    pub async fn execute(&self, name: &str, args: Value, mut ctx: ToolContext) -> Result<Value, String> {
        ctx.delegation_reads = self.delegation_reads();
        let caller_cancellation = ctx.cancellation.clone();
        ctx.cancellation = Cancellation::child(caller_cancellation.clone());
        let tool = self.0.get(name).ok_or("tool was not admitted for this step")?;
        let emit = |status: &str, result: Option<&Result<Value, String>>| -> Result<(), String> {
            if let Some(sink) = &ctx.sink {
                sink.emit(&ctx, name, &args, status, result)?;
            }
            Ok(())
        };
        emit("pending", None)?;
        let outcome = async {
            if ctx.cancellation.is_cancelled() || caller_cancellation.is_cancelled() {
                return Err("tool cancelled".into());
            }
            if !tool.active.load(Ordering::SeqCst) {
                return Err("tool owner was revoked".into());
            }
            tool.input.validate(&args).map_err(|e| format!("invalid tool input: {e}"))?;
            let mut approved = false;
            if tool.definition.side_effect {
                match ctx.effective_policy()? {
                    PermissionPolicy::ReadOnly => return Err("tool writes are forbidden in read-only mode".into()),
                    PermissionPolicy::Ask => {
                        emit("requires-approval", None)?;
                        let prompt = if delegation_writer(tool) {
                            let path = args.get("path").and_then(Value::as_str).ok_or("Missing workspace file path")?;
                            format!("Allow {name} on {path:?} for this call?")
                        } else {
                            format!("Allow {name} for this call?")
                        };
                        let answer = ctx.interactions.request(&ctx, "approval", name, &prompt, vec![]).await?;
                        if answer != "allow" {
                            return Err("tool approval denied".into());
                        }
                        approved = true;
                    }
                    PermissionPolicy::Allow => {}
                }
            }
            let _write_dispatch = if tool.definition.side_effect {
                Some(self.2.try_read().map_err(|_| "Tool writes are paused")?)
            } else {
                None
            };
            let verify_dispatch = || -> Result<(), String> {
                if ctx.cancellation.is_cancelled() || caller_cancellation.is_cancelled() {
                    return Err("tool cancelled".into());
                }
                if !tool.active.load(Ordering::SeqCst) {
                    return Err("tool owner was revoked".into());
                }
                if tool.definition.side_effect {
                    match ctx.effective_policy()? {
                        PermissionPolicy::ReadOnly => return Err("tool writes are forbidden in read-only mode".into()),
                        PermissionPolicy::Ask if !approved => {
                            return Err("tool policy changed; approval required".into())
                        }
                        _ => {}
                    }
                }
                Ok(())
            };
            verify_dispatch()?;
            emit("running", None)?;
            if ctx.sink.is_some() {
                verify_dispatch()?;
            }
            let mut admitted = ctx.clone();
            admitted.write_approved = approved;
            let output = (tool.handler)(admitted, args.clone()).await?;
            tool.output.validate(&output).map_err(|e| format!("invalid tool output: {e}"))?;
            if serde_json::to_vec(&output).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
                return Err("tool output exceeds 1 MiB".into());
            }
            Ok(output)
        };
        // Native handlers settle after killing owned processes. In-process handlers
        // must cooperate; a broken handler cannot hold the session forever.
        tokio::pin!(outcome);
        let result = tokio::select! {
            result=&mut outcome=>result,
            _=caller_cancellation.cancelled()=>{
                ctx.cancellation.cancel();ctx.interactions.cancel_call(&ctx);
                let _=tokio::time::timeout(Duration::from_secs(5),&mut outcome).await;
                Err("tool cancelled".into())
            },
            _=tokio::time::sleep(tool.timeout)=>{
                ctx.cancellation.cancel();ctx.interactions.cancel_call(&ctx);
                let _=tokio::time::timeout(Duration::from_secs(5),&mut outcome).await;
                Err("tool timed out".into())
            }
        };
        emit(
            if ctx.cancellation.is_cancelled() {
                "cancelled"
            } else if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            Some(&result),
        )?;
        result
    }
    pub fn rig_tools(&self, ctx: ToolContext) -> Vec<rig_core::tool::DynamicTool> {
        self.0
            .values()
            .map(|tool| {
                let admitted = self.clone();
                let ctx = ctx.clone();
                let name = tool.definition.name.clone();
                rig_core::tool::DynamicTool::new_with_context(
                    rig_core::message::ToolName::new(name.clone()).expect("validated name"),
                    tool.definition.description.clone(),
                    tool.definition.input_schema.clone(),
                    move |rig_ctx: &mut rig_core::tool::ToolContext, args| {
                        let admitted = admitted.clone();
                        let name = name.clone();
                        let mut ctx = ctx.clone();
                        let identity = rig_ctx.get::<RigCallIdentity>();
                        let envelope = args.get("__minerva_dispatch").and_then(Value::as_str).map(String::from);
                        Box::pin(async move {
                            let args = if let Some(token) = envelope {
                                let dispatch = admitted
                                    .1
                                    .pending
                                    .lock()
                                    .map_err(|_| rig_core::tool::ToolExecutionError::other("identity lock poisoned"))?
                                    .remove(&token)
                                    .ok_or_else(|| {
                                        rig_core::tool::ToolExecutionError::invalid_args("invalid host dispatch token")
                                    })?;
                                if dispatch.name != name {
                                    return Err(rig_core::tool::ToolExecutionError::invalid_args(
                                        "dispatch tool mismatch",
                                    ));
                                }
                                ctx.call_id = dispatch.call_id;
                                dispatch.args
                            } else {
                                args
                            };
                            if let Some(identity) =
                                identity.map_err(|e| rig_core::tool::ToolExecutionError::invalid_args(e.to_string()))?
                            {
                                ctx.call_id = identity.call_id;
                            }
                            if ctx.call_id.is_empty() {
                                return Err(rig_core::tool::ToolExecutionError::invalid_args(
                                    "missing exact tool call identity",
                                ));
                            }
                            admitted
                                .execute(&name, args, ctx)
                                .await
                                .map(rig_core::tool::ToolOutput::json)
                                .map_err(rig_core::tool::ToolExecutionError::other)
                        })
                    },
                )
            })
            .collect()
    }
}
fn delegation_writer(tool: &Registration) -> bool {
    tool.definition.side_effect
        && tool.owner == "native.workspace"
        && matches!(tool.definition.name.as_str(), "write_file" | "patch_file")
}

fn delegation_reader(tool: &Registration) -> bool {
    tool.active.load(Ordering::SeqCst)
        && !tool.definition.side_effect
        && matches!(
            (tool.owner.as_str(), tool.definition.name.as_str()),
            ("native.workspace", "read_file" | "search_files") | ("native.interaction", "ask_user")
        )
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RigCallIdentity {
    pub call_id: String,
}
impl rig_core::tool::ContextValue for RigCallIdentity {
    const KEY: &'static str = "minerva.call-identity";
}

#[derive(Clone, Default)]
pub struct IdentityHook {
    names: std::collections::BTreeSet<String>,
    pending: Arc<Mutex<BTreeMap<String, IdentityDispatch>>>,
}
struct IdentityDispatch {
    name: String,
    call_id: String,
    args: Value,
}
impl rig_agent::agent::hook::AgentHook for IdentityHook {
    async fn on_dispatch(
        &self,
        _: &rig_agent::agent::hook::HookContext,
        event: rig_agent::agent::hook::DispatchEvent<'_>,
    ) -> rig_agent::agent::hook::DispatchAction {
        use rig_agent::agent::hook::DispatchAction;
        let (Some(name), Some(args), Some(call)) = (event.tool_name(), event.tool_args(), event.call_id) else {
            return DispatchAction::proceed();
        };
        if !self.names.contains(name) {
            return DispatchAction::proceed();
        }
        let Ok(args) = serde_json::from_str(args) else {
            return DispatchAction::skip("invalid tool arguments");
        };
        let token = event.id.to_string();
        let Ok(mut pending) = self.pending.lock() else {
            return DispatchAction::stop("identity registry poisoned");
        };
        pending.insert(token.clone(), IdentityDispatch { name: name.into(), call_id: call.to_string(), args });
        DispatchAction::rewrite_tool_args(event.kind, json!({"__minerva_dispatch":token}))
    }
    async fn on_outcome(
        &self,
        _: &rig_agent::agent::hook::HookContext,
        event: rig_agent::agent::hook::OutcomeEvent<'_>,
    ) -> rig_agent::agent::hook::OutcomeAction {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&event.id.to_string());
        }
        rig_agent::agent::hook::OutcomeAction::proceed()
    }
}

pub struct ToolRuntimePlugin {
    pub tools: Arc<ToolRegistry>,
    pub interactions: Arc<InteractionBroker>,
}
impl cordis::Plugin for ToolRuntimePlugin {
    type Config = ();
    type Input = ();
    type PrepareError = std::convert::Infallible;
    type ApplyError = cordis::service::ServicePublishError;
    fn prepare(&self, (): ()) -> Result<(), std::convert::Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: cordis::Context, _: &()) -> Result<(), Self::ApplyError> {
        let _tools = ctx.provide(self.tools.clone())?;
        let _interactions = ctx.provide(self.interactions.clone())?;
        Ok(())
    }
}
#[cfg(test)]
mod allowlist_tests {
    use super::*;
    #[test]
    fn restriction_cannot_add_tools() {
        let registry = ToolRegistry::default();
        for name in ["allowed", "blocked"] {
            registry
                .register(
                    "test",
                    ToolDefinition {
                        name: name.into(),
                        description: String::new(),
                        input_schema: serde_json::json!({}),
                        output_schema: serde_json::json!({}),
                        side_effect: false,
                    },
                    |_, _| Box::pin(async { Ok(serde_json::Value::Null) }),
                )
                .unwrap();
        }
        let narrowed = registry.snapshot().restrict_to(&["allowed".into(), "nonexistent".into()]);
        assert_eq!(narrowed.definitions().iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["allowed"]);
        assert!(registry.snapshot().restrict_to(&[]).definitions().is_empty());
    }
}
