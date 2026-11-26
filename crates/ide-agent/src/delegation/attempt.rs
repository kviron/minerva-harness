use super::{Admission, AdmissionStore};
use crate::{AgentEntry, Task};
use minerva_plugin_sdk::delegation::{ActivationState, Completion, DelegationResult, ErrorCode};
use serde_json::Value;
use std::{path::Path, time::Duration};

pub(super) async fn open_for_turn(path: &Path) -> DelegationResult<AdmissionStore> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(750);
    loop {
        match AdmissionStore::open(path) {
            Err(error) if error.code == ErrorCode::Busy && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            result => return result,
        }
    }
}

pub(crate) enum Dispatch {
    Ordinary,
    Delegated(Box<Admission>),
    Cancelled,
}

pub(crate) async fn claim(task: &Task, operation: Option<&str>) -> Result<Dispatch, String> {
    if !task.state.lock().delegated_read_only {
        return if operation.is_none() {
            Ok(Dispatch::Ordinary)
        } else {
            Err("Delegated operation requires a delegated task".into())
        };
    }
    let provider = task.state.lock().task.provider.clone();
    if !task.shared.spec(&provider).is_some_and(|spec| super::read_only_driver(spec.driver)) {
        return Err("Delegated read-only execution requires a restricted native driver".into());
    }
    let store = open_for_turn(&task.shared.paths.store_dir).await.map_err(|e| e.to_string())?;
    let intent = operation
        .and_then(|operation| store.lookup(operation))
        .filter(|intent| intent.task_id == task.id)
        .ok_or("Delegated task has no matching admission intent")?;
    if intent.host_turn_id.is_none()
        && intent.completion.as_ref().is_some_and(|completion| completion.state == ActivationState::Cancelled)
    {
        return Ok(Dispatch::Cancelled);
    }
    store
        .claim_dispatch(&intent.operation_id)
        .map(|intent| Dispatch::Delegated(Box::new(intent)))
        .map_err(|e| e.to_string())
}

pub(crate) async fn bind_session(task: &Task, intent: &Admission, session: &str) -> Result<(), String> {
    {
        let state = task.state.lock();
        for (id, value) in &intent.requested_config {
            if !state.task.config.iter().any(|option| &option.id == id && option.current.as_ref() == Some(value)) {
                return Err(format!("Native delegated launch did not accept {id}={value}"));
            }
        }
    }
    open_for_turn(&task.shared.paths.store_dir)
        .await
        .and_then(|store| store.bind_native_session(&intent.operation_id, session))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub(crate) async fn cancel_before_prompt(task: &Task, intent: &Admission) -> Result<(), String> {
    open_for_turn(&task.shared.paths.store_dir)
        .await
        .and_then(|store| store.cancel_before_prompt(&intent.operation_id))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub(crate) async fn bind_claude_prompt(task: &Task, intent: &Admission, session: &str) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    open_for_turn(&task.shared.paths.store_dir)
        .await
        .and_then(|store| store.bind_native_turn(&intent.operation_id, session, &id))
        .map_err(|error| error.to_string())?;
    Ok(id)
}

pub(crate) async fn bind_codex_turn(task: &Task, params: &Value) -> Result<(), String> {
    let operation = params["operation"].as_str().ok_or("Missing delegated operation")?;
    let session = params["sessionId"].as_str().ok_or("Missing Codex session ID")?;
    let turn = params["nativeTurnId"].as_str().ok_or("Missing Codex turn ID")?;
    {
        let state = task.state.lock();
        if !state.delegated_read_only || !state.busy || state.session_id.as_deref() != Some(session) {
            return Err("No matching delegated Codex session".into());
        }
    }
    let store = open_for_turn(&task.shared.paths.store_dir).await.map_err(|error| error.to_string())?;
    if !store.lookup(operation).is_some_and(|intent| intent.task_id == task.id) {
        return Err("No matching delegated Codex operation".into());
    }
    store.bind_native_turn(operation, session, turn).map(|_| ()).map_err(|error| error.to_string())
}

pub(crate) async fn complete(
    task: &Task,
    intent: &Admission,
    session: &str,
    first_entry: usize,
    reply: &Value,
) -> Result<(), String> {
    if reply.pointer("/_meta/openidea/nativeThreadId").and_then(Value::as_str) != Some(session) {
        return Err("Native completion does not belong to the dispatched thread".into());
    }
    let turn = reply
        .pointer("/_meta/openidea/nativeTurnId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("Native completion has no correlated turn identity")?;
    let state = match reply.get("stopReason").and_then(Value::as_str) {
        Some("end_turn") => ActivationState::Succeeded,
        Some("cancelled") => ActivationState::Cancelled,
        Some("refusal") | Some("max_tokens") => ActivationState::Failed,
        _ => return Err("Unknown native completion outcome".into()),
    };
    let text = turn_summary(&task.state.lock().conversation.main.entries, first_entry).to_owned();
    let mut end = text.len().min(8192);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let store = open_for_turn(&task.shared.paths.store_dir).await.map_err(|e| e.to_string())?;
    store
        .complete_turn(
            &intent.operation_id,
            turn,
            Completion { result_id: format!("{}_result", intent.activation_id), state, summary: text[..end].into() },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn turn_summary(entries: &[AgentEntry], first_entry: usize) -> &str {
    entries
        .get(first_entry..)
        .unwrap_or_default()
        .iter()
        .rev()
        .find_map(|entry| if let AgentEntry::Agent { text } = entry { Some(text.as_str()) } else { None })
        .unwrap_or_default()
}

#[cfg(test)]
#[tokio::test]
async fn turn_writer_waits_for_the_observer_to_release_the_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let observer = AdmissionStore::open(dir.path()).unwrap();
    let (writer, ()) = tokio::join!(open_for_turn(dir.path()), async {
        tokio::time::sleep(Duration::from_millis(45)).await;
        drop(observer);
    });
    assert!(writer.is_ok());
}

#[cfg(test)]
#[test]
fn empty_attempt_cannot_reuse_the_previous_answer() {
    let previous = AgentEntry::Agent { text: "Previous answer".into() };
    let current = AgentEntry::Agent { text: "Current answer".into() };
    assert_eq!(turn_summary(std::slice::from_ref(&previous), 1), "");
    assert_eq!(turn_summary(&[previous, current], 1), "Current answer");
}
