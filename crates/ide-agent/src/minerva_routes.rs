//! Idle API-provider rebinding keeps the existing Minerva session.
use super::*;

impl AgentManager {
    /// The caller holds setting_changes; notification ownership precedes state locks.
    pub(super) async fn switch_minerva_provider(&self, id: &str, provider: String) -> IdeResult<()> {
        let task = self.task(id)?;
        let _connecting = task.connecting.lock().await;
        let mut events = task.connection_events.lock();
        let file = task.file.lock();
        if file.deleted {
            return Err(IdeError::not_found("task deleted"));
        }
        let providers = self.shared.minerva_providers.read();
        let target = providers
            .iter()
            .find(|spec| spec.id == provider && spec.driver == provider::Driver::Minerva)
            .ok_or_else(|| IdeError::invalid("Unknown Minerva API provider"))?;
        let mut state = task.state.lock();
        if state.busy
            || state.discovering_model.is_some()
            || !state.background_tasks.is_empty()
            || !task.permissions.lock().is_empty()
        {
            return Err(IdeError::invalid("wait until the agent is done"));
        }
        if state.task.status == AgentStatus::Archived {
            return Err(IdeError::invalid("the task is archived"));
        }
        self.shared
            .saved_task_spec(&state.task.provider)
            .filter(|spec| spec.driver == provider::Driver::Minerva)
            .ok_or_else(|| IdeError::invalid("Only Minerva API sessions can change providers"))?;
        if state.task.provider == target.id {
            return Ok(());
        }
        if state.session_id.is_none() {
            return Err(IdeError::invalid("Start the conversation first"));
        }
        let previous = state.task.clone();
        let previous_wanted = state.wanted.clone();
        let previous_pending = state.pending_model.clone();
        let mut removed: HashSet<_> = state
            .task
            .config
            .iter()
            .filter(|option| option.category.as_deref() == Some("model") || config::is_per_model_option(option))
            .map(|option| option.id.clone())
            .collect();
        if let Some(pending) = &state.pending_model {
            removed.insert(pending.model.id.clone());
            removed.extend(pending.options.iter().map(|option| option.id.clone()));
        }
        removed.insert("model".into());
        removed.insert("reasoning_effort".into());
        state.task.config.retain(|option| !removed.contains(&option.id));
        state.wanted.retain(|choice| !removed.contains(&choice.id));
        let catalog = self.shared.paths.catalogs.cached(&self.shared, &provider);
        for mut option in catalog.config.into_iter().filter(|option| option.category.as_deref() == Some("model")) {
            option.current = state.task.model.clone();
            state.task.config.push(option);
        }
        state.task.provider = provider;
        state.pending_model = None;
        if let Err(error) = task.write_snapshot(&Task::snapshot(&state)) {
            state.task.provider = previous.provider;
            state.task.config = previous.config;
            state.wanted = previous_wanted;
            state.pending_model = previous_pending;
            return Err(IdeError::unavailable(error));
        }
        state.model_choice += 1;
        *events += 1;
        task.config_sent.lock().clear();
        if let Some(live) = task.live.lock().take() {
            live.peer.kill();
        }
        drop(state);
        drop(providers);
        drop(file);
        task.emit_task();
        Ok(())
    }
}

impl Task {
    pub(super) fn uses_minerva_driver(&self) -> bool {
        let provider = self.state.lock().task.provider.clone();
        self.shared.saved_task_spec(&provider).is_some_and(|spec| spec.driver == provider::Driver::Minerva)
    }

    pub(super) fn validate_minerva_route(&self) -> IdeResult<()> {
        let providers = self.shared.minerva_providers.read();
        let state = self.state.lock();
        if self.shared.saved_task_spec(&state.task.provider).is_none_or(|spec| spec.driver != provider::Driver::Minerva)
        {
            return Ok(());
        }
        let Some(id) = state.task.provider.strip_prefix("minerva.api.") else { return Ok(()) };
        if !providers.iter().any(|spec| spec.id == state.task.provider) {
            return Err(IdeError::unavailable("Select an available Minerva API provider"));
        }
        let prefix = format!("api:{id}/");
        let model = state
            .task
            .model
            .as_deref()
            .filter(|model| model.starts_with(&prefix))
            .ok_or_else(|| IdeError::invalid("Select a model from the current Minerva provider"))?;
        if state
            .task
            .config
            .iter()
            .filter(|option| option.category.as_deref() == Some("model"))
            .any(|option| !option.values.iter().any(|value| value.value == model))
        {
            return Err(IdeError::invalid("The selected Minerva model is unavailable"));
        }
        Ok(())
    }
}

/// Ephemeral rollback data for a model-scoped settings transition.
pub(super) struct ModelChoiceRollback {
    wanted: Vec<AgentConfigChoice>,
    sent: HashSet<(String, String)>,
    scoped_ids: HashSet<String>,
}

impl Task {
    pub(super) fn clear_minerva_model_choices(&self, state: &mut TaskState) -> ModelChoiceRollback {
        let mut ids: HashSet<String> = state
            .task
            .config
            .iter()
            .filter(|option| config::is_per_model_option(option))
            .map(|option| option.id.clone())
            .collect();
        ids.insert("reasoning_effort".into());
        let mut sent = self.config_sent.lock();
        let rollback = ModelChoiceRollback {
            wanted: state.wanted.clone(),
            sent: sent.iter().filter(|(id, _)| ids.contains(id)).cloned().collect(),
            scoped_ids: ids.clone(),
        };
        state.wanted.retain(|choice| !ids.contains(&choice.id));
        sent.retain(|(id, _)| !ids.contains(id));
        rollback
    }
}

impl ModelChoiceRollback {
    pub(super) fn restore(&self, state: &mut TaskState, sent: &mut HashSet<(String, String)>) {
        state.wanted = self.wanted.clone();
        sent.retain(|(id, _)| !self.scoped_ids.contains(id));
        sent.extend(self.sent.iter().cloned());
    }
}
