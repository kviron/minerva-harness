use super::*;
use minerva_delegation::{limits::Limits, repository::Admission};

fn payload(id: &str, prompt: &str) -> DelegationResult<String> {
    serde_json::to_string(&("followup", id, prompt)).map_err(storage)
}

fn scoped_payload(id: &str, prompt: &str) -> DelegationResult<String> {
    serde_json::to_string(&("scoped-followup", id, prompt)).map_err(storage)
}

pub(super) fn lookup_scoped(
    db: &Connection,
    caller: &Caller,
    id: &str,
    operation: &str,
    prompt: &str,
) -> DelegationResult<Option<DelegationSnapshot>> {
    admission::lookup_payload(db, caller, operation, &scoped_payload(id, prompt)?)
}

/// The runtime supplies registration identities from its current native write grant.
pub(super) fn reserve_scoped(
    db: &mut Connection,
    caller: &Caller,
    id: &str,
    request: &StartRequest,
    limits: &Limits,
) -> DelegationResult<Admission> {
    let tx = db.transaction().map_err(storage)?;
    let payload = scoped_payload(id, &request.prompt)?;
    if let Some(snapshot) = admission::lookup_payload(&tx, caller, &request.operation_id, &payload)? {
        return Ok(Admission { snapshot, fresh: false });
    }
    tree::ensure_open(&tx, &caller.root_session_id)?;
    let snapshot = read(&tx, id)?;
    authorize_parent(&tx, caller, &snapshot)?;
    if queue::pending_count(&tx, id)? > 0 {
        return Err(DelegationError::new(ErrorCode::Busy, "Child has older queued follow-ups"));
    }
    let latest: Option<(String, String)> = tx
        .query_row(
            "SELECT r.id,r.status FROM runs r JOIN messages m ON m.session_id=r.session_id AND m.message_id=r.parent_id
         WHERE r.session_id=?1 AND json_extract(m.extras_json,'$.\"minerva.origin\"') IS NOT 'manager'
         ORDER BY m.seq DESC LIMIT 1",
            [&caller.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    if !latest.is_some_and(|(run, status)| request.origin_run.as_deref() == Some(run.as_str()) && status == "running") {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Scoped follow-up requires the current parent run"));
    }
    let mut scoped = request.clone();
    queue::restrict_to_latest_parent_run(&tx, caller, &mut scoped)?;
    let snapshot = prepare_activation_scoped(&tx, snapshot, scoped, None, limits, true)?;
    write(&tx, &snapshot)?;
    admission::record_operation(&tx, &snapshot, &payload)?;
    tx.commit().map_err(storage)?;
    Ok(Admission { snapshot, fresh: true })
}

pub(super) fn authorize_parent(db: &Connection, caller: &Caller, child: &DelegationSnapshot) -> DelegationResult<()> {
    if child.caller != *caller
        || resolve_caller(db, &caller.session_id, &caller.workspace_id, &caller.cwd)?.0 != *caller
    {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Only the direct parent can continue a child"));
    }
    Ok(())
}

pub(super) fn restrict_tools(child: &DelegationSnapshot, request: &mut StartRequest) {
    request
        .tool_registrations
        .retain(|name, registration| child.request.tool_registrations.get(name) == Some(registration));
}

pub(super) fn lookup(
    db: &Connection,
    caller: &Caller,
    id: &str,
    operation: &str,
    prompt: &str,
) -> DelegationResult<Option<DelegationSnapshot>> {
    admission::lookup_payload(db, caller, operation, &payload(id, prompt)?)
}

pub(super) fn reserve(
    db: &mut Connection,
    caller: &Caller,
    id: &str,
    request: &StartRequest,
    limits: &Limits,
) -> DelegationResult<Admission> {
    let tx = db.transaction().map_err(storage)?;
    let payload = payload(id, &request.prompt)?;
    if let Some(snapshot) = admission::lookup_payload(&tx, caller, &request.operation_id, &payload)? {
        return Ok(Admission { snapshot, fresh: false });
    }
    tree::ensure_open(&tx, &caller.root_session_id)?;
    let snapshot = read(&tx, id)?;
    authorize_parent(&tx, caller, &snapshot)?;
    if queue::pending_count(&tx, id)? > 0 {
        return Err(DelegationError::new(ErrorCode::Busy, "Child has older queued follow-ups"));
    }
    let mut scoped = request.clone();
    queue::restrict_to_latest_parent_run(&tx, caller, &mut scoped)?;
    let snapshot = prepare_activation(&tx, snapshot, scoped, None, limits)?;
    write(&tx, &snapshot)?;
    admission::record_operation(&tx, &snapshot, &payload)?;
    tx.commit().map_err(storage)?;
    Ok(Admission { snapshot, fresh: true })
}

pub(super) fn prepare_activation(
    db: &Connection,
    snapshot: DelegationSnapshot,
    request: StartRequest,
    work_item_id: Option<String>,
    limits: &Limits,
) -> DelegationResult<DelegationSnapshot> {
    prepare_activation_scoped(db, snapshot, request, work_item_id, limits, false)
}

fn prepare_activation_scoped(
    db: &Connection,
    mut snapshot: DelegationSnapshot,
    mut request: StartRequest,
    work_item_id: Option<String>,
    limits: &Limits,
    writable: bool,
) -> DelegationResult<DelegationSnapshot> {
    if snapshot.request.target != request.target {
        return Err(DelegationError::new(
            ErrorCode::Unsupported,
            "Continuation requires the existing execution target",
        ));
    }
    if snapshot.state != ActivationState::Succeeded || snapshot.handle.is_none() {
        return Err(DelegationError::new(ErrorCode::Busy, "Child has no successful run to continue"));
    }
    let active: i64 = db
        .query_row(
            "SELECT count(*) FROM delegation_children WHERE root_session=?1 AND active=1",
            [&snapshot.caller.root_session_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if active >= limits.active_per_root as i64 {
        return Err(DelegationError::new(ErrorCode::Busy, "Delegation capacity exhausted"));
    }
    let token: String = db.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0)).map_err(storage)?;
    if writable {
        request.tool_registrations.retain(|name, registration| {
            matches!(name.as_str(), "write_file" | "patch_file")
                || snapshot.request.tool_registrations.get(name) == Some(registration)
        });
    } else {
        restrict_tools(&snapshot, &mut request);
    }
    snapshot.request = request;
    snapshot.work_item_id = work_item_id.unwrap_or_else(|| format!("work_{token}"));
    snapshot.activation_id = format!("act_{token}");
    snapshot.state = ActivationState::Starting;
    snapshot.previous_handle = snapshot.handle.take();
    snapshot.completion = None;
    Ok(snapshot)
}
