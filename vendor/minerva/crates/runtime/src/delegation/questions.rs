use super::Resolver;
use crate::{plugins::PluginManager, sessions::SessionService, tools::InteractionBroker};
use mh_sessions::delegation::questions::QuestionState;
use minerva_delegation::{ActivationState, Caller, DelegationResult, DelegationSnapshot, Target};
use serde_json::{json, Value};

pub(super) fn reconcile(
    sessions: &SessionService,
    broker: &InteractionBroker,
    child: &DelegationSnapshot,
) -> DelegationResult<()> {
    if child.state != ActivationState::Running || !matches!(child.request.target, Target::Minerva { .. }) {
        return Ok(());
    }
    let Some(handle) = &child.handle else { return Ok(()) };
    let stored = sessions.list_delegation_questions(&child.caller, &child.id)?;
    let live = broker.list_questions(&handle.session_id, &handle.run_id);
    for question in &live {
        sessions.publish_delegation_question(&child.id, &child.activation_id, question)?;
    }
    for question in stored {
        if question.state == QuestionState::Pending
            && question.activation_id == child.activation_id
            && !live.iter().any(|item| item.id == question.interaction.id)
        {
            sessions.expire_delegation_question(&child.id, &question.activation_id, &question.interaction.id)?;
        }
    }
    Ok(())
}

impl Resolver {
    pub(super) fn list_questions(&self, caller: &Caller, id: &str) -> Result<Vec<Value>, String> {
        let plugins = self.plugins.upgrade().ok_or("Runtime stopped")?;
        let child = self.sessions.delegation_repository().get(caller, id).map_err(|e| e.to_string())?;
        reconcile(&self.sessions, &plugins.harness.interactions, &child).map_err(|e| e.to_string())?;
        let questions = self.sessions.list_delegation_questions(caller, id).map_err(|e| e.to_string())?;
        Ok(questions
            .into_iter()
            .filter(|question| question.state == QuestionState::Pending)
            .map(|question| {
                let item = question.interaction;
                json!({"id":item.id,"title":item.title,"prompt":item.prompt,"options":item.options})
            })
            .collect())
    }

    pub(super) fn answer_question(
        &self,
        caller: &Caller,
        child: &str,
        id: &str,
        operation: &str,
        answer: &str,
    ) -> Result<Value, String> {
        let plugins = self.plugins.upgrade().ok_or("Runtime stopped")?;
        let state = deliver(&self.sessions, &plugins, caller, child, id, operation, answer)?;
        Ok(json!({"id":id,"state":state}))
    }
}

impl crate::Runtime {
    pub fn answer_delegation_question(
        &self,
        caller: &Caller,
        child: &str,
        id: &str,
        operation: &str,
        answer: &str,
    ) -> Result<QuestionState, String> {
        deliver(&self.sessions, &self.plugins, caller, child, id, operation, answer)
    }
}

fn deliver(
    sessions: &SessionService,
    plugins: &PluginManager,
    caller: &Caller,
    child: &str,
    id: &str,
    operation: &str,
    answer: &str,
) -> Result<QuestionState, String> {
    let claim = sessions.claim_delegation_answer(caller, child, id, operation, answer).map_err(|e| e.to_string())?;
    let state = if claim.fresh {
        let sent = plugins.harness.interactions.answer_question(&claim.question.interaction, answer.into());
        sessions.settle_delegation_answer(child, id, operation, sent).map_err(|e| e.to_string())?;
        if sent {
            QuestionState::Delivered
        } else {
            QuestionState::Expired
        }
    } else {
        claim.question.state
    };
    Ok(state)
}
