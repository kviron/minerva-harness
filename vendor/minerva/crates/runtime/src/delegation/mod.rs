//! Composition and adapters for the shared delegation domain.
mod executor;
mod questions;
mod queue;
mod routing;
mod scoped;
pub use scoped::ScopedDelegation;
#[cfg(test)]
mod tests;
mod user_followup;
mod wake;
use crate::{
    plugins::PluginManager, run_supervisor::AgentLoopService, sessions::SessionService, workspaces::WorkspaceService,
};
pub use executor::ChildCheckout;
use minerva_delegation::{limits::Limits, Caller, DelegationService};
use std::sync::{Arc, Weak};

pub struct DelegationRuntime {
    pub service: Arc<DelegationService>,
    internal: Arc<executor::MinervaExecutor>,
    host: Option<Arc<dyn minerva_delegation::ExecutionPort>>,
    registration: Option<minerva_tool_subagent::Registration>,
    stop: tokio::sync::watch::Sender<bool>,
    monitor: Option<tokio::task::JoinHandle<()>>,
}

struct Resolver {
    sessions: Arc<SessionService>,
    plugins: Weak<PluginManager>,
    workspaces: Arc<WorkspaceService>,
}
impl minerva_tool_subagent::CallerResolver for Resolver {
    fn resolve(&self, context: &crate::tools::ToolContext) -> Result<Caller, String> {
        let plugins = self.plugins.upgrade().ok_or("Runtime stopped")?;
        let settings = plugins.harness.context.workspace_get(&context.session_id)?;
        let cwd = settings.cwd.ok_or("Delegation requires a workspace")?;
        if context.cwd.as_deref() != Some(std::path::Path::new(&cwd)) {
            return Err("Tool workspace does not match the session".into());
        }
        let workspace = self.workspaces.session_workspace(&context.session_id)?.ok_or("Session workspace not found")?;
        self.sessions.delegation_caller(&context.session_id, &workspace, &cwd).map_err(|e| e.to_string())
    }

    fn questions(&self, caller: &Caller, child: &str) -> Result<Vec<serde_json::Value>, String> {
        self.list_questions(caller, child)
    }

    fn answer(
        &self,
        caller: &Caller,
        child: &str,
        question: &str,
        operation: &str,
        answer: &str,
    ) -> Result<serde_json::Value, String> {
        self.answer_question(caller, child, question, operation, answer)
    }
}

impl DelegationRuntime {
    pub(super) fn start(
        sessions: Arc<SessionService>,
        agent_loop: Arc<AgentLoopService>,
        plugins: &Arc<PluginManager>,
        host: Option<Arc<dyn minerva_delegation::ExecutionPort>>,
        config: minerva_plugin_host::config::DelegationConfig,
    ) -> Result<Self, String> {
        let workspaces = plugins.context().try_service::<WorkspaceService>().map_err(|e| e.to_string())?;
        let repository = sessions.delegation_repository();
        sessions.revoke_persisted_delegation_reads().map_err(|e| e.to_string())?;
        let mut recoverable = Vec::new();
        for snapshot in sessions.delegation_active().map_err(|e| e.to_string())? {
            if sessions.recoverable_queue_activation(&snapshot).map_err(|e| e.to_string())? {
                recoverable.push(snapshot);
                continue;
            }
            if snapshot.handle.is_none() {
                repository
                    .mark_unknown(
                        &snapshot.id,
                        &snapshot.activation_id,
                        "Runtime restarted before the execution handle was saved",
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        let executor = Arc::new(executor::MinervaExecutor {
            sessions: sessions.clone(),
            agent_loop: agent_loop.clone(),
            plugins: Arc::downgrade(plugins),
            workspaces: workspaces.clone(),
        });
        let enabled = config.enabled;
        let deadline_seconds = config.execution_deadline_seconds;
        let service = Arc::new(
            DelegationService::new(
                repository,
                Arc::new(routing::Executors { internal: executor.clone(), host: host.clone() }),
                Limits::default(),
            )
            .with_fallbacks(config.fallbacks),
        );
        let resolver = Arc::new(Resolver { sessions: sessions.clone(), plugins: Arc::downgrade(plugins), workspaces });
        let registration = if enabled {
            Some(minerva_tool_subagent::register(plugins.harness.tools.clone(), service.clone(), resolver)?)
        } else {
            service.close_admission();
            if sessions.has_delegation_history().map_err(|error| error.to_string())? {
                Some(minerva_tool_subagent::register_management(
                    plugins.harness.tools.clone(),
                    service.clone(),
                    resolver,
                )?)
            } else {
                None
            }
        };
        for snapshot in recoverable {
            service.dispatch_recovered(snapshot);
        }
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let observed = service.clone();
        let queue_plugins = Arc::downgrade(plugins);
        let owner = format!(
            "runtime-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos()
        );
        let monitor = tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut failures = std::collections::BTreeMap::new();
            let mut dispatched_deadlines = std::collections::BTreeSet::new();
            loop {
                tokio::select! {
                    _ = stopped.changed() => break,
                    _ = tick.tick() => {
                        match sessions.delegation_deadline_stops(deadline_seconds) {
                            Ok(mut intents) => {
                                let active = intents.iter().map(|child| child.activation_id.clone()).collect();
                                intents.retain(|child| !dispatched_deadlines.contains(&child.activation_id));
                                observed.interrupt_activations(&intents);
                                dispatched_deadlines = active;
                            }
                            Err(error) => tracing::warn!(%error, "Delegation deadline check failed"),
                        }
                        match sessions.delegation_active() {
                            Ok(active) => for snapshot in active {
                                if let Some(plugins) = queue_plugins.upgrade() {
                                    if let Err(error) = questions::reconcile(&sessions, &plugins.harness.interactions, &snapshot) {
                                        tracing::trace!(delegation = %snapshot.id, %error, "Question reconciliation deferred");
                                    } else if enabled && snapshot.state == minerva_delegation::ActivationState::Running
                                        && matches!(snapshot.request.target, minerva_delegation::Target::Minerva { .. })
                                        && plugins.harness.context.workspace_get(&snapshot.caller.session_id)
                                            .is_ok_and(|settings| settings.cwd.as_deref() == Some(snapshot.caller.cwd.as_str()))
                                    {
                                        if let Err(error) = wake::resume_for_question(&sessions, &agent_loop, &snapshot).await {
                                            tracing::trace!(delegation = %snapshot.id, %error, "Question wake deferred");
                                        }
                                    }
                                }
                                // Observe only. Unknown starts are never automatically executed again.
                                if let Err(error) = observed.poll(&snapshot.caller, &snapshot.id).await {
                                    if failures.get(&snapshot.id) != Some(&error.code) {
                                        tracing::warn!(delegation = %snapshot.id, code = ?error.code, "Delegation observation failed");
                                        failures.insert(snapshot.id, error.code);
                                    }
                                } else { failures.remove(&snapshot.id); }
                            },
                            Err(error) => {
                                tracing::error!(code = ?error.code, "Delegation monitor cannot read storage");
                                break;
                            },
                        }
                        match sessions.delegation_pending() {
                            Ok(pending) => for snapshot in pending {
                                let received = sessions.delegation_consumer(&snapshot.caller, &owner)
                                    .and_then(|generation| sessions.delegation_receive_results(&snapshot.caller, generation, 32));
                                if let Err(error) = received {
                                    if failures.get(&snapshot.id) != Some(&error.code) {
                                        tracing::warn!(delegation = %snapshot.id, code = ?error.code, "Delegation result remains pending");
                                        failures.insert(snapshot.id, error.code);
                                    }
                                }
                            },
                            Err(error) => tracing::warn!(code = ?error.code, "Delegation outbox cannot be read"),
                        }
                        if enabled {
                            if let Some(plugins) = queue_plugins.upgrade() {
                                queue::advance(&sessions, &plugins, &observed);
                            }
                            match sessions.delegation_unbound_results() {
                                Ok(pending) => for child in pending {
                                    // A concurrent user run or a stop may win admission.
                                    if let Err(error) = wake::resume_parent(&sessions, &agent_loop, &child).await {
                                        tracing::trace!(delegation = %child.id, %error, "Parent wake deferred");
                                    }
                                },
                                Err(error) => tracing::trace!(code = ?error.code, "Wake inventory unavailable"),
                            }
                        }
                    }
                }
            }
        });
        Ok(Self { service, internal: executor, host, registration, stop, monitor: Some(monitor) })
    }

    pub(super) async fn shutdown(&mut self) {
        self.service.close_admission();
        self.registration.take();
        self.stop.send_replace(true);
        if let Some(task) = self.monitor.take() {
            let _ = task.await;
        }
        self.service.drain_starts_until(std::time::Duration::from_secs(5)).await;
    }
}
