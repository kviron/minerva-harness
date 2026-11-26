use super::*;

impl crate::sessions::SessionService {
    pub fn confirms_child_stopped_before_run(&self, id: &str, activation: &str) -> DelegationResult<bool> {
        self.store.with_delegation_db(|db| {
            let child = read(db, id)?;
            if child.activation_id != activation || child.state != ActivationState::Stopping {
                return Ok(false);
            }
            db.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM runs WHERE session_id=?1 AND id=?2)",
                rusqlite::params![format!("del_{id}"), activation],
                |row| row.get(0),
            )
            .map_err(storage)
        })
    }
}

pub(super) fn ensure_open(db: &Connection, root: &str) -> DelegationResult<()> {
    let stopped: bool = db
        .query_row("SELECT EXISTS(SELECT 1 FROM delegation_stopped_trees WHERE root_session=?1)", [root], |row| {
            row.get(0)
        })
        .map_err(storage)?;
    if stopped {
        return Err(DelegationError::new(ErrorCode::Cancelled, "Delegation tree is stopped"));
    }
    Ok(())
}

pub(super) fn stop(db: &mut Connection, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
    let tx = db.transaction().map_err(storage)?;
    let (resolved, depth) = resolve_caller(&tx, &caller.session_id, &caller.workspace_id, &caller.cwd)?;
    if depth != 0 || resolved != *caller {
        return Err(DelegationError::new(
            ErrorCode::Forbidden,
            "Only the root conversation can stop its delegation tree",
        ));
    }
    tx.execute("INSERT OR IGNORE INTO delegation_stopped_trees VALUES(?1)", [&caller.root_session_id])
        .map_err(storage)?;
    tx.execute(
        "UPDATE delegation_followup_queue SET cancelled=1 WHERE cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL AND delegation_id IN (
            SELECT id FROM delegation_children WHERE root_session=?1
        )",
        [&caller.root_session_id],
    )
    .map_err(storage)?;
    tx.execute(
        "INSERT INTO delegation_wake_state(session_id,generation,stopped)
         SELECT s.id,1,1 FROM sessions s
         WHERE s.id=?1 OR EXISTS(
            SELECT 1 FROM delegation_children c WHERE c.root_session=?1
              AND (s.id='del_'||c.id OR s.id=json_extract(c.snapshot_json,'$.handle.sessionId'))
         )
         ON CONFLICT(session_id) DO UPDATE SET generation=generation+1,stopped=1",
        [&caller.root_session_id],
    )
    .map_err(storage)?;
    let mut snapshots = {
        let mut query = tx
            .prepare("SELECT snapshot_json FROM delegation_children WHERE root_session=?1 AND active=1 ORDER BY rowid")
            .map_err(storage)?;
        let rows = query.query_map([&caller.root_session_id], |row| row.get::<_, String>(0)).map_err(storage)?;
        rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage))
            .collect::<DelegationResult<Vec<DelegationSnapshot>>>()?
    };
    for snapshot in &mut snapshots {
        snapshot.state = ActivationState::Stopping;
        write(&tx, snapshot)?;
    }
    tx.commit().map_err(storage)?;
    Ok(snapshots)
}
