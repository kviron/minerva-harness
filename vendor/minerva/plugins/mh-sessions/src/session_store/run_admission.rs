use super::*;

pub(super) fn accept(
    tx: &rusqlite::Transaction<'_>,
    request: &crate::StartRunRequest,
    created_at: &str,
    origin: &crate::delegation::wake::AdmissionOrigin,
) -> Result<bool, String> {
    let payload = serde_json::to_value(request).map_err(|e| e.to_string())?;
    let old: Option<String> = tx
        .query_row(
            "SELECT request_json FROM runs WHERE session_id=?1 AND id=?2",
            params![request.session_id, request.request_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(old) = old {
        if serde_json::from_str::<Value>(&old).map_err(|e| e.to_string())? == payload {
            return Ok(false);
        }
        return Err("conflict: requestId already has a different payload".into());
    }
    crate::delegation::wake::admit(tx, request, origin)?;
    let busy: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE session_id=?1 AND status='running') OR EXISTS(SELECT 1 FROM messages WHERE session_id=?1 AND message_id IN (?2,?3))",params![request.session_id,request.request_id,request.message.id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if busy {
        return Err("conflict: session busy or message ID already exists".into());
    }
    if request.options["requireCurrentHead"].as_bool() == Some(true) {
        let head: Option<String> = tx
            .query_row(
                "SELECT message_id FROM messages WHERE session_id=?1 ORDER BY seq DESC LIMIT 1",
                [&request.session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if head != request.parent_id {
            return Err("conflict: session head changed before run admission".into());
        }
    }
    if let Some(parent) = &request.parent_id {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE session_id=?1 AND message_id=?2)",
                params![request.session_id, parent],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("parent is not in this session".into());
        }
    }
    let mut input = Vec::new();
    if !request.message.text.is_empty() {
        input.push(ContentPart::text("text", &request.message.text));
    }
    if let Some(images) = request.options["attachments"].as_array() {
        input.extend(images.iter().cloned().map(|data| {
            ContentPart {
                kind: if matches!(
                    data["mimeType"].as_str(),
                    Some("image/png" | "image/jpeg" | "image/gif" | "image/webp")
                ) {
                    "image"
                } else {
                    "file"
                }
                .into(),
                data,
            }
        }));
    }
    crate::transcript::Transcript { parts: input.clone(), replay: Default::default() }.validate()?;
    let mut extras = if matches!(origin, crate::delegation::wake::AdmissionOrigin::Wake { question: Some(_), .. }) {
        json!({})
    } else {
        crate::delegation::run_input::bind(tx, request)?
    };
    if let crate::delegation::wake::AdmissionOrigin::Wake { question, .. } = origin {
        if question.is_none() && !extras["minerva.delegation"].as_array().is_some_and(|items| !items.is_empty()) {
            return Err("Wake has no eligible pending notices".into());
        }
        extras["minerva.origin"] = json!("manager");
        if let Some(binding) = question {
            extras["minerva.delegation.question"] = json!({
                "childId": binding.child_id,
                "questionId": binding.question_id,
            });
        }
    } else {
        crate::delegation::run_input::set_followup_origin(tx, request, &mut extras)?;
    }
    tx.execute("INSERT INTO messages(session_id,message_id,parent_id,role,created_at,content_json,extras_json,seq) VALUES(?1,?2,?3,'user',?4,?5,?6,(SELECT COALESCE(MAX(seq),0)+1 FROM messages WHERE session_id=?1))",params![request.session_id,request.message.id,request.parent_id,created_at,serde_json::to_string(&input).map_err(|e|e.to_string())?,extras.to_string()]).map_err(|e|e.to_string())?;
    tx.execute(
        "INSERT INTO runs VALUES(?1,?2,?3,?4,'running',NULL,unixepoch())",
        params![request.session_id, request.request_id, request.message.id, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
            "UPDATE sessions SET title=CASE WHEN title='Новый диалог' THEN ?3 ELSE title END,updated_at=unixepoch(),model=CASE WHEN EXISTS(SELECT 1 FROM session_selections WHERE session_id=?1) THEN model ELSE ?2 END WHERE id=?1",
            params![request.session_id, request.model,request.message.text.chars().take(80).collect::<String>()],
        )
        .map_err(|e| e.to_string())?;
    Ok(true)
}
