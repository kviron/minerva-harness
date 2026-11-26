use crate::{
    plugins::PluginManager, run_supervisor::AgentLoopService, sessions::SessionService, workspaces::WorkspaceService,
    RunMessage, StartRunRequest,
};
use minerva_plugin_sdk::delegation::*;
use std::sync::{Arc, Weak};

pub(super) struct MinervaExecutor {
    pub sessions: Arc<SessionService>,
    pub agent_loop: Arc<AgentLoopService>,
    pub plugins: Weak<PluginManager>,
    pub workspaces: Arc<WorkspaceService>,
}

pub(super) fn error(code: ErrorCode, value: impl std::fmt::Display) -> DelegationError {
    DelegationError::new(code, value.to_string())
}

pub struct ChildCheckout {
    pub ceiling: crate::tools::PermissionPolicy,
    pub lease: Arc<dyn crate::run_supervisor::CheckoutWriteLease>,
}

impl MinervaExecutor {
    pub(super) async fn start_scoped(
        &self,
        snapshot: &DelegationSnapshot,
        checkout: Option<ChildCheckout>,
    ) -> DelegationResult<ExecutionHandle> {
        let Target::Minerva { route, effort } = &snapshot.request.target else {
            return Err(error(ErrorCode::Unsupported, "Target is not a Minerva model"));
        };
        let plugins = self.plugins.upgrade().ok_or_else(|| error(ErrorCode::Unavailable, "Runtime stopped"))?;
        let session = format!("del_{}", snapshot.id);
        if checkout.as_ref().is_some_and(|checkout| {
            !checkout.lease.authorizes_child(&session, &snapshot.activation_id) || !checkout.lease.may_write()
        }) {
            return Err(error(ErrorCode::Forbidden, "Checkout lease does not authorize this child activation"));
        }
        // A recovered intent must be reconciled, never silently run a second time.
        if self.agent_loop.get_run(&session, &snapshot.activation_id).is_ok() {
            return Err(error(ErrorCode::OutcomeUnknown, "Child run exists; reconcile its activation before retrying"));
        }
        let mut settings = plugins
            .harness
            .context
            .workspace_get(&snapshot.caller.session_id)
            .map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
        if settings.cwd.as_deref() != Some(snapshot.caller.cwd.as_str()) {
            return Err(error(ErrorCode::OutcomeUnknown, "Parent workspace changed after admission"));
        }
        let workspace = Some(snapshot.caller.workspace_id.as_str());
        let parent_workspace = self
            .workspaces
            .session_workspace(&snapshot.caller.session_id)
            .map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
        if parent_workspace.as_deref() != workspace {
            return Err(error(ErrorCode::Forbidden, "Parent session no longer belongs to this workspace"));
        }
        let parent_id = if let Some(previous) = &snapshot.previous_handle {
            if previous.session_id != session
                || self
                    .agent_loop
                    .get_run(&session, &previous.run_id)
                    .map_err(|e| error(ErrorCode::OutcomeUnknown, e))?
                    .status
                    != "completed"
            {
                return Err(error(ErrorCode::OutcomeUnknown, "Previous child run is not confirmed complete"));
            }
            let child_workspace =
                self.workspaces.session_workspace(&session).map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
            if child_workspace.as_deref() != workspace {
                return Err(error(ErrorCode::Forbidden, "Child session no longer belongs to this workspace"));
            }
            // Completed runs persist the assistant reply under their run ID.
            Some(previous.run_id.clone())
        } else {
            if self.sessions.get(&session).is_ok() {
                return Err(error(ErrorCode::OutcomeUnknown, "Child session exists; reconcile its activation"));
            }
            self.workspaces
                .create_session(&session, &snapshot.caller.workspace_id)
                .map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
            None
        };
        settings.permission_policy = checkout.as_ref().map_or(crate::tools::PermissionPolicy::ReadOnly, |checkout| {
            settings.permission_policy.restrict(checkout.ceiling)
        });
        plugins.harness.context.workspace_save(&session, settings).map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
        let mut options = serde_json::json!({
            "liveSelection": false,
            "delegationToolRegistrations": snapshot.request.tool_registrations,
            "requireCurrentHead": parent_id.is_some(),
        });
        if let Some(effort) = effort {
            options["reasoningEffort"] = serde_json::json!(effort);
        }
        let request = StartRunRequest {
            session_id: session,
            request_id: snapshot.activation_id.clone(),
            parent_id,
            message: RunMessage {
                id: format!("{}_input", snapshot.activation_id),
                text: snapshot.request.prompt.clone(),
            },
            model: route.clone(),
            options,
        };
        let started = match checkout {
            Some(checkout) => {
                self.agent_loop.start_delegated_child_with_checkout(request, checkout.ceiling, checkout.lease).await
            }
            None => self.agent_loop.start_delegated_child(request).await,
        };
        let handle = match started {
            Ok(handle) => handle,
            Err(message) => {
                if self
                    .sessions
                    .confirms_child_stopped_before_run(&snapshot.id, &snapshot.activation_id)
                    .unwrap_or(false)
                {
                    return Err(error(ErrorCode::Cancelled, "Delegation cancelled before execution"));
                }
                return Err(error(ErrorCode::OutcomeUnknown, message));
            }
        };
        Ok(ExecutionHandle { session_id: handle.session_id, run_id: handle.run_id })
    }
}

impl ExecutionPort for MinervaExecutor {
    fn capabilities(&self, target: &Target) -> ExecutionCapabilities {
        let unsupported = Capability::Unsupported { reason: "This adapter executes Minerva API children only".into() };
        if !matches!(target, Target::Minerva { .. }) {
            return ExecutionCapabilities {
                read_only: unsupported.clone(),
                interrupt: unsupported.clone(),
                continuation: unsupported.clone(),
                nested_delegation_control: unsupported,
            };
        }
        ExecutionCapabilities {
            read_only: Capability::Supported { scope: "Minerva tool policy: fixed ReadOnly admission".into() },
            interrupt: Capability::Supported { scope: "Cooperative cancellation of the exact run".into() },
            continuation: Capability::Supported { scope: "New work in the completed Minerva child session".into() },
            nested_delegation_control: Capability::Supported {
                scope: "Persistent root-wide admission and depth limit".into(),
            },
        }
    }

    fn discover<'a>(&'a self, _caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let plugins = self.plugins.upgrade().ok_or_else(|| error(ErrorCode::Unavailable, "Runtime stopped"))?;
            let providers = plugins.providers().map_err(|e| error(ErrorCode::Unavailable, e))?;
            let mut candidates = Vec::new();
            for view in providers.list().await {
                let provider = view.provider();
                for model in &provider.models {
                    let route = format!("api:{}/{model}", provider.id);
                    candidates.push(Candidate {
                        target: Target::Minerva { route: route.clone(), effort: None },
                        read_only: true,
                        unavailable_reason: None,
                    });
                    for effort in view.reasoning(model) {
                        candidates.push(Candidate {
                            target: Target::Minerva { route: route.clone(), effort: Some(effort.clone()) },
                            read_only: true,
                            unavailable_reason: None,
                        });
                    }
                }
            }
            Ok(candidates)
        })
    }

    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(self.start_scoped(snapshot, None))
    }

    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        Box::pin(async move {
            let handle = snapshot.handle.clone().unwrap_or_else(|| ExecutionHandle {
                session_id: format!("del_{}", snapshot.id),
                run_id: snapshot.activation_id.clone(),
            });
            let run = match self.agent_loop.get_run(&handle.session_id, &handle.run_id) {
                Ok(run) => run,
                Err(_)
                    if snapshot.state == ActivationState::Accepted || snapshot.state == ActivationState::Starting =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(error(ErrorCode::OutcomeUnknown, e)),
            };
            let state = match run.status.as_str() {
                "completed" => ActivationState::Succeeded,
                "failed" => ActivationState::Failed,
                "cancelled" => ActivationState::Cancelled,
                "interrupted" => {
                    return Err(error(
                        ErrorCode::OutcomeUnknown,
                        "Child run was interrupted; reconcile before resuming",
                    ))
                }
                _ => return Ok(None),
            };
            let summary = run.error.unwrap_or_else(|| {
                run.parts
                    .iter()
                    .filter(|p| p.kind == "text")
                    .filter_map(|p| p.data["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(Some(Completion { result_id: format!("{}_result", snapshot.activation_id), state, summary }))
        })
    }

    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            let handle = snapshot.handle.clone().unwrap_or_else(|| ExecutionHandle {
                session_id: format!("del_{}", snapshot.id),
                run_id: snapshot.activation_id.clone(),
            });
            let cancelled = self
                .agent_loop
                .cancel_run(&handle.session_id, &handle.run_id)
                .map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
            if !cancelled {
                let run = self
                    .agent_loop
                    .get_run(&handle.session_id, &handle.run_id)
                    .map_err(|e| error(ErrorCode::OutcomeUnknown, e))?;
                if !matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
                    return Err(error(ErrorCode::OutcomeUnknown, "Child cancellation has not been confirmed"));
                }
            }
            Ok(())
        })
    }
}
