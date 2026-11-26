use super::{ManagerNotice, IDE_USER_FOLLOWUP_ORIGIN};
use crate::StartRunRequest;
use rusqlite::{params, Transaction};
use serde_json::{json, Value};

pub(crate) fn set_followup_origin(
    tx: &Transaction<'_>,
    request: &StartRunRequest,
    extras: &mut Value,
) -> Result<(), String> {
    let Some(child) = request.session_id.strip_prefix("del_") else {
        return Ok(());
    };
    let prefix = minerva_delegation::IDE_USER_OPERATION_PREFIX;
    let is_user_followup: bool = tx
        .query_row(
            "SELECT EXISTS(
             SELECT 1 FROM delegation_children
             WHERE id=?1 AND activation_id=?2
               AND json_extract(snapshot_json,'$.request.target.kind')='minerva'
               AND json_extract(snapshot_json,'$.request.target.route')=?3
               AND json_extract(snapshot_json,'$.request.prompt')=?4
               AND substr(json_extract(snapshot_json,'$.request.operationId'),1,length(?5))=?5
               AND json_extract(snapshot_json,'$.previousHandle.sessionId')=?6
               AND json_extract(snapshot_json,'$.previousHandle.runId')=?7
         )",
            params![
                child,
                request.request_id,
                request.model,
                request.message.text,
                prefix,
                request.session_id,
                request.parent_id
            ],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if is_user_followup {
        extras["minerva.origin"] = json!(IDE_USER_FOLLOWUP_ORIGIN);
    }
    Ok(())
}

pub(crate) fn bind(tx: &Transaction<'_>, request: &StartRunRequest) -> Result<Value, String> {
    let Some(parent) = request.parent_id.as_deref() else {
        return Ok(json!({}));
    };
    if !request.model.starts_with("api:") {
        return Ok(json!({}));
    }
    bind_branch(tx, request, parent).map_err(|error| error.to_string())
}

fn bind_branch(
    tx: &Transaction<'_>,
    request: &StartRunRequest,
    parent: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut query = tx.prepare(
        "WITH RECURSIVE ancestors(id) AS (
           SELECT ?2 UNION SELECT m.parent_id FROM messages m JOIN ancestors a ON m.message_id=a.id
           WHERE m.session_id=?1 AND m.parent_id IS NOT NULL
         )
         SELECT n.result_id,n.notice_json FROM delegation_notices n
         JOIN delegation_terminal_snapshots c ON c.activation_id=json_extract(n.notice_json,'$.activationId')
         JOIN ancestors a ON a.id=json_extract(c.snapshot_json,'$.request.originRun')
         WHERE n.session_id=?1 AND n.bound_run IS NULL ORDER BY n.event_seq LIMIT 8",
    )?;
    let rows = query.query_map(params![request.session_id, parent], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut bytes = 0;
    let mut notices = Vec::new();
    for row in rows {
        let (id, raw) = row?;
        bytes += raw.len();
        if bytes > 16 * 1024 {
            break;
        }
        let notice: ManagerNotice = serde_json::from_str(&raw)?;
        notice.validate()?;
        tx.execute(
            "UPDATE delegation_notices SET bound_run=?3 WHERE session_id=?1 AND result_id=?2 AND bound_run IS NULL",
            params![request.session_id, id, request.request_id],
        )?;
        notices.push(notice);
    }
    Ok(if notices.is_empty() { json!({}) } else { json!({"minerva.delegation": notices}) })
}
