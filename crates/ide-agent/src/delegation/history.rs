use super::AdmissionStore;
use crate::{AgentEntry, AgentManager};
use ide_core::{IdeError, IdeResult};

impl AgentManager {
    pub(crate) fn delegated_history(
        &self,
        root: &str,
        parent: &str,
        child: &str,
    ) -> IdeResult<Option<Vec<AgentEntry>>> {
        if !self.shared.paths.store_dir.join("delegation/admissions.json").is_file() {
            return Ok(None);
        }
        let store =
            AdmissionStore::open(&self.shared.paths.store_dir).map_err(|e| IdeError::unavailable(e.to_string()))?;
        let Some(admission) = store.for_delegation(root, parent, child) else {
            return Ok(None);
        };
        let task = self.task(&admission.task_id)?;
        let state = task.state.lock();
        if !state.delegated_read_only {
            return Err(IdeError::invalid("Native history ownership does not match delegated task"));
        }
        Ok(Some(state.conversation.main.entries.clone()))
    }
}
