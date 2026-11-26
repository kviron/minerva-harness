use super::*;
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreferredFollowup {
    pub work_item_id: String,
    pub prompt: String,
}

impl crate::sessions::SessionService {
    pub fn claim_delegation_followup(
        &self,
        handle: &ExecutionHandle,
        work_item_id: &str,
    ) -> DelegationResult<Option<String>> {
        let Some(child_id) = handle.session_id.strip_prefix("del_") else {
            return Ok(None);
        };
        self.store.claim_delegation_followup(child_id, handle, work_item_id)
    }

    pub fn confirm_delegation_followup(&self, handle: &ExecutionHandle, work_item_id: &str) -> DelegationResult<bool> {
        let Some(child_id) = handle.session_id.strip_prefix("del_") else {
            return Ok(false);
        };
        self.store.confirm_delegation_followup(child_id, handle, work_item_id)
    }

    pub fn prefer_delegation_followup_in_run(
        &self,
        child_id: &str,
        handle: &ExecutionHandle,
        work_item_id: &str,
    ) -> DelegationResult<bool> {
        self.store.prefer_delegation_followup_in_run(child_id, handle, work_item_id)
    }

    pub fn preferred_delegation_followup(
        &self,
        handle: &ExecutionHandle,
    ) -> DelegationResult<Option<PreferredFollowup>> {
        let Some(child_id) = handle.session_id.strip_prefix("del_") else {
            return Ok(None);
        };
        self.store.with_delegation_db(|db| {
            if !steering_run_is_open(db, child_id, handle)? {
                return Ok(None);
            }
            let pending: Option<(String, String)> = db
                .query_row(
                    "SELECT work_item_id,request_json FROM delegation_followup_queue
                 WHERE delegation_id=?1 AND cancelled=0 AND activation_id IS NULL
                   AND delivery_run IS NULL AND preferred_run=?2
                 ORDER BY sequence LIMIT 1",
                    params![child_id, handle.run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            pending
                .map(|(work_item_id, raw)| {
                    let request: StartRequest = serde_json::from_str(&raw).map_err(storage)?;
                    Ok(PreferredFollowup { work_item_id, prompt: request.prompt })
                })
                .transpose()
        })
    }
}

impl SessionStore {
    pub fn prefer_delegation_followup_in_run(
        &self,
        child_id: &str,
        handle: &ExecutionHandle,
        work_item_id: &str,
    ) -> DelegationResult<bool> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            if !steering_run_is_open(&tx, child_id, handle)? {
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE delegation_followup_queue SET preferred_run=?3
                 WHERE work_item_id=?1 AND delegation_id=?2 AND cancelled=0
                   AND activation_id IS NULL
                   AND (preferred_run IS NULL OR preferred_run=?3)
                   AND (delivery_run IS NULL OR
                        (delivery_run=?3 AND delivery_state IN ('claimed','delivered')))",
                    params![work_item_id, child_id, handle.run_id],
                )
                .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(changed == 1)
        })
    }

    pub fn claim_delegation_followup(
        &self,
        child_id: &str,
        handle: &ExecutionHandle,
        work_item_id: &str,
    ) -> DelegationResult<Option<String>> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            if !steering_run_is_open(&tx, child_id, handle)? {
                return Ok(None);
            }
            let request: Option<String> = tx
                .query_row(
                    "UPDATE delegation_followup_queue SET delivery_run=?3,delivery_state='claimed'
                     WHERE work_item_id=?1 AND delegation_id=?2 AND cancelled=0
                       AND activation_id IS NULL AND delivery_run IS NULL AND preferred_run=?3
                     RETURNING request_json",
                    params![work_item_id, child_id, handle.run_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            let request = request.map(|raw| serde_json::from_str::<StartRequest>(&raw)).transpose().map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(request.map(|request| request.prompt))
        })
    }

    pub fn confirm_delegation_followup(
        &self,
        child_id: &str,
        handle: &ExecutionHandle,
        work_item_id: &str,
    ) -> DelegationResult<bool> {
        self.with_delegation_db(|db| {
            let child = read(db, child_id)?;
            if child.handle.as_ref() != Some(handle) {
                return Ok(false);
            }
            db.execute(
                "UPDATE delegation_followup_queue SET delivery_state='delivered'
                 WHERE work_item_id=?1 AND delegation_id=?2 AND delivery_run=?3 AND delivery_state='claimed'",
                params![work_item_id, child_id, handle.run_id],
            )
            .map(|changed| changed == 1)
            .map_err(storage)
        })
    }
}

fn steering_run_is_open(db: &Connection, child_id: &str, handle: &ExecutionHandle) -> DelegationResult<bool> {
    let child = read(db, child_id)?;
    if child.state != ActivationState::Running
        || child.handle.as_ref() != Some(handle)
        || !matches!(child.request.target, Target::Minerva { .. })
    {
        return Ok(false);
    }
    tree::ensure_open(db, &child.caller.root_session_id)?;
    Ok(true)
}
