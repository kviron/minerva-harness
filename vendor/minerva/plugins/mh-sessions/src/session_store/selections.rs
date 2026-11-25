//! Durable requested routes and handler admissions; no provider or network policy.
use super::*;
use crate::{DispatchAdmission, RouteDispatch, RouteSelection};
#[cfg(test)]
mod tests;

pub(super) fn migrate(db: &mut Connection) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE session_selections(session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,revision INTEGER NOT NULL CHECK(revision>0),model TEXT NOT NULL,reasoning_effort TEXT,PRIMARY KEY(session_id,revision));
        CREATE TABLE run_selection_dispatches(session_id TEXT NOT NULL,run_id TEXT NOT NULL,effect_id TEXT NOT NULL,revision INTEGER NOT NULL,data_json TEXT NOT NULL,PRIMARY KEY(session_id,run_id,effect_id),FOREIGN KEY(session_id,run_id) REFERENCES runs(session_id,id),FOREIGN KEY(session_id,revision) REFERENCES session_selections(session_id,revision));
        PRAGMA user_version=7;").map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn latest(db: &Connection, session: &str) -> Result<Option<RouteSelection>, String> {
    db.query_row("SELECT revision,model,reasoning_effort FROM session_selections WHERE session_id=?1 ORDER BY revision DESC LIMIT 1", [session], |row| Ok(RouteSelection {
        revision: row.get(0)?, model: row.get(1)?, reasoning_effort: row.get(2)?,
    })).optional().map_err(|e| e.to_string())
}

fn event(tx: &rusqlite::Transaction<'_>, session: &str, kind: &str, data: Value) -> Result<(), String> {
    tx.execute("INSERT INTO session_events(session_id,seq,kind,data_json) VALUES(?1,(SELECT COALESCE(MAX(seq),0)+1 FROM session_events WHERE session_id=?1),?2,?3)", params![session,kind,data.to_string()]).map_err(|e|e.to_string())?;
    Ok(())
}

fn write(
    tx: &rusqlite::Transaction<'_>,
    session: &str,
    model: &str,
    effort: Option<&str>,
    expected: Option<i64>,
    only_if_absent: bool,
) -> Result<RouteSelection, String> {
    let exists: bool = tx
        .query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)", [session], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !exists {
        return Err("Session not found".into());
    }
    let old = latest(tx, session)?;
    if only_if_absent {
        if let Some(old) = old {
            return Ok(old);
        }
    }
    if expected.is_some_and(|revision| old.as_ref().map_or(0, |old| old.revision) != revision) {
        return Err("Model selection changed; refresh its settings".into());
    }
    let effort = effort.filter(|effort| *effort != "default");
    if let Some(old) = old.as_ref() {
        if old.model == model && old.reasoning_effort.as_deref() == effort {
            return Ok(old.clone());
        }
    }
    let revision = old.map_or(Some(1), |old| old.revision.checked_add(1)).ok_or("Selection revision exhausted")?;
    let selection = RouteSelection { revision, model: model.into(), reasoning_effort: effort.map(str::to_owned) };
    tx.execute(
        "INSERT INTO session_selections(session_id,revision,model,reasoning_effort) VALUES(?1,?2,?3,?4)",
        params![session, revision, model, effort],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("UPDATE sessions SET model=?2 WHERE id=?1", params![session, model]).map_err(|e| e.to_string())?;
    event(tx, session, "session.selection_requested", json!(selection))?;
    Ok(selection)
}

impl SessionStore {
    pub(crate) fn selection(&self, session: &str) -> Result<Option<RouteSelection>, String> {
        let db = self.connection.lock()?;
        latest(&db, session)
    }

    pub(crate) fn request_selection(
        &self,
        session: &str,
        model: &str,
        effort: Option<&str>,
        expected: Option<i64>,
        only_if_absent: bool,
    ) -> Result<RouteSelection, String> {
        let mut db = self.connection.lock()?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let selection = write(&tx, session, model, effort, expected, only_if_absent)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(selection)
    }

    pub(super) fn set_model_with_selection(&self, session: &str, model: &str) -> Result<(), String> {
        let mut db = self.connection.lock()?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        if let Some(old) = latest(&tx, session)? {
            let effort = (old.model == model).then_some(old.reasoning_effort.as_deref()).flatten();
            write(&tx, session, model, effort, None, false)?;
        } else if tx
            .execute("UPDATE sessions SET model=?2 WHERE id=?1", params![session, model])
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("Session not found".into());
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub(crate) fn record_selection_dispatch(
        &self,
        session: &str,
        dispatch: &RouteDispatch,
    ) -> Result<DispatchAdmission, String> {
        let payload = serde_json::to_value(dispatch).map_err(|e| e.to_string())?;
        let mut db = self.connection.lock()?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT data_json FROM run_selection_dispatches WHERE session_id=?1 AND run_id=?2 AND effect_id=?3",
                params![session, dispatch.run_id, dispatch.effect_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(previous) = previous {
            if serde_json::from_str::<Value>(&previous).map_err(|e| e.to_string())? == payload {
                return Ok(DispatchAdmission::Duplicate);
            }
            return Err("Effect ID already has another route admission".into());
        }
        let request: Option<String> = tx
            .query_row(
                "SELECT request_json FROM runs WHERE session_id=?1 AND id=?2 AND status='running'",
                params![session, dispatch.run_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let request: Value =
            serde_json::from_str(&request.ok_or("Run is no longer active")?).map_err(|e| e.to_string())?;
        let api_route = |model: &str| {
            model
                .strip_prefix("api:")
                .and_then(|route| route.split_once('/'))
                .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
        };
        if request["options"]["liveSelection"] != true
            || !request["model"].as_str().is_some_and(api_route)
            || !api_route(&dispatch.selection.model)
        {
            return Err("Live selection is not admitted for this run and route".into());
        }
        let selection: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT model,reasoning_effort FROM session_selections WHERE session_id=?1 AND revision=?2",
                params![session, dispatch.selection.revision],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if selection.as_ref().map(|(model, effort)| (model.as_str(), effort.as_deref()))
            != Some((dispatch.selection.model.as_str(), dispatch.selection.reasoning_effort.as_deref()))
        {
            return Err("Dispatch does not match its requested selection revision".into());
        }
        let last: Option<String> = tx.query_row("SELECT data_json FROM run_selection_dispatches WHERE session_id=?1 AND run_id=?2 ORDER BY rowid DESC LIMIT 1",params![session,dispatch.run_id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        let changed = if let Some(last) = last {
            let last: RouteDispatch = serde_json::from_str(&last).map_err(|e| e.to_string())?;
            last.selection.model != dispatch.selection.model
                || last.selection.reasoning_effort != dispatch.selection.reasoning_effort
        } else {
            let effort = request["options"]["reasoningEffort"].as_str().filter(|effort| *effort != "default");
            request["model"].as_str() != Some(dispatch.selection.model.as_str())
                || effort != dispatch.selection.reasoning_effort.as_deref()
        };
        tx.execute("INSERT INTO run_selection_dispatches(session_id,run_id,effect_id,revision,data_json) VALUES(?1,?2,?3,?4,?5)",params![session,dispatch.run_id,dispatch.effect_id,dispatch.selection.revision,payload.to_string()]).map_err(|e|e.to_string())?;
        event(&tx, session, "run.selection_dispatch_intent", payload)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(DispatchAdmission::Recorded { changed })
    }
}
