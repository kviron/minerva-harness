use super::{Admission, AdmissionPhase, HostExecutor};
use crate::{AgentPrompt, Isolation, NewAgentTask};
use ide_core::IpcPath;
use minerva_plugin_sdk::delegation::*;

impl HostExecutor {
    /// Persists a correlated idle task only. This is not permission to dispatch a native turn.
    pub async fn prepare_native(&self, snapshot: &DelegationSnapshot, writable: bool) -> DelegationResult<Admission> {
        let manager = self.manager.upgrade().ok_or_else(|| failure(ErrorCode::Unavailable, "Project is closed"))?;
        let Target::IdeAgent { agent_id, options } = &snapshot.request.target else {
            return Err(failure(ErrorCode::Unsupported, "Expected a native IDE agent target"));
        };
        let spec =
            manager.shared.spec(agent_id).ok_or_else(|| failure(ErrorCode::Unavailable, "Agent is unavailable"))?;
        if !super::read_only_driver(spec.driver) {
            return Err(failure(ErrorCode::Unsupported, "Native agent does not support delegated read-only execution"));
        }
        let cwd =
            super::project_workspace(&snapshot.caller.cwd, &manager.shared.paths.project_root, ErrorCode::Unavailable)?;
        if snapshot.request.prompt.trim().is_empty() {
            return Err(failure(ErrorCode::Invalid, "Delegated task prompt is empty"));
        }
        if let Some(model) = options.get("model") {
            tokio::time::timeout(
                std::time::Duration::from_secs(20),
                manager.shared.paths.catalogs.load_levels(&manager.shared, agent_id, model),
            )
            .await
            .map_err(|_| failure(ErrorCode::Unavailable, "Native model discovery timed out"))?
            .map_err(|error| failure(ErrorCode::Unavailable, error))?;
        }
        let catalog = manager.shared.paths.catalogs.cached(&manager.shared, agent_id);
        let choices = super::options::validate(options, &catalog)?;
        let (operation, payload_hash) = super::attempt_identity(snapshot)?;
        let store = super::attempt::open_for_turn(&manager.shared.paths.store_dir).await?;
        let (task_id, native_session_id, codex_scoped_tools) = if let Some(previous) = &snapshot.previous_handle {
            let prior = store.for_execution(&previous.session_id, &previous.run_id).ok_or_else(|| {
                failure(ErrorCode::OutcomeUnknown, "Previous native execution could not be identified")
            })?;
            if prior.parent_session_id != snapshot.caller.session_id
                || prior.root_session_id != snapshot.caller.root_session_id
                || prior.delegation_id != snapshot.id
            {
                return Err(failure(ErrorCode::Forbidden, "Previous native execution belongs to another delegation"));
            }
            if prior.phase != AdmissionPhase::Settled
                || prior.completion.as_ref().is_none_or(|result| result.state != ActivationState::Succeeded)
                || prior.host_turn_id.is_none()
            {
                return Err(failure(ErrorCode::Conflict, "Prior native execution has no successful confirmed turn"));
            }
            if writable {
                check_scoped_tools(spec.driver, &prior)?;
            }
            let session = prior
                .native_session_id
                .ok_or_else(|| failure(ErrorCode::Unsupported, "Previous native session identity was not saved"))?;
            let task = manager.task(&prior.task_id).map_err(|e| failure(ErrorCode::OutcomeUnknown, e))?;
            let state = task.state.lock();
            if !state.delegated_read_only
                || state.task.provider != *agent_id
                || state.session_id.as_deref() != Some(session.as_str())
            {
                return Err(failure(ErrorCode::Conflict, "Original native task or session is unavailable"));
            }
            (prior.task_id, Some(session), prior.codex_scoped_tools)
        } else {
            (
                blake3::hash(operation.as_bytes()).to_hex().to_string(),
                None,
                writable && spec.driver == crate::provider::Driver::Codex,
            )
        };
        let admission = store.reserve(Admission {
            requested_config: choices
                .iter()
                .filter(|choice| choice.id != "mode")
                .map(|choice| (choice.id.clone(), choice.value.clone()))
                .collect(),
            parent_session_id: snapshot.caller.session_id.clone(),
            root_session_id: snapshot.caller.root_session_id.clone(),
            delegation_id: snapshot.id.clone(),
            operation_id: operation,
            payload_hash,
            task_id: task_id.clone(),
            activation_id: snapshot.activation_id.clone(),
            request_id: snapshot.activation_id.clone(),
            codex_scoped_tools,
            host_turn_id: None,
            native_session_id,
            phase: AdmissionPhase::Reserved,
            completion: None,
        })?;
        if manager.tasks.read().contains_key(&task_id) {
            return Ok(admission);
        }
        if manager.shared.paths.store_dir.join(format!("{task_id}.json")).exists() {
            return Err(failure(
                ErrorCode::OutcomeUnknown,
                "Prepared task exists on disk; reopen the project before retrying",
            ));
        }
        manager
            .create_with_task_id(
                NewAgentTask {
                    project_root: Some(IpcPath(cwd)),
                    prompt: AgentPrompt { text: snapshot.request.prompt.clone(), ..Default::default() },
                    provider: agent_id.clone(),
                    config: choices,
                    isolation: Isolation::Local,
                    idle: true,
                    ..Default::default()
                },
                Some(task_id),
            )
            .await
            .map_err(|e| failure(ErrorCode::StorageFailed, e))?;
        Ok(admission)
    }
}

pub(super) fn check_scoped_tools(driver: crate::provider::Driver, intent: &Admission) -> DelegationResult<()> {
    if driver == crate::provider::Driver::Codex && !intent.codex_scoped_tools {
        return Err(failure(ErrorCode::Unsupported, "This Codex thread has no OpenIDEA file tools"));
    }
    Ok(())
}

fn failure(code: ErrorCode, message: impl std::fmt::Display) -> DelegationError {
    DelegationError::new(code, message.to_string())
}
