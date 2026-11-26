use super::*;
use minerva_delegation::repository::RepositoryPort;

pub(super) fn list(db: &Connection, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
    let mut q = db
        .prepare("SELECT snapshot_json FROM delegation_children WHERE root_session=?1 ORDER BY rowid")
        .map_err(storage)?;
    let rows = q.query_map([&caller.root_session_id], |r| r.get::<_, String>(0)).map_err(storage)?;
    let mut snapshots = Vec::new();
    for row in rows {
        let snapshot = serde_json::from_str(&row.map_err(storage)?).map_err(storage)?;
        if authorized(caller, &snapshot).is_ok() {
            snapshots.push(snapshot);
        }
    }
    Ok(snapshots)
}

impl crate::sessions::SessionService {
    pub fn internal_delegation_descendants(&self, session: &str) -> DelegationResult<Vec<String>> {
        self.store.with_delegation_db(|db| {
            // Completed turns can still have live background jobs.
            let mut query = db
                .prepare(
                    "WITH RECURSIVE descendants(session) AS (
                    SELECT ?1
                    UNION
                    SELECT 'del_' || c.id FROM delegation_children c
                    JOIN descendants d ON json_extract(c.snapshot_json,'$.caller.sessionId')=d.session
                    WHERE json_extract(c.snapshot_json,'$.request.target.kind')='minerva'
                )
                SELECT session FROM descendants WHERE session<>?1 ORDER BY session",
                )
                .map_err(storage)?;
            let rows = query.query_map([session], |row| row.get(0)).map_err(storage)?;
            rows.collect::<Result<_, _>>().map_err(storage)
        })
    }

    pub fn settled_delegation_activation(
        &self,
        caller: &Caller,
        child: &str,
        activation: &str,
    ) -> DelegationResult<DelegationSnapshot> {
        self.store.with_delegation_db(|db| {
            let raw: String = db
                .query_row(
                    "SELECT snapshot_json FROM delegation_terminal_snapshots WHERE activation_id=?1",
                    [activation],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let snapshot: DelegationSnapshot = serde_json::from_str(&raw).map_err(storage)?;
            authorized(caller, &snapshot)?;
            if snapshot.id != child {
                return Err(conflict("Activation belongs to another delegated child"));
            }
            Ok(snapshot)
        })
    }

    pub fn has_delegation_history(&self) -> DelegationResult<bool> {
        self.store.with_delegation_db(|db| {
            db.query_row("SELECT EXISTS(SELECT 1 FROM delegation_children)", [], |row| row.get(0)).map_err(storage)
        })
    }

    pub fn delegation_repository(&self) -> std::sync::Arc<dyn RepositoryPort> {
        self.store.clone()
    }
}
