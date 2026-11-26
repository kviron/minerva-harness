mod admission;
mod delivery;
mod diagnostics;
mod followup;
mod followup_delivery;
mod mutations;
mod queries;
pub mod questions;
mod queue;
pub(crate) mod run_input;
pub(crate) mod schema;
#[cfg(test)]
mod tests;
mod tree;
mod user_followups;
pub(crate) mod wake;
pub use followup_delivery::PreferredFollowup;
pub use user_followups::{UserFollowup, UserFollowupState};
pub use wake::WakeQuestion;
#[cfg(test)]
mod wake_tests;

use crate::session_store::SessionStore;
use minerva_plugin_sdk::delegation::*;
use rusqlite::{Connection, OptionalExtension};

fn storage(e: impl std::fmt::Display) -> DelegationError {
    DelegationError::new(ErrorCode::StorageFailed, e.to_string())
}
fn conflict(message: &str) -> DelegationError {
    DelegationError::new(ErrorCode::Conflict, message)
}
fn read(db: &Connection, id: &str) -> DelegationResult<DelegationSnapshot> {
    read_optional(db, id)?.ok_or_else(|| DelegationError::new(ErrorCode::Unavailable, "Delegation not found"))
}
fn read_optional(db: &Connection, id: &str) -> DelegationResult<Option<DelegationSnapshot>> {
    let raw: Option<String> = db
        .query_row("SELECT snapshot_json FROM delegation_children WHERE id=?1", [id], |r| r.get(0))
        .optional()
        .map_err(storage)?;
    raw.map(|json| serde_json::from_str(&json)).transpose().map_err(storage)
}
fn authorized(caller: &Caller, snapshot: &DelegationSnapshot) -> DelegationResult<()> {
    if caller.root_session_id != snapshot.caller.root_session_id
        || caller.workspace_id != snapshot.caller.workspace_id
        || caller.cwd != snapshot.caller.cwd
        || (caller.session_id != snapshot.caller.session_id && caller.session_id != snapshot.caller.root_session_id)
    {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Delegation belongs to another caller"));
    }
    Ok(())
}
fn write(db: &Connection, snapshot: &DelegationSnapshot) -> DelegationResult<()> {
    db.execute(
        "UPDATE delegation_children SET snapshot_json=?2,active=?3,activation_id=?4,
         admitted_at_ms=CASE WHEN activation_id<>?4 THEN CAST(unixepoch('subsec')*1000 AS INTEGER) ELSE admitted_at_ms END,
         deadline_stop=CASE WHEN activation_id<>?4 THEN 0 ELSE deadline_stop END
         WHERE id=?1",
        rusqlite::params![
            snapshot.id,
            serde_json::to_string(snapshot).map_err(storage)?,
            !snapshot.state.terminal(),
            snapshot.activation_id
        ],
    )
    .map_err(storage)?;
    if snapshot.state.terminal() || matches!(snapshot.state, ActivationState::Stopping | ActivationState::Unknown) {
        db.execute(
            "UPDATE delegation_questions SET state='expired' WHERE delegation_id=?1 AND activation_id=?2 AND state='pending'",
            rusqlite::params![snapshot.id, snapshot.activation_id],
        ).map_err(storage)?;
    }
    if snapshot.state.terminal() || snapshot.state == ActivationState::Unknown {
        if let Some(handle) = &snapshot.handle {
            db.execute(
                "UPDATE delegation_followup_queue SET delivery_state='unknown'
                 WHERE delegation_id=?1 AND delivery_run=?2 AND delivery_state='claimed'",
                rusqlite::params![snapshot.id, handle.run_id],
            )
            .map_err(storage)?;
        }
    }
    Ok(())
}
fn resolve_caller(db: &Connection, session: &str, workspace: &str, cwd: &str) -> DelegationResult<(Caller, u32)> {
    let raw:Option<String>=db.query_row("SELECT snapshot_json FROM delegation_children WHERE 'del_'||id=?1 OR json_extract(snapshot_json,'$.handle.sessionId')=?1",[session],|r|r.get(0)).optional().map_err(storage)?;
    if let Some(raw) = raw {
        let parent: DelegationSnapshot = serde_json::from_str(&raw).map_err(storage)?;
        if parent.caller.workspace_id != workspace || parent.caller.cwd != cwd {
            return Err(DelegationError::new(ErrorCode::Forbidden, "Child workspace mismatch"));
        }
        return Ok((
            Caller {
                session_id: session.into(),
                root_session_id: parent.caller.root_session_id,
                workspace_id: workspace.into(),
                cwd: cwd.into(),
            },
            parent.depth,
        ));
    }
    let exists: bool =
        db.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)", [session], |r| r.get(0)).map_err(storage)?;
    if !exists {
        return Err(DelegationError::new(ErrorCode::Forbidden, "Caller session not found"));
    }
    Ok((
        Caller {
            session_id: session.into(),
            root_session_id: session.into(),
            workspace_id: workspace.into(),
            cwd: cwd.into(),
        },
        0,
    ))
}
impl SessionStore {
    pub fn delegation_pending_results(&self, caller: &Caller) -> DelegationResult<Vec<Completion>> {
        self.with_delegation_db(|db| {
            let mut q = db.prepare("SELECT r.completion_json FROM delegation_outbox o JOIN delegation_results r USING(activation_id) WHERE o.parent_session=?1 AND o.acknowledged=0 ORDER BY o.rowid").map_err(storage)?;
            let rows = q.query_map([&caller.session_id], |r| r.get::<_,String>(0)).map_err(storage)?;
            rows.map(|r| serde_json::from_str(&r.map_err(storage)?).map_err(storage)).collect()
        })
    }
}
impl crate::sessions::SessionService {
    pub fn delegation_unbound_results(&self) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.store.with_delegation_db(|db| {
            let mut query = db.prepare(
                "WITH RECURSIVE
                 pending(session_id, event_seq, origin, snapshot) AS (
                    SELECT n.session_id,n.event_seq,json_extract(c.snapshot_json,'$.request.originRun'),c.snapshot_json
                    FROM delegation_notices n
                    JOIN delegation_terminal_snapshots c ON c.activation_id=json_extract(n.notice_json,'$.activationId')
                    JOIN sessions s ON s.id=n.session_id
                    WHERE n.bound_run IS NULL AND s.model LIKE 'api:%'
                      AND json_extract(c.snapshot_json,'$.request.originRun') IS NOT NULL
                      AND NOT EXISTS(SELECT 1 FROM delegation_stopped_trees t
                        WHERE t.root_session=json_extract(c.snapshot_json,'$.caller.rootSessionId'))
                      AND NOT EXISTS(SELECT 1 FROM delegation_wake_state w WHERE w.session_id=n.session_id AND w.stopped=1)
                      AND NOT EXISTS(SELECT 1 FROM runs r WHERE r.session_id=n.session_id AND r.status='running')
                 ),
                 ancestors(session_id, id) AS (
                    SELECT p.session_id,(SELECT m.message_id FROM messages m WHERE m.session_id=p.session_id
                        ORDER BY m.seq DESC LIMIT 1) FROM (SELECT DISTINCT session_id FROM pending) p
                    UNION
                    SELECT m.session_id,m.parent_id FROM ancestors a JOIN messages m
                        ON m.session_id=a.session_id AND m.message_id=a.id WHERE m.parent_id IS NOT NULL
                 ),
                 eligible AS (
                    SELECT p.snapshot,p.event_seq,p.session_id,
                        ROW_NUMBER() OVER(PARTITION BY p.session_id ORDER BY p.event_seq) AS position
                    FROM pending p JOIN ancestors a ON a.session_id=p.session_id AND a.id=p.origin
                 )
                 SELECT snapshot FROM eligible WHERE position=1 ORDER BY event_seq,session_id LIMIT 64",
            ).map_err(storage)?;
            let rows = query.query_map([], |row| row.get::<_, String>(0)).map_err(storage)?;
            rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage)).collect()
        })
    }
    pub fn delegation_active(&self) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.store.with_delegation_db(|db| {
            let mut q = db
                .prepare("SELECT snapshot_json FROM delegation_children WHERE active=1 ORDER BY rowid")
                .map_err(storage)?;
            let rows = q.query_map([], |r| r.get::<_, String>(0)).map_err(storage)?;
            rows.map(|row| serde_json::from_str(&row.map_err(storage)?).map_err(storage)).collect()
        })
    }
    pub fn delegation_pending(&self) -> DelegationResult<Vec<DelegationSnapshot>> {
        self.store.with_delegation_db(|db|{
            let mut q=db.prepare("SELECT c.snapshot_json FROM delegation_outbox o JOIN delegation_terminal_snapshots c ON c.activation_id=o.activation_id WHERE o.acknowledged=0 ORDER BY o.rowid").map_err(storage)?;
            let rows=q.query_map([],|r|r.get::<_,String>(0)).map_err(storage)?;
            rows.map(|row|serde_json::from_str(&row.map_err(storage)?).map_err(storage)).collect()
        })
    }
    pub fn delegation_caller(&self, session: &str, workspace: &str, cwd: &str) -> DelegationResult<Caller> {
        self.store.with_delegation_db(|db| resolve_caller(db, session, workspace, cwd).map(|v| v.0))
    }

    pub fn is_internal_delegation_session(&self, session: &str) -> DelegationResult<bool> {
        let Some(id) = session.strip_prefix("del_") else { return Ok(false) };
        self.store.with_delegation_db(|db| {
            db.query_row("SELECT EXISTS(SELECT 1 FROM delegation_children WHERE id=?1)", [id], |row| row.get(0))
                .map_err(storage)
        })
    }

    pub fn internal_delegation_ancestors(&self, session: &str) -> DelegationResult<Vec<String>> {
        let Some(id) = session.strip_prefix("del_") else { return Ok(Vec::new()) };
        self.store.with_delegation_db(|db| {
            let Some(mut child) = read_optional(db, id)? else { return Ok(Vec::new()) };
            let root = child.caller.root_session_id.clone();
            let mut seen = std::collections::HashSet::from([session.to_owned()]);
            let mut ancestors = Vec::new();
            loop {
                let parent = child.caller.session_id;
                if !seen.insert(parent.clone()) {
                    return Err(conflict("Delegation policy lineage contains a cycle"));
                }
                ancestors.push(parent.clone());
                if parent == root {
                    return Ok(ancestors);
                }
                let id =
                    parent.strip_prefix("del_").ok_or_else(|| conflict("Delegation parent is outside its tree"))?;
                let ancestor = read(db, id)?;
                if ancestor.caller.root_session_id != root
                    || ancestor.caller.workspace_id != child.caller.workspace_id
                    || ancestor.caller.cwd != child.caller.cwd
                {
                    return Err(conflict("Delegation policy lineage changed workspace"));
                }
                child = ancestor;
            }
        })
    }
}
