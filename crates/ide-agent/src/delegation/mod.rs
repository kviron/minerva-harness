//! Project-scoped native execution, admission and turn correlation.
pub(super) mod attempt;
mod history;
pub(super) mod idle;
#[cfg(all(test, windows))]
mod live_claude_tests;
#[cfg(all(test, windows))]
mod live_codex_tests;
mod options;
mod prepare;
pub(super) mod scoped;
mod store;
#[cfg(test)]
mod tests;
mod user;

use minerva_plugin_sdk::delegation::*;
use std::path::{Path, PathBuf};
use std::sync::Weak;
pub use store::{Admission, AdmissionPhase, AdmissionStore};

pub struct HostExecutor {
    manager: Weak<crate::AgentManager>,
}

impl HostExecutor {
    pub fn new(manager: Weak<crate::AgentManager>) -> Self {
        Self { manager }
    }
}

const UNCORRELATED: &str = "Native delegation requires durable host turn identity and recovery";
const CLAUDE_MISSING: &str = "Claude Code executable is not available in this project's launch environment";
const CODEX_MISSING: &str = "Codex executable is not available in this project's launch environment";

fn project_workspace(cwd: &str, project: &Path, unavailable: ErrorCode) -> DelegationResult<PathBuf> {
    let cwd = dunce::canonicalize(cwd).map_err(|_| DelegationError::new(ErrorCode::Forbidden, "Unknown workspace"))?;
    let root = dunce::canonicalize(project).map_err(|_| DelegationError::new(unavailable, "Project is unavailable"))?;
    if cwd != root {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Workspace does not belong to this project"));
    }
    Ok(root)
}

fn native_installed(spec: &crate::provider::ProviderSpec, root: &std::path::Path) -> bool {
    let launch = spec.clone().resolve_for_launch(root);
    let executable = ide_jsonrpc::resolve_spawn_program(&launch.command, &launch.env);
    (executable.is_absolute() || executable.components().count() > 1)
        && root.join(executable).to_str().is_some_and(crate::provider::installed)
}

impl HostExecutor {
    async fn start_native(
        &self,
        snapshot: &DelegationSnapshot,
        checkout: Option<NativeCheckout>,
    ) -> DelegationResult<ExecutionHandle> {
        let writable = checkout.is_some();
        let prepared = self.prepare_native(snapshot, writable).await?;
        let (manager, ledger, intent) = self.resolve_attempt(snapshot).await?;
        if prepared != intent {
            return Err(DelegationError::new(ErrorCode::OutcomeUnknown, "Native attempt changed during preparation"));
        }
        let handle = ExecutionHandle { session_id: intent.task_id.clone(), run_id: intent.request_id.clone() };
        if intent.phase != AdmissionPhase::Reserved {
            return Ok(handle);
        }
        let task = manager
            .task(&intent.task_id)
            .map_err(|e| DelegationError::new(ErrorCode::OutcomeUnknown, e.to_string()))?;
        let (ledger, intent) = if snapshot.previous_handle.is_some() && task.state.lock().busy {
            drop(ledger);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while task.state.lock().busy {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .map_err(|_| DelegationError::new(ErrorCode::Busy, "The previous native turn is still settling"))?;
            let (_, ledger, intent) = self.resolve_attempt(snapshot).await?;
            if intent.phase != AdmissionPhase::Reserved {
                return Ok(handle);
            }
            (ledger, intent)
        } else {
            (ledger, intent)
        };
        {
            let mut state = task.state.lock();
            if !state.delegated_read_only || state.busy {
                return Err(DelegationError::new(ErrorCode::OutcomeUnknown, "Native task is not idle"));
            }
            *task.scoped_native.lock() = checkout.map(|grant| scoped::ScopedNative::new(snapshot.clone(), grant));
            state.busy = true;
        }
        drop(ledger);
        if writable
            && snapshot.previous_handle.is_some()
            && let Some(live) = task.live.lock().take()
        {
            live.peer.kill();
        }
        let prompt = crate::AgentPrompt { text: snapshot.request.prompt.clone(), ..Default::default() };
        manager.shared.runtime.spawn(async move { task.run_with_attempt(prompt, Some(intent.operation_id)).await });
        Ok(handle)
    }
}

fn read_only_driver(driver: crate::provider::Driver) -> bool {
    matches!(driver, crate::provider::Driver::Codex | crate::provider::Driver::Claude)
}

impl ExecutionPort for HostExecutor {
    fn capabilities_for(&self, _caller: &Caller, target: &Target) -> ExecutionCapabilities {
        let mut caps = self.capabilities(target);
        if let Target::IdeAgent { agent_id, options } = target
            && options.keys().all(|key| matches!(key.as_str(), "model" | "effort"))
            && let Some(spec) = self.manager.upgrade().and_then(|m| m.shared.spec(agent_id))
            && read_only_driver(spec.driver)
        {
            let scope = match spec.driver {
                crate::provider::Driver::Codex => "Codex read-only sandbox and verified disabled MCP inventory",
                crate::provider::Driver::Claude => {
                    "Claude restricted workspace with verified Read, Glob and Grep inventory"
                }
                _ => unreachable!(),
            };
            caps.read_only = Capability::Supported { scope: scope.into() };
            caps.continuation =
                Capability::Supported { scope: "A successful native attempt with a confirmed original session".into() };
        }
        caps
    }
    fn discover<'a>(&'a self, caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let manager = self
                .manager
                .upgrade()
                .ok_or_else(|| DelegationError::new(ErrorCode::Unavailable, "Project is closed"))?;
            let root = project_workspace(&caller.cwd, &manager.shared.paths.project_root, ErrorCode::Unavailable)?;
            let providers = manager.shared.providers.read().clone();
            let launches = providers
                .iter()
                .filter(|spec| read_only_driver(spec.driver) && native_installed(spec, &root))
                .filter_map(|spec| {
                    let (catalog, flight) =
                        manager.shared.paths.catalogs.request(&manager.shared, spec.clone(), false, None);
                    if catalog.refreshed_at_ms.is_some() { None } else { flight }
                })
                .collect::<Vec<_>>();
            // All refreshes start before waiting, with one budget for the entire lookup.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(8), async {
                for mut flight in launches {
                    let _ = flight.wait_for(|done| *done).await;
                }
            })
            .await;
            let candidates = providers
                .iter()
                .filter(|p| p.driver != crate::provider::Driver::Minerva)
                .flat_map(|p| {
                    let supported = read_only_driver(p.driver);
                    let unavailable_reason = if !supported {
                        Some(UNCORRELATED.into())
                    } else if !native_installed(p, &root) {
                        Some(
                            if p.driver == crate::provider::Driver::Claude { CLAUDE_MISSING } else { CODEX_MISSING }
                                .into(),
                        )
                    } else {
                        None
                    };
                    let mut candidates = vec![Candidate {
                        target: Target::IdeAgent { agent_id: p.id.clone(), options: Default::default() },
                        read_only: supported,
                        unavailable_reason: unavailable_reason.clone(),
                    }];
                    if unavailable_reason.is_none() {
                        let catalog = manager.shared.paths.catalogs.cached(&manager.shared, &p.id);
                        if catalog.refreshed_at_ms.is_some() {
                            candidates.extend(
                                options::catalog_targets(&p.id, &catalog).into_iter().map(|target| Candidate {
                                    target,
                                    read_only: true,
                                    unavailable_reason: None,
                                }),
                            );
                        }
                    }
                    candidates
                })
                .collect();
            Ok(candidates)
        })
    }

    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(self.start_native(snapshot, None))
    }

    fn supports_scoped_write(&self, _: &Caller, target: &Target) -> bool {
        cfg!(windows)
            && matches!(target, Target::IdeAgent { agent_id, .. }
            if self.manager.upgrade().and_then(|m| m.shared.spec(agent_id))
                .is_some_and(|spec| read_only_driver(spec.driver)))
    }

    fn check_scoped_followup<'a>(&'a self, child: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            let (manager, _, intent) = self.resolve_attempt(child).await?;
            let Target::IdeAgent { agent_id, .. } = &child.request.target else {
                return Err(DelegationError::new(ErrorCode::Unsupported, UNCORRELATED));
            };
            let spec = manager
                .shared
                .spec(agent_id)
                .ok_or_else(|| DelegationError::new(ErrorCode::Unavailable, "Agent is unavailable"))?;
            prepare::check_scoped_tools(spec.driver, &intent)
        })
    }

    fn start_with_checkout<'a>(
        &'a self,
        snapshot: &'a DelegationSnapshot,
        checkout: NativeCheckout,
    ) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move {
            if !self.supports_scoped_write(&snapshot.caller, &snapshot.request.target)
                || checkout.write_policy(snapshot) == minerva_plugin_sdk::tools::PermissionPolicy::ReadOnly
            {
                return Err(DelegationError::new(
                    ErrorCode::Unsupported,
                    "Native writable delegation requires a supported task on Windows and write permission",
                ));
            }
            self.start_native(snapshot, Some(checkout)).await
        })
    }

    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        Box::pin(async move {
            let (manager, _, intent) = self.resolve_attempt(snapshot).await?;
            if let Some(completion) = intent.completion {
                return Ok(Some(completion));
            }
            if intent.phase == AdmissionPhase::Reserved
                || manager.task(&intent.task_id).is_ok_and(|task| task.state.lock().busy)
            {
                return Ok(None);
            }
            // The task can settle after our first ledger read and before it clears busy.
            let (_, _, latest) = self.resolve_attempt(snapshot).await?;
            if let Some(completion) = latest.completion {
                return Ok(Some(completion));
            }
            Err(DelegationError::new(ErrorCode::OutcomeUnknown, "Native attempt has no confirmed result"))
        })
    }

    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            let (manager, store, intent) = self.resolve_attempt(snapshot).await?;
            if intent.phase == AdmissionPhase::Reserved {
                store.cancel_reserved(&intent.operation_id)?;
                return Ok(());
            }
            if intent.phase == AdmissionPhase::Settled {
                return Ok(());
            }
            drop(store);
            let task = manager
                .task(&intent.task_id)
                .map_err(|error| DelegationError::new(ErrorCode::OutcomeUnknown, error.to_string()))?;
            let (delegated, busy) = {
                let state = task.state.lock();
                (state.delegated_read_only, state.busy)
            };
            if !delegated || (!busy && task.live.lock().is_none()) {
                return Err(DelegationError::new(ErrorCode::OutcomeUnknown, "Native attempt is not attached"));
            }
            task.cancel();
            Ok(())
        })
    }
}

impl HostExecutor {
    async fn resolve_attempt(
        &self,
        snapshot: &DelegationSnapshot,
    ) -> DelegationResult<(std::sync::Arc<crate::AgentManager>, AdmissionStore, Admission)> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| DelegationError::new(ErrorCode::OutcomeUnknown, "Project is closed"))?;
        project_workspace(&snapshot.caller.cwd, &manager.shared.paths.project_root, ErrorCode::OutcomeUnknown)?;
        let (operation, payload_hash) = attempt_identity(snapshot)?;
        let store = attempt::open_for_turn(&manager.shared.paths.store_dir).await?;
        let intent = store.lookup(&operation).ok_or_else(|| {
            DelegationError::new(ErrorCode::OutcomeUnknown, "Native attempt is missing from the project ledger")
        })?;
        if intent.activation_id != snapshot.activation_id
            || intent.payload_hash != payload_hash
            || snapshot
                .handle
                .as_ref()
                .is_some_and(|handle| handle.session_id != intent.task_id || handle.run_id != intent.request_id)
        {
            return Err(DelegationError::new(ErrorCode::Conflict, "Native attempt does not match the delegation"));
        }
        Ok((manager, store, intent))
    }
}

fn attempt_identity(snapshot: &DelegationSnapshot) -> DelegationResult<(String, String)> {
    fn hash(value: impl serde::Serialize) -> DelegationResult<String> {
        let bytes =
            serde_json::to_vec(&value).map_err(|error| DelegationError::new(ErrorCode::Invalid, error.to_string()))?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }
    Ok((
        hash((&snapshot.caller.session_id, &snapshot.request.operation_id))?,
        if let Some(previous) = &snapshot.previous_handle {
            hash((&snapshot.caller, &snapshot.request, &snapshot.id, &snapshot.activation_id, previous))?
        } else {
            hash((&snapshot.caller, &snapshot.request, &snapshot.id, &snapshot.activation_id))?
        },
    ))
}
