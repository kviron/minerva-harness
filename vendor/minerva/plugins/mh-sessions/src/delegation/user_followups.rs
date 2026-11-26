use super::*;
use minerva_delegation::IDE_USER_OPERATION_PREFIX;
use rusqlite::params;

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserFollowup {
    pub id: String,
    pub prompt: String,
    pub state: UserFollowupState,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UserFollowupState {
    Queued,
    Requested,
    Unknown,
}

const MAX_VISIBLE_FOLLOWUPS: i64 = 32;

impl crate::sessions::SessionService {
    pub fn list_user_delegation_followups(&self, caller: &Caller, id: &str) -> DelegationResult<Vec<UserFollowup>> {
        self.store.with_delegation_db(|db| {
            let child = read(db, id)?;
            if caller.session_id != caller.root_session_id {
                return Err(DelegationError::new(
                    ErrorCode::Forbidden,
                    "Only the root conversation can view user follow-ups",
                ));
            }
            authorized(caller, &child)?;
            let active_run = (child.state == ActivationState::Running)
                .then(|| child.handle.as_ref().map(|handle| handle.run_id.as_str()))
                .flatten();
            let mut query = db
                .prepare(
                    "SELECT work_item_id, request_json, preferred_run, delivery_state
                 FROM delegation_followup_queue
                 WHERE delegation_id=?1 AND substr(operation_id,1,length(?2))=?2
                   AND cancelled=0 AND activation_id IS NULL AND delivery_state IS NOT 'delivered'
                 ORDER BY (delivery_state IS 'unknown'), sequence LIMIT ?3",
                )
                .map_err(storage)?;
            let rows = query
                .query_map(params![id, IDE_USER_OPERATION_PREFIX, MAX_VISIBLE_FOLLOWUPS], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(storage)?;
            rows.map(|row| {
                let (id, raw, preferred, delivery) = row.map_err(storage)?;
                let request: StartRequest = serde_json::from_str(&raw).map_err(storage)?;
                let state = match delivery.as_deref() {
                    Some("unknown") => UserFollowupState::Unknown,
                    Some("claimed") => UserFollowupState::Requested,
                    _ if active_run.is_some() && preferred.as_deref() == active_run => UserFollowupState::Requested,
                    _ => UserFollowupState::Queued,
                };
                Ok(UserFollowup { id, prompt: request.prompt, state })
            })
            .collect()
        })
    }
}
