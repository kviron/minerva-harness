use super::*;
use minerva_delegation::{limits::Limits, repository::QueuedFollowup};
use rusqlite::params;
use std::collections::BTreeMap;

pub(super) fn lookup(
    db: &Connection,
    caller: &Caller,
    id: &str,
    operation: &str,
    prompt: &str,
) -> DelegationResult<Option<QueuedFollowup>> {
    let prior: Option<(String, String, String, bool, Option<String>)> = db
        .query_row(
            "SELECT work_item_id,delegation_id,request_json,cancelled,activation_id FROM delegation_followup_queue
         WHERE caller_session=?1 AND operation_id=?2",
            params![caller.session_id, operation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(storage)?;
    let Some((work_item_id, saved_child, saved_request, cancelled, activation_id)) = prior else { return Ok(None) };
    followup::authorize_parent(db, caller, &read(db, &saved_child)?)?;
    let saved: StartRequest = serde_json::from_str(&saved_request).map_err(storage)?;
    if saved_child != id || saved.prompt != prompt {
        return Err(conflict("Operation ID reused with different payload"));
    }
    Ok(Some(QueuedFollowup { work_item_id, cancelled, activation_id }))
}

pub(super) fn pending_count(db: &Connection, id: &str) -> DelegationResult<i64> {
    db.query_row(
        "SELECT count(*) FROM delegation_followup_queue
         WHERE delegation_id=?1 AND cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL",
        [id],
        |row| row.get(0),
    )
    .map_err(storage)
}

pub(super) fn ensure_operation_unused(db: &Connection, caller: &Caller, operation: &str) -> DelegationResult<()> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM delegation_followup_queue WHERE caller_session=?1 AND operation_id=?2)",
            params![caller.session_id, operation],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if exists {
        return Err(conflict("Operation ID already belongs to a queued follow-up"));
    }
    Ok(())
}

impl SessionStore {
    pub fn enqueue_delegation_followup(
        &self,
        caller: &Caller,
        id: &str,
        request: &StartRequest,
        limits: &Limits,
    ) -> DelegationResult<QueuedFollowup> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let child = read(&tx, id)?;
            followup::authorize_parent(&tx, caller, &child)?;
            if let Some(prior) = lookup(&tx, caller, id, &request.operation_id, &request.prompt)? {
                return Ok(prior);
            }
            let operation_used: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM delegation_operations WHERE caller_session=?1 AND operation_id=?2)",
                params![caller.session_id, request.operation_id],
                |row| row.get(0),
            ).map_err(storage)?;
            if operation_used {
                return Err(conflict("Operation ID already belongs to an activation"));
            }
            tree::ensure_open(&tx, &caller.root_session_id)?;
            if child.request.target != request.target {
                return Err(DelegationError::new(ErrorCode::Unsupported, "Queue requires the existing execution target"));
            }
            if !matches!(child.state, ActivationState::Accepted | ActivationState::Starting | ActivationState::Running | ActivationState::Succeeded) {
                return Err(DelegationError::new(ErrorCode::Busy, "Child is not accepting queued follow-ups"));
            }
            let pending = pending_count(&tx, id)?;
            if pending >= limits.queued_per_child as i64 {
                return Err(DelegationError::new(ErrorCode::Busy, "Child follow-up queue is full"));
            }
            let work_item_id: String = tx.query_row("SELECT 'work_'||lower(hex(randomblob(16)))", [], |row| row.get(0))
                .map_err(storage)?;
            let mut scoped = request.clone();
            followup::restrict_tools(&child, &mut scoped);
            tx.execute(
                "INSERT INTO delegation_followup_queue(work_item_id,caller_session,operation_id,delegation_id,request_json)
                 VALUES(?1,?2,?3,?4,?5)",
                params![work_item_id, caller.session_id, request.operation_id, id, serde_json::to_string(&scoped).map_err(storage)?],
            ).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(QueuedFollowup { work_item_id, cancelled: false, activation_id: None })
        })
    }

    pub fn promote_delegation_followup(
        &self,
        caller: &Caller,
        id: &str,
        parent_reads: &BTreeMap<String, u64>,
        limits: &Limits,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let child = read(&tx, id)?;
            followup::authorize_parent(&tx, caller, &child)?;
            tree::ensure_open(&tx, &caller.root_session_id)?;
            if child.state != ActivationState::Succeeded {
                return Ok(None);
            }
            let next: Option<(String, String)> = tx
                .query_row(
                    "SELECT work_item_id,request_json FROM delegation_followup_queue
                 WHERE delegation_id=?1 AND cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL ORDER BY sequence LIMIT 1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((work_item_id, raw)) = next else { return Ok(None) };
            let mut request: StartRequest = serde_json::from_str(&raw).map_err(storage)?;
            request.tool_registrations.retain(|name, registration| parent_reads.get(name) == Some(registration));
            restrict_to_latest_parent_run(&tx, caller, &mut request)?;
            let snapshot = followup::prepare_activation(&tx, child, request, Some(work_item_id), limits)?;
            write(&tx, &snapshot)?;
            tx.execute(
                "UPDATE delegation_followup_queue SET activation_id=?2 WHERE work_item_id=?1",
                params![snapshot.work_item_id, snapshot.activation_id],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(Some(snapshot))
        })
    }
}

pub(super) fn restrict_to_latest_parent_run(
    db: &Connection,
    caller: &Caller,
    request: &mut StartRequest,
) -> DelegationResult<()> {
    restrict_to_latest_run(db, &caller.root_session_id, request)?;
    if caller.session_id != caller.root_session_id {
        restrict_to_latest_run(db, &caller.session_id, request)?;
    }
    Ok(())
}

fn restrict_to_latest_run(db: &Connection, session: &str, request: &mut StartRequest) -> DelegationResult<()> {
    let latest: Option<String> = db.query_row(
        "SELECT r.request_json FROM runs r JOIN messages m ON m.session_id=r.session_id AND m.message_id=r.parent_id
         WHERE r.session_id=?1 AND json_extract(m.extras_json,'$.\"minerva.origin\"') IS NOT 'manager'
         ORDER BY m.seq DESC LIMIT 1",
        [session], |row| row.get(0),
    ).optional().map_err(storage)?;
    let Some(raw) = latest else { return Ok(()) };
    let latest: crate::StartRunRequest = serde_json::from_str(&raw).map_err(storage)?;
    if let Some(allowlist) = latest.options.get("toolAllowlist") {
        let allowed: Vec<String> = serde_json::from_value(allowlist.clone()).map_err(storage)?;
        request.tool_registrations.retain(|name, _| allowed.contains(name));
    }
    if let Some(scope) = latest.options.get("delegationToolRegistrations") {
        let registrations: BTreeMap<String, u64> = serde_json::from_value(scope.clone()).map_err(storage)?;
        request.tool_registrations.retain(|name, registration| registrations.get(name) == Some(registration));
    }
    Ok(())
}

impl crate::sessions::SessionService {
    /// Tool registration identities are valid only for the runtime that admitted them.
    pub fn revoke_persisted_delegation_reads(&self) -> DelegationResult<()> {
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            tx.execute(
                "UPDATE delegation_children SET snapshot_json=json_remove(snapshot_json,'$.request.toolRegistrations')
                 WHERE json_extract(snapshot_json,'$.request.target.kind')='minerva'
                   AND json_type(snapshot_json,'$.request.toolRegistrations') IS NOT NULL",
                [],
            ).map_err(storage)?;
            tx.execute(
                "UPDATE delegation_followup_queue SET request_json=json_remove(request_json,'$.toolRegistrations')
                 WHERE cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL AND json_type(request_json,'$.toolRegistrations') IS NOT NULL",
                [],
            ).map_err(storage)?;
            tx.commit().map_err(storage)
        })
    }

    /// Only the local session engine can prove that no model request was started:
    /// it commits the exact run before contacting the provider.
    pub fn recoverable_queue_activation(&self, child: &DelegationSnapshot) -> DelegationResult<bool> {
        if child.state != ActivationState::Starting
            || child.handle.is_some()
            || !matches!(child.request.target, Target::Minerva { .. })
            || child.previous_handle.is_none()
        {
            return Ok(false);
        }
        self.store.with_delegation_db(|db| {
            db.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM delegation_followup_queue q
                   JOIN delegation_children c ON c.id=q.delegation_id AND c.activation_id=q.activation_id
                   WHERE c.id=?1 AND q.activation_id=?2 AND q.cancelled=0
                     AND json_extract(c.snapshot_json,'$.state')='starting'
                     AND NOT EXISTS(SELECT 1 FROM delegation_stopped_trees t WHERE t.root_session=c.root_session)
                     AND NOT EXISTS(SELECT 1 FROM runs r WHERE r.session_id='del_'||c.id AND r.id=q.activation_id)
                 )",
                params![child.id, child.activation_id],
                |row| row.get(0),
            )
            .map_err(storage)
        })
    }

    pub fn delegation_followups_ready(&self, limits: &Limits) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.store.with_delegation_db(|db| {
            let mut query = db
                .prepare(
                    "SELECT c.snapshot_json FROM delegation_children c
                 JOIN delegation_followup_queue q ON q.sequence=(
                     SELECT min(sequence) FROM delegation_followup_queue
                     WHERE delegation_id=c.id AND cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL
                 )
                 WHERE c.active=0 AND json_extract(c.snapshot_json,'$.state')='succeeded'
                   AND NOT EXISTS(SELECT 1 FROM delegation_stopped_trees t WHERE t.root_session=c.root_session)
                   AND (SELECT count(*) FROM delegation_children active
                        WHERE active.root_session=c.root_session AND active.active=1) < ?1
                 ORDER BY q.sequence LIMIT 64",
                )
                .map_err(storage)?;
            let rows =
                query.query_map([limits.active_per_root as i64], |row| row.get::<_, String>(0)).map_err(storage)?;
            rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage)).collect()
        })
    }
}
