use super::*;
use rusqlite::params;

pub(super) fn record_unknown(db: &Connection, snapshot: &DelegationSnapshot, reason: &str) -> DelegationResult<()> {
    let reason: String = reason.chars().take(1024).collect();
    let payload = serde_json::json!({
        "delegationId": snapshot.id,
        "activationId": snapshot.activation_id,
        "state": "unknown",
        "reason": reason,
    });
    let payload = serde_json::to_string(&payload).map_err(storage)?;
    let query = "SELECT data_json FROM session_events WHERE session_id=?1
        AND kind='delegation.diagnostic' AND json_extract(data_json,'$.activationId')=?2
        ORDER BY seq DESC LIMIT 1";
    let previous: Option<String> = db
        .query_row(query, params![snapshot.caller.session_id, snapshot.activation_id], |r| r.get(0))
        .optional()
        .map_err(storage)?;
    if previous.as_deref() == Some(payload.as_str()) {
        return Ok(());
    }
    let seq: i64 = db
        .query_row(
            "SELECT coalesce(max(seq),0)+1 FROM session_events WHERE session_id=?1",
            [&snapshot.caller.session_id],
            |r| r.get(0),
        )
        .map_err(storage)?;
    db.execute(
        "INSERT INTO session_events VALUES(?1,?2,'delegation.diagnostic',?3)",
        params![snapshot.caller.session_id, seq, payload],
    )
    .map_err(storage)?;
    Ok(())
}
