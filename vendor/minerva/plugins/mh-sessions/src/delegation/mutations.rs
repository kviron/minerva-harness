use super::*;
use minerva_delegation::{
    limits::Limits,
    repository::{Admission, RepositoryPort},
};
use rusqlite::params;

impl RepositoryPort for SessionStore {
    fn lookup_queued(
        &self,
        caller: &Caller,
        id: &str,
        op: &str,
        prompt: &str,
    ) -> DelegationResult<Option<minerva_delegation::repository::QueuedFollowup>> {
        self.with_delegation_db(|db| queue::lookup(db, caller, id, op, prompt))
    }
    fn enqueue(
        &self,
        caller: &Caller,
        id: &str,
        request: &StartRequest,
        limits: &Limits,
    ) -> DelegationResult<minerva_delegation::repository::QueuedFollowup> {
        self.enqueue_delegation_followup(caller, id, request, limits)
    }
    fn promote(
        &self,
        caller: &Caller,
        id: &str,
        parent_reads: &std::collections::BTreeMap<String, u64>,
        limits: &Limits,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        self.promote_delegation_followup(caller, id, parent_reads, limits)
    }
    fn lookup_operation(
        &self,
        caller: &Caller,
        request: &StartRequest,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        self.with_delegation_db(|db| super::admission::lookup(db, caller, request))
    }
    fn admit(&self, caller: &Caller, request: &StartRequest, limits: &Limits) -> DelegationResult<Admission> {
        self.with_delegation_db(|db| super::admission::reserve(db, caller, request, limits, None))
    }
    fn admit_fallback(
        &self,
        caller: &Caller,
        request: &StartRequest,
        target: &Target,
        reason: &str,
        limits: &Limits,
    ) -> DelegationResult<Admission> {
        self.with_delegation_db(|db| super::admission::reserve(db, caller, request, limits, Some((target, reason))))
    }
    fn admit_followup(
        &self,
        caller: &Caller,
        id: &str,
        request: &StartRequest,
        limits: &Limits,
    ) -> DelegationResult<Admission> {
        self.with_delegation_db(|db| super::followup::reserve(db, caller, id, request, limits))
    }
    fn lookup_followup(
        &self,
        caller: &Caller,
        id: &str,
        operation: &str,
        prompt: &str,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        self.with_delegation_db(|db| super::followup::lookup(db, caller, id, operation, prompt))
    }
    fn lookup_scoped_followup(
        &self,
        caller: &Caller,
        id: &str,
        operation: &str,
        prompt: &str,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        self.with_delegation_db(|db| followup::lookup_scoped(db, caller, id, operation, prompt))
    }
    fn admit_scoped_followup(
        &self,
        caller: &Caller,
        id: &str,
        request: &StartRequest,
        limits: &Limits,
    ) -> DelegationResult<Admission> {
        self.with_delegation_db(|db| followup::reserve_scoped(db, caller, id, request, limits))
    }
    fn get(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot> {
        self.with_delegation_db(|db| {
            let snapshot = read(db, id)?;
            authorized(caller, &snapshot)?;
            Ok(snapshot)
        })
    }
    fn list(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.with_delegation_db(|db| super::queries::list(db, caller))
    }
    fn started(&self, id: &str, activation: &str, handle: &ExecutionHandle) -> DelegationResult<()> {
        self.update_activation(id, activation, |snapshot| {
            if snapshot.state.terminal() {
                return Ok(());
            }
            if let Some(prior) = &snapshot.handle {
                if prior != handle {
                    return Err(conflict("Execution handle changed"));
                }
            }
            snapshot.handle = Some(handle.clone());
            if snapshot.state != ActivationState::Stopping {
                snapshot.state = ActivationState::Running;
            }
            Ok(())
        })
    }
    fn settle(&self, id: &str, activation: &str, completion: &Completion) -> DelegationResult<()> {
        if !completion.state.terminal() {
            return Err(conflict("Completion requires terminal outcome"));
        }
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let mut snapshot = read(&tx, id)?;
            if snapshot.activation_id != activation {
                return Err(conflict("Stale activation"));
            }
            if let Some(prior) = &snapshot.completion {
                return if prior == completion { Ok(()) } else { Err(conflict("Conflicting terminal result")) };
            }
            snapshot.state = completion.state;
            snapshot.completion = Some(completion.clone());
            tx.execute(
                "INSERT INTO delegation_results VALUES(?1,?2,?3)",
                params![activation, id, serde_json::to_string(completion).map_err(storage)?],
            )
            .map_err(storage)?;
            tx.execute(
                "INSERT INTO delegation_terminal_snapshots VALUES(?1,?2)",
                params![activation, serde_json::to_string(&snapshot).map_err(storage)?],
            )
            .map_err(storage)?;
            tx.execute(
                "INSERT INTO delegation_outbox(activation_id,parent_session) VALUES(?1,?2)",
                params![activation, snapshot.caller.session_id],
            )
            .map_err(storage)?;
            write(&tx, &snapshot)?;
            tx.commit().map_err(storage)
        })
    }
    fn mark_unknown(&self, id: &str, activation: &str, reason: &str) -> DelegationResult<()> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let mut snapshot = read(&tx, id)?;
            if snapshot.activation_id != activation {
                return Err(conflict("Stale activation"));
            }
            if snapshot.state.terminal() {
                return Ok(());
            }
            snapshot.state = ActivationState::Unknown;
            write(&tx, &snapshot)?;
            super::diagnostics::record_unknown(&tx, &snapshot, reason)?;
            tx.commit().map_err(storage)
        })
    }
    fn request_stop(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let mut s = read(&tx, id)?;
            authorized(caller, &s)?;
            tx.execute("UPDATE delegation_followup_queue SET cancelled=1 WHERE delegation_id=?1 AND cancelled=0 AND activation_id IS NULL AND delivery_run IS NULL", [id])
                .map_err(storage)?;
            if !s.state.terminal() {
                s.state = ActivationState::Stopping;
                write(&tx, &s)?;
            }
            tx.commit().map_err(storage)?;
            Ok(s)
        })
    }
    fn request_stop_tree(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.with_delegation_db(|db| super::tree::stop(db, caller))
    }
}

impl crate::sessions::SessionService {
    pub fn delegation_deadline_stops(&self, deadline_seconds: u32) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let mut expired = {
                let mut query = tx
                    .prepare(
                        "SELECT snapshot_json FROM delegation_children WHERE active=1 AND ?1>0
                        AND admitted_at_ms <= CAST(unixepoch('subsec')*1000 AS INTEGER)-?1
                        AND json_extract(snapshot_json,'$.state') IN ('accepted','starting','running')",
                    )
                    .map_err(storage)?;
                let rows = query
                    .query_map([i64::from(deadline_seconds) * 1000], |row| row.get::<_, String>(0))
                    .map_err(storage)?;
                rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage))
                    .collect::<DelegationResult<Vec<DelegationSnapshot>>>()?
            };
            for child in &mut expired {
                child.state = ActivationState::Stopping;
                write(&tx, child)?;
                tx.execute("UPDATE delegation_children SET deadline_stop=1 WHERE id=?1", [&child.id])
                    .map_err(storage)?;
            }
            let pending = {
                let mut query = tx
                    .prepare("SELECT snapshot_json FROM delegation_children WHERE active=1 AND deadline_stop=1")
                    .map_err(storage)?;
                let rows = query.query_map([], |row| row.get::<_, String>(0)).map_err(storage)?;
                rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage))
                    .collect::<DelegationResult<Vec<DelegationSnapshot>>>()?
            };
            tx.commit().map_err(storage)?;
            Ok(pending)
        })
    }

    pub fn request_stop_delegation_activation(
        &self,
        caller: &Caller,
        id: &str,
        activation: &str,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        self.store.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let mut snapshot = read(&tx, id)?;
            authorized(caller, &snapshot)?;
            if snapshot.activation_id != activation || snapshot.state.terminal() {
                return Ok(None);
            }
            snapshot.state = ActivationState::Stopping;
            write(&tx, &snapshot)?;
            tx.commit().map_err(storage)?;
            Ok(Some(snapshot))
        })
    }
}

impl SessionStore {
    fn update_activation(
        &self,
        id: &str,
        activation: &str,
        update: impl FnOnce(&mut DelegationSnapshot) -> DelegationResult<()>,
    ) -> DelegationResult<()> {
        self.with_delegation_db(|db| {
            let tx = db.transaction().map_err(storage)?;
            let mut s = read(&tx, id)?;
            if s.activation_id != activation {
                return Err(conflict("Stale activation"));
            }
            update(&mut s)?;
            write(&tx, &s)?;
            tx.commit().map_err(storage)
        })
    }
}
