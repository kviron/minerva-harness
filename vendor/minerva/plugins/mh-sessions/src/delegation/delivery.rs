use super::*;
use rusqlite::params;

fn fence(db: &Connection, caller: &Caller, generation: i64) -> DelegationResult<()> {
    let current: Option<i64> = db
        .query_row("SELECT generation FROM delegation_consumers WHERE session_id=?1", [&caller.session_id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage)?;
    let resolved = resolve_caller(db, &caller.session_id, &caller.workspace_id, &caller.cwd)?.0;
    if current != Some(generation) || resolved != *caller {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Stale delegation inbox consumer"));
    }
    Ok(())
}

impl crate::sessions::SessionService {
    pub fn delegation_consumer(&self, caller: &Caller, owner: &str) -> DelegationResult<i64> {
        if owner.is_empty() || owner.len() > 200 {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid inbox owner"));
        }
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let resolved = resolve_caller(&tx, &caller.session_id, &caller.workspace_id, &caller.cwd)?.0;
            if resolved != *caller {
                return Err(DelegationError::new(ErrorCode::Forbidden, "Invalid inbox caller"));
            }
            let query = "INSERT INTO delegation_consumers VALUES(?1,?2,1)
                ON CONFLICT(session_id) DO UPDATE SET
                generation=CASE WHEN owner=excluded.owner THEN generation ELSE generation+1 END,
                owner=excluded.owner";
            tx.execute(query, params![caller.session_id, owner]).map_err(storage)?;
            let generation = tx
                .query_row(
                    "SELECT generation FROM delegation_consumers WHERE session_id=?1",
                    [&caller.session_id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(generation)
        })
    }

    /// Records terminal facts and acknowledges transport, without consuming a model input.
    pub fn delegation_receive_results(
        &self,
        caller: &Caller,
        generation: i64,
        limit: usize,
    ) -> DelegationResult<Vec<ManagerNotice>> {
        let limit = limit.clamp(1, 128) as i64;
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            fence(&tx, caller, generation)?;
            let snapshots = {
                let query = "SELECT c.snapshot_json FROM delegation_outbox o
                    JOIN delegation_terminal_snapshots c ON c.activation_id=o.activation_id
                    WHERE o.parent_session=?1 AND o.acknowledged=0 ORDER BY o.rowid LIMIT ?2";
                let mut q = tx.prepare(query).map_err(storage)?;
                let rows =
                    q.query_map(params![caller.session_id, limit], |r| r.get::<_, String>(0)).map_err(storage)?;
                rows.map(|r| serde_json::from_str::<DelegationSnapshot>(&r.map_err(storage)?).map_err(storage))
                    .collect::<DelegationResult<Vec<_>>>()?
            };
            for snapshot in snapshots {
                authorized(caller, &snapshot)?;
                let result = snapshot.completion.ok_or_else(|| conflict("Outbox has no completion"))?;
                let notice = ManagerNotice {
                    version: 1,
                    origin: NoticeOrigin::Manager,
                    result_id: result.result_id,
                    delegation_id: snapshot.id,
                    work_item_id: snapshot.work_item_id,
                    activation_id: snapshot.activation_id.clone(),
                    state: result.state,
                    summary: result.summary,
                };
                notice.validate()?;
                let json = serde_json::to_string(&notice).map_err(storage)?;
                let query = "SELECT coalesce(max(seq),0)+1 FROM session_events WHERE session_id=?1";
                let seq: i64 = tx.query_row(query, [&caller.session_id], |r| r.get(0)).map_err(storage)?;
                let query = "INSERT INTO session_events VALUES(?1,?2,'delegation.result',?3)";
                tx.execute(query, params![caller.session_id, seq, json]).map_err(storage)?;
                let query =
                    "INSERT INTO delegation_notices(result_id,session_id,notice_json,event_seq) VALUES(?1,?2,?3,?4)";
                tx.execute(query, params![notice.result_id, caller.session_id, json, seq]).map_err(storage)?;
                tx.execute(
                    "UPDATE delegation_outbox SET acknowledged=1 WHERE activation_id=?1",
                    [&snapshot.activation_id],
                )
                .map_err(storage)?;
            }
            let notices = {
                let query = "SELECT notice_json FROM delegation_notices WHERE session_id=?1
                    AND bound_run IS NULL ORDER BY event_seq LIMIT ?2";
                let mut q = tx.prepare(query).map_err(storage)?;
                let rows =
                    q.query_map(params![caller.session_id, limit], |r| r.get::<_, String>(0)).map_err(storage)?;
                rows.map(|r| serde_json::from_str(&r.map_err(storage)?).map_err(storage))
                    .collect::<DelegationResult<Vec<_>>>()?
            };
            tx.commit().map_err(storage)?;
            Ok(notices)
        })
    }

    /// Binding only: the future input arbiter must combine this with run admission.
    pub fn delegation_bind_notices(
        &self,
        caller: &Caller,
        generation: i64,
        ids: &[String],
        run: &str,
    ) -> DelegationResult<()> {
        if ids.is_empty() || ids.len() > 128 {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid notice batch"));
        }
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            fence(&tx, caller, generation)?;
            let query = "SELECT EXISTS(SELECT 1 FROM runs WHERE session_id=?1 AND id=?2)";
            let exists: bool = tx.query_row(query, params![caller.session_id, run], |r| r.get(0)).map_err(storage)?;
            if !exists {
                return Err(conflict("Cannot bind notice to an unaccepted run"));
            }
            for id in ids {
                let query = "UPDATE delegation_notices SET bound_run=?3 WHERE session_id=?1
                    AND result_id=?2 AND (bound_run IS NULL OR bound_run=?3)";
                let changed = tx.execute(query, params![caller.session_id, id, run]).map_err(storage)?;
                if changed != 1 {
                    return Err(conflict("Notice missing or bound to another run"));
                }
            }
            tx.commit().map_err(storage)
        })
    }
}
