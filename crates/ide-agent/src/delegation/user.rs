use crate::{AgentDelegationReceipt, AgentManager, provider::Driver};
use ide_core::{IdeError, IdeResult};
use serde_json::{Value, json};

impl AgentManager {
    pub async fn send_delegation_followup(
        &self,
        id: &str,
        subagent: &str,
        operation: &str,
        prompt: String,
    ) -> IdeResult<AgentDelegationReceipt> {
        let task = self.task(id)?;
        let _settings = task.setting_changes.lock().await;
        let child = subagent
            .strip_prefix("del_")
            .filter(|child| !child.is_empty())
            .ok_or_else(|| IdeError::invalid("the child does not support Minerva follow-ups"))?;
        if !task.uses_minerva_driver() {
            return Err(IdeError::invalid("this agent does not support Minerva delegation"));
        }
        let available_session = || {
            let state = task.state.lock();
            if state.task.status == crate::AgentStatus::Archived || state.conversation.parent_of(subagent).is_none() {
                return Err(IdeError::unavailable("the delegated child is unavailable"));
            }
            state.session_id.clone().ok_or_else(|| IdeError::unavailable("the task has no session"))
        };
        let saved = available_session()?;
        let live = task.ensure_session().await.map_err(IdeError::internal)?;
        if live.driver != Driver::Minerva || live.session != saved || available_session()? != saved {
            return Err(IdeError::unavailable("the original Minerva session is unavailable"));
        }
        let receipt: IdeResult<Value> = live
            .peer
            .request(
                "_openidea/delegation_followup",
                json!({
                    "sessionId":live.session,"childId":child,"operationId":operation,"prompt":prompt
                }),
                None,
            )
            .await
            .map_err(|error| IdeError::internal(task.describe(error)));
        task.release_when_idle();
        let receipt = receipt?;
        match receipt["disposition"].as_str() {
            Some("queued") => {
                if receipt["cancelled"] == true {
                    return Err(IdeError::unavailable("the follow-up was cancelled"));
                }
                Ok(AgentDelegationReceipt {
                    queued: receipt["activationId"].is_null(),
                    work_item_id: receipt["workItemId"]
                        .as_str()
                        .ok_or_else(|| IdeError::internal("invalid queued receipt"))?
                        .to_owned(),
                })
            }
            Some("activation") => Ok(AgentDelegationReceipt {
                queued: false,
                work_item_id: receipt["workItemId"]
                    .as_str()
                    .ok_or_else(|| IdeError::internal("invalid activation receipt"))?
                    .to_owned(),
            }),
            _ => Err(IdeError::internal("invalid delegation receipt")),
        }
    }
}

impl AgentManager {
    /// Requests delivery of an existing queued follow-up at the child's next model step.
    pub async fn prefer_delegation_followup(
        &self,
        id: &str,
        subagent: &str,
        work_item: &str,
        expected_run: &str,
    ) -> IdeResult<bool> {
        let task = self.task(id)?;
        let _settings = task.setting_changes.lock().await;
        let child = subagent
            .strip_prefix("del_")
            .filter(|id| !id.is_empty())
            .ok_or_else(|| IdeError::invalid("the child does not support Minerva follow-ups"))?;
        if !task.uses_minerva_driver() || work_item.is_empty() || expected_run.is_empty() {
            return Err(IdeError::invalid("invalid Minerva follow-up delivery request"));
        }
        let session = {
            let state = task.state.lock();
            if state.task.status == crate::AgentStatus::Archived || state.conversation.parent_of(subagent).is_none() {
                return Err(IdeError::unavailable("the delegated child is unavailable"));
            }
            state.session_id.clone().ok_or_else(|| IdeError::unavailable("the task has no session"))?
        };
        let live = task.ensure_session().await.map_err(IdeError::internal)?;
        if live.driver != Driver::Minerva
            || live.session != session
            || task.state.lock().session_id.as_deref() != Some(&session)
        {
            return Err(IdeError::unavailable("the original Minerva session is unavailable"));
        }
        let result: IdeResult<bool> = live
            .peer
            .request(
                "_openidea/delegation_prefer_followup",
                json!({"sessionId":session,"childId":child,"workItemId":work_item,"expectedRunId":expected_run}),
                None,
            )
            .await
            .map_err(|error| IdeError::internal(task.describe(error)));
        task.release_when_idle();
        result
    }
}

impl AgentManager {
    pub(crate) async fn guard_delegation_removal(&self, task: &std::sync::Arc<crate::Task>) -> IdeResult<()> {
        if !task.uses_minerva_driver() {
            return Ok(());
        }
        let saved = {
            let state = task.state.lock();
            match &state.session_id {
                Some(id) => id.clone(),
                None if state.conversation.saved_subagents().is_empty() => return Ok(()),
                None => {
                    return Err(IdeError::unavailable(
                        "The delegated tree has no recoverable parent session; reconcile its children before removing this task",
                    ));
                }
            }
        };
        let live = task
            .ensure_session()
            .await
            .map_err(|error| IdeError::unavailable(format!("Cannot verify delegated work before removal: {error}")))?;
        if live.driver != Driver::Minerva || live.session != saved {
            return Err(IdeError::unavailable("The original Minerva session could not be verified before removal"));
        }
        let result: IdeResult<Value> = live
            .peer
            .request("_openidea/prepare_task_removal", json!({"sessionId": saved}), None)
            .await
            .map_err(|error| IdeError::unavailable(task.describe(error)));
        if result.is_err() {
            task.release_when_idle();
        }
        result.map(|_| ())
    }
}
