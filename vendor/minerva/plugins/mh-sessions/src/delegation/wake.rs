use super::*;
use rusqlite::Transaction;

pub(crate) enum AdmissionOrigin {
    Existing,
    User,
    DelegatedChild,
    Wake { caller: Caller, generation: i64, question: Option<WakeQuestion> },
}

#[derive(Clone)]
pub struct WakeQuestion {
    pub child_id: String,
    pub question_id: String,
}

pub(crate) fn admit(
    tx: &Transaction<'_>,
    request: &crate::StartRunRequest,
    origin: &AdmissionOrigin,
) -> Result<(), String> {
    match origin {
        AdmissionOrigin::Existing => Ok(()),
        AdmissionOrigin::DelegatedChild => {
            let id = request.session_id.strip_prefix("del_").ok_or("Invalid delegated child session")?;
            let child = read(tx, id).map_err(|error| error.to_string())?;
            let Target::Minerva { route, effort } = &child.request.target else {
                return Err("Delegated child is not a Minerva target".into());
            };
            let previous_run = child.previous_handle.as_ref().map(|handle| handle.run_id.as_str());
            let registrations = serde_json::to_value(&child.request.tool_registrations).map_err(|e| e.to_string())?;
            if child.state != ActivationState::Starting
                || child.activation_id != request.request_id
                || route != &request.model
                || effort.as_deref() != request.options["reasoningEffort"].as_str()
                || child.request.prompt != request.message.text
                || request.message.id != format!("{}_input", child.activation_id)
                || request.parent_id.as_deref() != previous_run
                || request.options["requireCurrentHead"].as_bool() != Some(previous_run.is_some())
                || request.options["liveSelection"].as_bool() != Some(false)
                || request.options["delegationToolRegistrations"] != registrations
            {
                return Err("Run does not match the starting delegation activation".into());
            }
            tree::ensure_open(tx, &child.caller.root_session_id).map_err(|error| error.to_string())
        }
        AdmissionOrigin::User => {
            tx.execute("DELETE FROM delegation_stopped_trees WHERE root_session=?1", [&request.session_id])
                .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO delegation_wake_state VALUES(?1,1,0) ON CONFLICT(session_id)
                 DO UPDATE SET generation=generation+1,stopped=0",
                [&request.session_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        AdmissionOrigin::Wake { caller, generation, question } => {
            if request.session_id != caller.session_id || !request.model.starts_with("api:") {
                return Err("Invalid wake target".into());
            }
            let resolved =
                resolve_caller(tx, &caller.session_id, &caller.workspace_id, &caller.cwd).map_err(|e| e.to_string())?.0;
            if resolved != *caller {
                return Err("Invalid wake caller".into());
            }
            tree::ensure_open(tx, &caller.root_session_id).map_err(|e| e.to_string())?;
            let current: Option<(i64, bool)> = tx
                .query_row(
                    "SELECT generation,stopped FROM delegation_wake_state WHERE session_id=?1",
                    [&caller.session_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if current.unwrap_or((0, false)) != (*generation, false) {
                return Err("Wake rejected by user-stop generation".into());
            }
            let head: Option<String> = tx
                .query_row(
                    "SELECT message_id FROM messages WHERE session_id=?1 ORDER BY seq DESC LIMIT 1",
                    [&caller.session_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if head != request.parent_id {
                return Err("Wake branch head changed".into());
            }
            if let Some(question) = question {
                verify_question(tx, caller, request, question)?;
            }
            Ok(())
        }
    }
}

fn verify_question(
    tx: &Transaction<'_>,
    caller: &Caller,
    request: &crate::StartRunRequest,
    question: &WakeQuestion,
) -> Result<(), String> {
    if request.request_id != format!("wake_question_{}", question.question_id) {
        return Err("Invalid question wake identity".into());
    }
    let child = read(tx, &question.child_id).map_err(|error| error.to_string())?;
    if child.caller != *caller || child.state != ActivationState::Running {
        return Err("Question wake child is not running in this conversation".into());
    }
    let Some(origin_run) = child.request.origin_run else { return Err("Question wake has no origin".into()) };
    let eligible: bool = tx
        .query_row(
            "WITH RECURSIVE ancestors(id) AS (
            SELECT ?2 UNION SELECT m.parent_id FROM messages m JOIN ancestors a ON m.message_id=a.id
            WHERE m.session_id=?1 AND m.parent_id IS NOT NULL
         )
         SELECT EXISTS(
            SELECT 1 FROM delegation_questions q JOIN ancestors a ON a.id=?3
            WHERE q.id=?4 AND q.delegation_id=?5 AND q.activation_id=?6 AND q.state='pending'
         )",
            rusqlite::params![
                caller.session_id,
                request.parent_id,
                origin_run,
                question.question_id,
                question.child_id,
                child.activation_id
            ],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !eligible {
        return Err("Question is no longer pending on this branch".into());
    }
    Ok(())
}

impl crate::sessions::SessionService {
    /// Trusted user action; cancellation of a tool call must not call this API.
    pub fn delegation_user_stop(&self, session: &str) -> DelegationResult<i64> {
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            tx.execute(
                "INSERT INTO delegation_wake_state VALUES(?1,1,1) ON CONFLICT(session_id)
                 DO UPDATE SET generation=generation+1,stopped=1",
                [session],
            )
            .map_err(storage)?;
            let generation = tx
                .query_row("SELECT generation FROM delegation_wake_state WHERE session_id=?1", [session], |r| r.get(0))
                .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(generation)
        })
    }

    pub fn delegation_wake_generation(&self, session: &str) -> DelegationResult<Option<i64>> {
        self.store.with_delegation_db(|db| {
            let state: Option<(i64, bool)> = db
                .query_row("SELECT generation,stopped FROM delegation_wake_state WHERE session_id=?1", [session], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()
                .map_err(storage)?;
            Ok(match state {
                Some((_, true)) => None,
                Some((generation, false)) => Some(generation),
                None => Some(0),
            })
        })
    }

    pub fn accept_user_run(&self, request: &crate::StartRunRequest, created_at: &str) -> Result<bool, String> {
        Self::validate_run_request(request)?;
        self.store.accept_run_origin(request, created_at, &AdmissionOrigin::User)
    }

    pub fn accept_delegated_child_run(
        &self,
        request: &crate::StartRunRequest,
        created_at: &str,
    ) -> Result<bool, String> {
        Self::validate_run_request(request)?;
        self.store.accept_run_origin(request, created_at, &AdmissionOrigin::DelegatedChild)
    }

    pub fn accept_wake_run(
        &self,
        caller: &Caller,
        generation: i64,
        request: &crate::StartRunRequest,
        created_at: &str,
    ) -> Result<bool, String> {
        Self::validate_run_request(request)?;
        self.store.accept_run_origin(
            request,
            created_at,
            &AdmissionOrigin::Wake { caller: caller.clone(), generation, question: None },
        )
    }

    pub fn accept_question_wake_run(
        &self,
        caller: &Caller,
        generation: i64,
        question: &WakeQuestion,
        request: &crate::StartRunRequest,
        created_at: &str,
    ) -> Result<bool, String> {
        Self::validate_run_request(request)?;
        self.store.accept_run_origin(
            request,
            created_at,
            &AdmissionOrigin::Wake { caller: caller.clone(), generation, question: Some(question.clone()) },
        )
    }
}
