use super::*;
use minerva_delegation::{limits::Limits, repository::Admission};
use rusqlite::params;

pub(super) fn lookup(
    db: &Connection,
    caller: &Caller,
    request: &StartRequest,
) -> DelegationResult<Option<DelegationSnapshot>> {
    lookup_payload(db, caller, &request.operation_id, &operation_payload(request)?)
}

pub(super) fn lookup_payload(
    db: &Connection,
    caller: &Caller,
    operation_id: &str,
    expected_payload: &str,
) -> DelegationResult<Option<DelegationSnapshot>> {
    let query = "SELECT payload_json,delegation_id,activation_id FROM delegation_operations
        WHERE caller_session=?1 AND operation_id=?2";
    let prior: Option<(String, String, String)> = db
        .query_row(query, params![caller.session_id, operation_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()
        .map_err(storage)?;
    let Some((payload, id, activation)) = prior else {
        queue::ensure_operation_unused(db, caller, operation_id)?;
        return Ok(None);
    };
    if payload != expected_payload {
        return Err(conflict("Operation ID reused with different payload"));
    }
    let current = read(db, &id)?;
    let snapshot = if current.activation_id == activation {
        current
    } else {
        let json: String = db
            .query_row(
                "SELECT s.snapshot_json FROM delegation_terminal_snapshots s
             JOIN delegation_results r USING(activation_id) WHERE s.activation_id=?1 AND r.delegation_id=?2",
                params![activation, id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        serde_json::from_str(&json).map_err(storage)?
    };
    authorized(caller, &snapshot)?;
    Ok(Some(snapshot))
}

fn child_depth(db: &Connection, caller: &Caller, limits: &Limits) -> DelegationResult<u32> {
    let (resolved, depth) = resolve_caller(db, &caller.session_id, &caller.workspace_id, &caller.cwd)?;
    if resolved != *caller {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Invalid caller lineage"));
    }
    let depth = depth + 1;
    if depth > limits.max_depth {
        return Err(DelegationError::new(ErrorCode::Busy, "Delegation depth exhausted"));
    }
    let query = "SELECT count(*),coalesce(sum(active),0) FROM delegation_children WHERE root_session=?1";
    let (total, active): (i64, i64) =
        db.query_row(query, [&caller.root_session_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(storage)?;
    if total >= limits.children_per_root as i64 || active >= limits.active_per_root as i64 {
        return Err(DelegationError::new(ErrorCode::Busy, "Delegation capacity exhausted"));
    }
    Ok(depth)
}

pub(super) fn reserve(
    db: &mut Connection,
    caller: &Caller,
    request: &StartRequest,
    limits: &Limits,
    fallback: Option<(&Target, &str)>,
) -> DelegationResult<Admission> {
    let tx = db.transaction().map_err(storage)?;
    if let Some(snapshot) = lookup(&tx, caller, request)? {
        return Ok(Admission { snapshot, fresh: false });
    }
    tree::ensure_open(&tx, &caller.root_session_id)?;
    let depth = child_depth(&tx, caller, limits)?;
    let id: String = tx.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0)).map_err(storage)?;
    let mut effective = request.clone();
    let fallback = fallback.map(|(target, reason)| {
        effective.target = target.clone();
        FallbackSelection { requested_target: request.target.clone(), reason: reason.to_owned() }
    });
    let snapshot = DelegationSnapshot {
        work_item_id: format!("work_{id}"),
        activation_id: format!("act_{id}"),
        id: id.clone(),
        caller: caller.clone(),
        request: effective,
        fallback,
        depth,
        state: ActivationState::Starting,
        previous_handle: None,
        handle: None,
        completion: None,
    };
    tx.execute(
        "INSERT INTO delegation_children(id,parent_session,root_session,activation_id,snapshot_json,active,admitted_at_ms)
         VALUES(?1,?2,?3,?4,?5,1,CAST(unixepoch('subsec')*1000 AS INTEGER))",
        params![
            id,
            caller.session_id,
            caller.root_session_id,
            snapshot.activation_id,
            serde_json::to_string(&snapshot).map_err(storage)?
        ],
    )
    .map_err(storage)?;
    record_operation(&tx, &snapshot, &operation_payload(request)?)?;
    tx.commit().map_err(storage)?;
    Ok(Admission { snapshot, fresh: true })
}

pub(super) fn record_operation(db: &Connection, snapshot: &DelegationSnapshot, payload: &str) -> DelegationResult<()> {
    db.execute(
        "INSERT INTO delegation_operations(caller_session,operation_id,payload_json,delegation_id,activation_id)
         VALUES(?1,?2,?3,?4,?5)",
        params![
            snapshot.caller.session_id,
            snapshot.request.operation_id,
            payload,
            snapshot.id,
            snapshot.activation_id
        ],
    )
    .map_err(storage)?;
    Ok(())
}

fn operation_payload(request: &StartRequest) -> DelegationResult<String> {
    let mut payload = request.clone();
    payload.tool_registrations.clear();
    payload.origin_run = None;
    serde_json::to_string(&payload).map_err(storage)
}
