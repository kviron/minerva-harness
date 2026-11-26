use super::*;
use crate::sessions::SessionService;
use minerva_plugin_sdk::tools::PendingInteraction;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuestionState {
    Pending,
    Delivering,
    Delivered,
    Expired,
    Unknown,
}

impl QuestionState {
    fn parse(value: &str) -> DelegationResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "delivering" => Ok(Self::Delivering),
            "delivered" => Ok(Self::Delivered),
            "expired" => Ok(Self::Expired),
            "unknown" => Ok(Self::Unknown),
            _ => Err(storage("Invalid stored delegation question state")),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationQuestion {
    pub activation_id: String,
    pub interaction: PendingInteraction,
    pub state: QuestionState,
}

pub struct AnswerClaim {
    pub question: DelegationQuestion,
    pub fresh: bool,
}

struct StoredAnswer {
    operation: Option<String>,
    actor: Option<String>,
    text: Option<String>,
}

fn read_question(db: &Connection, child: &str, id: &str) -> DelegationResult<(DelegationQuestion, StoredAnswer)> {
    let (activation_id, json, state, operation, actor, text): (String, String, String, _, _, _) = db
        .query_row(
            "SELECT activation_id,question_json,state,operation_id,answered_by,answer
             FROM delegation_questions WHERE delegation_id=?1 AND id=?2",
            params![child, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| DelegationError::new(ErrorCode::Unavailable, "Question not found"))?;
    let interaction = serde_json::from_str(&json).map_err(storage)?;
    Ok((
        DelegationQuestion { activation_id, interaction, state: QuestionState::parse(&state)? },
        StoredAnswer { operation, actor, text },
    ))
}

fn verify_live_binding(
    db: &Connection,
    child: &DelegationSnapshot,
    activation: &str,
    question: &PendingInteraction,
) -> DelegationResult<()> {
    tree::ensure_open(db, &child.caller.root_session_id)?;
    if child.state != ActivationState::Running
        || child.activation_id != activation
        || !matches!(child.request.target, Target::Minerva { .. })
        || child
            .handle
            .as_ref()
            .is_none_or(|handle| handle.session_id != question.session_id || handle.run_id != question.run_id)
        || question.kind != "question"
    {
        return Err(conflict("Question does not belong to the current running child"));
    }
    Ok(())
}

impl SessionService {
    /// Reconciles a stored question whose exact broker wait has ended.
    pub fn expire_delegation_question(&self, child: &str, activation: &str, id: &str) -> DelegationResult<()> {
        self.store.with_delegation_db(|db| {
            db.execute(
                "UPDATE delegation_questions SET state='expired'
                 WHERE delegation_id=?1 AND activation_id=?2 AND id=?3 AND state='pending'",
                params![child, activation, id],
            )
            .map_err(storage)?;
            Ok(())
        })
    }

    pub fn list_delegation_questions(&self, caller: &Caller, child: &str) -> DelegationResult<Vec<DelegationQuestion>> {
        self.store.with_delegation_db(|db| {
            authorized(caller, &read(db, child)?)?;
            let mut query = db
                .prepare("SELECT id FROM delegation_questions WHERE delegation_id=?1 ORDER BY rowid")
                .map_err(storage)?;
            let ids = query.query_map([child], |row| row.get::<_, String>(0)).map_err(storage)?;
            ids.map(|row| read_question(db, child, &row.map_err(storage)?).map(|(question, _)| question)).collect()
        })
    }

    pub fn publish_delegation_question(
        &self,
        child: &str,
        activation: &str,
        question: &PendingInteraction,
    ) -> DelegationResult<()> {
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            verify_live_binding(&tx, &read(&tx, child)?, activation, question)?;
            let json = serde_json::to_string(question).map_err(storage)?;
            let inserted = tx
                .execute(
                    "INSERT INTO delegation_questions(id,delegation_id,activation_id,question_json)
                 VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING",
                    params![question.id, child, activation, json],
                )
                .map_err(storage)?;
            if inserted == 0 {
                let existing: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM delegation_questions
                     WHERE id=?1 AND delegation_id=?2 AND activation_id=?3 AND question_json=?4)",
                        params![question.id, child, activation, json],
                        |r| r.get(0),
                    )
                    .map_err(storage)?;
                if !existing {
                    return Err(conflict("Question ID already belongs to another interaction"));
                }
            }
            tx.commit().map_err(storage)
        })
    }

    pub fn delegation_question(&self, caller: &Caller, child: &str, id: &str) -> DelegationResult<DelegationQuestion> {
        self.store.with_delegation_db(|db| {
            authorized(caller, &read(db, child)?)?;
            read_question(db, child, id).map(|(question, _)| question)
        })
    }

    pub fn claim_delegation_answer(
        &self,
        caller: &Caller,
        child: &str,
        id: &str,
        operation: &str,
        answer: &str,
    ) -> DelegationResult<AnswerClaim> {
        if operation.trim().is_empty() || operation.len() > 200 || answer.trim().is_empty() || answer.len() > 65536 {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid question answer or operation ID"));
        }
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let child_snapshot = read(&tx, child)?;
            authorized(caller, &child_snapshot)?;
            let (mut question, stored) = read_question(&tx, child, id)?;
            if stored.operation.is_some() {
                if stored.operation.as_deref() == Some(operation) && stored.actor.as_deref() == Some(&caller.session_id)
                    && stored.text.as_deref() == Some(answer)
                {
                    return Ok(AnswerClaim { question, fresh: false });
                }
                return Err(conflict("Question already has a different answer operation"));
            }
            if question.state != QuestionState::Pending {
                return Err(conflict("Question is no longer pending"));
            }
            verify_live_binding(&tx, &child_snapshot, &question.activation_id, &question.interaction)?;
            if !question.interaction.options.is_empty() && !question.interaction.options.iter().any(|value| value == answer) {
                return Err(DelegationError::new(ErrorCode::Invalid, "Answer is not one of the question options"));
            }
            tx.execute(
                "UPDATE delegation_questions SET state='delivering',operation_id=?2,answered_by=?3,answer=?4 WHERE id=?1",
                params![id, operation, caller.session_id, answer],
            ).map_err(storage)?;
            tx.commit().map_err(storage)?;
            question.state = QuestionState::Delivering;
            Ok(AnswerClaim { question, fresh: true })
        })
    }

    pub fn settle_delegation_answer(&self, child: &str, id: &str, operation: &str, sent: bool) -> DelegationResult<()> {
        self.store.with_delegation_db(|db| {
            let state = if sent { "delivered" } else { "expired" };
            let changed = db
                .execute(
                    "UPDATE delegation_questions SET state=?4
                 WHERE delegation_id=?1 AND id=?2 AND operation_id=?3 AND state='delivering'",
                    params![child, id, operation, state],
                )
                .map_err(storage)?;
            if changed != 1 {
                return Err(conflict("Answer operation is no longer delivering"));
            }
            Ok(())
        })
    }
}
