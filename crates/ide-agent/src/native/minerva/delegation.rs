//! Backend-owned delegation snapshots projected onto existing ACP child conversations.
use super::Engine;
use ide_minerva::minerva_runtime::{
    DelegationQuestion, QuestionState, Runtime,
    delegation_contract::{ActivationState, DelegationSnapshot, IDE_USER_FOLLOWUP_ORIGIN, Target},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

impl Engine {
    pub(super) fn cancel_delegated(&self, child_id: &str) -> bool {
        let Some((parent, delegation)) = self.delegated_runs.lock().get(child_id).cloned() else {
            return false;
        };
        let service = self.service.clone();
        let cwd = self.cwd.clone();
        self.runtime.spawn(async move {
            let result = async {
                let runtime = service.runtime().await?;
                let workspace = service.workspace(&cwd).await?;
                let settings = runtime.plugins.harness.context.workspace_get(&parent)?;
                let caller = runtime
                    .sessions
                    .delegation_caller(&parent, &workspace.id, settings.cwd.as_deref().ok_or("No workspace")?)
                    .map_err(|e| e.to_string())?;
                runtime.interrupt_delegation(&caller, &delegation).await?;
                Ok::<_, String>(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(%error, "Minerva delegated cancellation requires reconciliation");
            }
        });
        true
    }

    pub(super) fn watch_delegation(&self, runtime: &Arc<Runtime>, parent: &str) -> Result<(), String> {
        let settings = runtime.plugins.harness.context.workspace_get(parent)?;
        let cwd = settings.cwd.ok_or("Session has no workspace")?;
        let workspace = runtime
            .plugins
            .context()
            .try_service::<ide_minerva::minerva_runtime::workspaces::WorkspaceService>()
            .map_err(|e| e.to_string())?
            .list()?
            .into_iter()
            .find(|w| w.session_ids.iter().any(|id| id == parent))
            .ok_or("Session workspace not found")?;
        let caller = runtime.sessions.delegation_caller(parent, &workspace.id, &cwd).map_err(|e| e.to_string())?;
        let registered =
            runtime.plugins.harness.tools.snapshot().definitions().iter().any(|tool| tool.name == "subagent_start");
        if !registered && runtime.sessions.delegation_repository().list(&caller).map_err(|e| e.to_string())?.is_empty()
        {
            return Ok(());
        }
        let client = self.client.get().ok_or("Connection closed")?.clone();
        if !self.delegation_sessions.lock().insert(parent.into()) {
            return Ok(());
        }
        let parent = parent.to_owned();
        let watched = self.delegation_sessions.clone();
        let runs = self.delegated_runs.clone();
        let runtime = runtime.clone();
        self.runtime.spawn(async move {
            let repository = runtime.sessions.delegation_repository();
            let mut previous = HashMap::<String, Value>::new();
            let mut wake_cursor = 0;
            let mut wake_runs = std::collections::HashSet::new();
            let mut known_children = std::collections::HashSet::new();
            let mut histories = HashMap::<String, ChildHistory>::new();
            let mut last_full = std::time::Instant::now();
            let mut interval = tokio::time::interval(Duration::from_millis(300));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = client.closed() => break, _ = interval.tick() => {} }
                if let Ok(events) = runtime.sessions.events_after(&parent, wake_cursor, 128) {
                    for event in events {
                        wake_cursor = event.seq;
                        if let Some(update) = wake_update(&parent, &event.kind, &event.data, &mut wake_runs) {
                            let _ = client.notify("session/update", json!({"sessionId":parent,"update":update}));
                        }
                    }
                }
                let Ok(mut children) = repository.list(&caller) else {
                    break;
                };
                children.sort_by_key(|child| child.depth);
                let refresh = last_full.elapsed() >= Duration::from_secs(5);
                if refresh {
                    last_full = std::time::Instant::now();
                }
                for child in children {
                    let child_id = format!("del_{}", child.id);
                    known_children.insert(child_id.clone());
                    runs.lock().insert(child_id.clone(), (child.caller.session_id.clone(), child.id.clone()));
                    let mut updates = card_updates(&child);
                    if let Ok(items) = runtime.sessions.list_user_delegation_followups(&caller, &child.id)
                        && let Ok(items) = serde_json::to_value(items)
                    {
                        for (_, update) in &mut updates {
                            if update["sessionUpdate"] == "subagent_update" {
                                update["queuedFollowups"] = items.clone();
                            }
                        }
                    }
                    for (owner, update) in updates {
                        let key = format!("{}:{}:{}:{}", owner, child_id, update["sessionUpdate"], update["messageId"]);
                        if refresh || previous.get(&key) != Some(&update) {
                            if client.notify("session/update", json!({"sessionId":owner,"update":update})).is_err() {
                                break;
                            }
                            previous.insert(key, update);
                        }
                    }
                    if let Ok(questions) = runtime.sessions.list_delegation_questions(&caller, &child.id) {
                        for question in questions {
                            let update = question_update(&child, &question);
                            let key = update["id"].as_str().unwrap_or_default().to_owned();
                            if refresh || previous.get(&key) != Some(&update) {
                                if client.notify("session/update", json!({"sessionId":parent,"update":update})).is_err()
                                {
                                    break;
                                }
                                previous.insert(key, update);
                            }
                        }
                    }
                    if !matches!(&child.request.target, Target::Minerva { .. }) {
                        continue;
                    }
                    let history = histories.entry(child_id.clone()).or_default();
                    if let Ok(events) = runtime.sessions.events_after(&child_id, history.cursor, 128) {
                        for event in events {
                            if event.kind == "message.upsert" {
                                for update in history_updates(&child, &child_id, &event.data).into_iter().flatten() {
                                    let _ =
                                        client.notify("session/update", json!({"sessionId":child_id,"update":update}));
                                    history.record(update);
                                }
                            }
                            history.cursor = event.seq;
                        }
                    }
                    if refresh {
                        // Session/load can temporarily suppress notifications while replaying.
                        for update in &history.messages {
                            let _ = client.notify("session/update", json!({"sessionId":child_id,"update":update}));
                        }
                    }
                }
            }
            watched.lock().remove(&parent);
            runs.lock().retain(|child, _| !known_children.contains(child));
        });
        Ok(())
    }
}

fn question_update(child: &DelegationSnapshot, question: &DelegationQuestion) -> Value {
    let item = &question.interaction;
    let chosen = match question.state {
        QuestionState::Pending | QuestionState::Delivering => None,
        QuestionState::Delivered => Some("Answered"),
        QuestionState::Expired => Some("Expired"),
        QuestionState::Unknown => Some("Delivery unknown"),
    };
    let id = format!("{}{}:{}", crate::async_questions::DELEGATED_QUESTION_PREFIX, child.id, item.id);
    let target = target_label(&child.request.target);
    let task = child.request.prompt.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
    let short_task: String = task.chars().take(48).collect();
    let header = if short_task.is_empty() {
        target.to_owned()
    } else {
        let suffix = if short_task.len() < task.len() { "…" } else { "" };
        format!("{target} · {short_task}{suffix}")
    };
    json!({"sessionUpdate":"user_question","id":id,"title":format!("{}: {}", target_label(&child.request.target),item.title),
        "chosen":chosen,"questions":[{"answerKey":"answer","header":header,"question":item.prompt,
        "multiSelect":false,"options":item.options.iter().map(|label| json!({"label":label})).collect::<Vec<_>>()}]
    })
}

#[derive(Default)]
struct ChildHistory {
    cursor: i64,
    messages: Vec<Value>,
    positions: HashMap<String, usize>,
}

impl ChildHistory {
    fn record(&mut self, update: Value) {
        let Some(id) = update["messageId"].as_str() else { return };
        if let Some(&index) = self.positions.get(id) {
            self.messages[index] = update;
        } else {
            self.positions.insert(id.to_owned(), self.messages.len());
            self.messages.push(update);
        }
    }
}

fn history_updates(child: &DelegationSnapshot, child_id: &str, message: &Value) -> Option<Vec<Value>> {
    let id = message["id"].as_str()?;
    let role = message["role"].as_str()?;
    if role == "user" && message["extensions"]["minerva.origin"] == "manager" {
        return None;
    }
    if !matches!(role, "user" | "assistant") {
        return None;
    }
    let (sender, recipient) = if role == "assistant" {
        (child_id, child.caller.session_id.as_str())
    } else {
        (child.caller.session_id.as_str(), child_id)
    };
    let reply = |segment: &str, text: &[&str]| {
        if role == "user" && message["extensions"]["minerva.origin"] == IDE_USER_FOLLOWUP_ORIGIN {
            return json!({"sessionUpdate":"minerva_user_followup","messageId":format!("followup:{segment}"),
                "text":text.join("\n"),"steered":false});
        }
        json!({
            "sessionUpdate":"session_message","messageId":segment,"senderSessionId":sender,
            "recipientSessionId":recipient,"content":[{"type":"text","text":text.join("\n")}]
        })
    };
    let mut segment = id.to_owned();
    let mut text = Vec::new();
    let mut updates = Vec::new();
    for part in message["parts"].as_array()? {
        if part["kind"] == "text" {
            text.extend(part["data"]["text"].as_str().filter(|value| !value.is_empty()));
        } else if role == "assistant"
            && part["kind"] == "extension"
            && part["data"]["source"] == "delegation-followup"
            && let (Some(work), Some(prompt)) = (part["data"]["workItemId"].as_str(), part["data"]["text"].as_str())
        {
            if !text.is_empty() {
                updates.push(reply(&segment, &text));
                text.clear();
            }
            updates.push(
                json!({"sessionUpdate":"minerva_user_followup","messageId":format!("followup:{work}"),"text":prompt,"steered":true}),
            );
            segment = format!("{id}:after:{work}");
        }
    }
    if !text.is_empty() {
        updates.push(reply(&segment, &text));
    }
    Some(updates)
}

fn wake_update(parent: &str, kind: &str, event: &Value, runs: &mut std::collections::HashSet<String>) -> Option<Value> {
    if kind == "message.upsert" && event["extensions"]["minerva.origin"] == "manager" {
        let input = event["id"].as_str()?;
        if let Some(run) = input.strip_suffix("_input") {
            runs.insert(run.to_owned());
        }
    }
    if kind != "run.updated" || event["status"] != "completed" {
        return None;
    }
    let run = event["runId"].as_str()?;
    if !runs.contains(run) {
        return None;
    }
    let text = event["parts"]
        .as_array()?
        .iter()
        .filter(|part| part["kind"] == "text")
        .filter_map(|part| part["data"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return None;
    }
    Some(json!({"sessionUpdate":"session_message","messageId":run,
        "senderSessionId":parent,"recipientSessionId":parent,"content":[{"type":"text","text":text}]}))
}

#[cfg(test)]
#[test]
fn only_trusted_manager_runs_project_a_completed_answer() {
    let mut runs = std::collections::HashSet::new();
    let done = json!({"runId":"wake_one","status":"completed","parts":[{"kind":"text","data":{"text":"Reviewed"}}]});
    assert!(wake_update("parent", "run.updated", &done, &mut runs).is_none());
    wake_update(
        "parent",
        "message.upsert",
        &json!({"id":"wake_one_input","extensions":{"minerva.origin":"manager"}}),
        &mut runs,
    );
    let update = wake_update("parent", "run.updated", &done, &mut runs).unwrap();
    assert_eq!(update["messageId"], "wake_one");
    assert_eq!(update["content"][0]["text"], "Reviewed");
    let mut conversation = crate::conversation::Conversation::default();
    conversation.apply(Some("parent"), &update, 1.0);
    let ids = conversation.main.message_ids();
    let mut restored = crate::conversation::Conversation::restore(conversation.main.entries, Vec::new());
    restored.main.restore_message_ids(ids);
    restored.apply(Some("parent"), &update, 2.0);
    assert_eq!(restored.main.entries.len(), 1);
}

fn card_updates(child: &DelegationSnapshot) -> Vec<(String, Value)> {
    let child_id = format!("del_{}", child.id);
    let state = match child.state {
        ActivationState::Succeeded => json!({"state":"idle","stopReason":"end_turn"}),
        ActivationState::Cancelled => json!({"state":"idle","stopReason":"cancelled"}),
        ActivationState::Failed => {
            json!({"state":"idle","stopReason":"error","error":{"message":child.completion.as_ref().map(|c| c.summary.as_str()).unwrap_or("Child failed")}})
        }
        ActivationState::Unknown => json!({"state":"requires_action"}),
        _ => json!({"state":"running"}),
    };
    let title = target_label(&child.request.target);
    let fallback = child
        .fallback
        .as_ref()
        .map(|selection| json!({"requested":target_label(&selection.requested_target),"reason":selection.reason}));
    let owner = child.caller.session_id.clone();
    let delivery_run_id =
        if child.state == ActivationState::Running && matches!(&child.request.target, Target::Minerva { .. }) {
            child.handle.as_ref().map(|handle| handle.run_id.as_str())
        } else {
            None
        };
    let mut updates = vec![(
        owner.clone(),
        json!({"sessionUpdate":"subagent_update","sessionId":child_id,
        "title":title,"description":child.request.prompt,"fallback":fallback,"deliveryRunId":delivery_run_id,"state":state,"capabilities":{"cancel":{}}}),
    )];
    updates.push((
        owner.clone(),
        json!({"sessionUpdate":"session_message","messageId":format!("{}_task",child.id),
        "senderSessionId":owner,"recipientSessionId":child_id,"content":[{"type":"text","text":child.request.prompt}]}),
    ));
    if let Some(completion) = &child.completion {
        updates.push((owner.clone(), json!({"sessionUpdate":"session_message","messageId":completion.result_id,
            "senderSessionId":child_id,"recipientSessionId":owner,"content":[{"type":"text","text":completion.summary}]})));
    }
    updates
}

fn target_label(target: &Target) -> &str {
    match target {
        Target::Minerva { route, .. } => route,
        Target::IdeAgent { agent_id, .. } => agent_id,
    }
}

pub(super) fn approval_agent(runtime: &Runtime, parent: &str, pending: &Value) -> Option<String> {
    let (activation, _) = pending["callId"].as_str()?.strip_prefix("delegation:")?.split_once(':')?;
    let run = pending["runId"].as_str()?;
    let child = runtime.sessions.delegation_active().ok()?.into_iter().find(|child| {
        child.activation_id == activation
            && child.caller.session_id == parent
            && child.request.origin_run.as_deref() == Some(run)
    })?;
    Some(
        match target_label(&child.request.target) {
            "codex" => "Codex",
            "claude" => "Claude Code",
            label => label,
        }
        .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn external_approval_label_is_bound_to_the_parent_run_and_child_activation() {
        let dir = tempfile::tempdir().unwrap();
        let service = ide_minerva::Service::new(dir.path().join("data"), dir.path().join("config"));
        let runtime = service.runtime().await.unwrap();
        let workspace = service.workspace(dir.path()).await.unwrap();
        runtime.create_session("parent", Some(&workspace.id)).unwrap();
        let cwd = runtime.plugins.harness.context.workspace_get("parent").unwrap().cwd.unwrap();
        let caller = runtime.sessions.delegation_caller("parent", &workspace.id, &cwd).unwrap();
        let request = serde_json::from_value(json!({
            "operationId":"write","originRun":"parent-run","prompt":"Write a file",
            "target":{"kind":"ide-agent","agentId":"codex","options":{}}
        }))
        .unwrap();
        let child = runtime.sessions.delegation_repository().admit(&caller, &request, &Default::default()).unwrap();
        let pending =
            json!({"callId":format!("delegation:{}:write",child.snapshot.activation_id),"runId":"parent-run"});
        assert_eq!(approval_agent(&runtime, "parent", &pending).as_deref(), Some("Codex"));
        assert_eq!(approval_agent(&runtime, "other-parent", &pending), None);
        let stale = json!({"callId":pending["callId"],"runId":"old-run"});
        assert_eq!(approval_agent(&runtime, "parent", &stale), None);
        service.shutdown().await.unwrap();
    }

    #[test]
    fn delegated_question_keeps_its_identity_when_delivery_settles_and_replays() {
        let child: DelegationSnapshot = serde_json::from_value(json!({
            "id":"child","workItemId":"work","activationId":"activation","depth":1,
            "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":"C:/project"},
            "request":{"operationId":"operation","target":{"kind":"minerva","route":"api:provider/model","effort":null},"prompt":"Inspect files"},
            "state":"running"
        })).unwrap();
        let mut question = DelegationQuestion {
            activation_id: "activation".into(),
            interaction: serde_json::from_value(json!({
                "id":"interaction","sessionId":"del_child","runId":"activation","callId":"ask",
                "kind":"question","title":"Question","prompt":"Which approach?","options":["One"],"createdAt":0
            }))
            .unwrap(),
            state: QuestionState::Pending,
        };
        let mut conversation = crate::conversation::Conversation::default();
        conversation.apply(None, &question_update(&child, &question), 0.0);
        assert!(matches!(&conversation.main.entries[0], crate::AgentEntry::Question { chosen: None, .. }));
        question.state = QuestionState::Delivered;
        conversation.apply(None, &question_update(&child, &question), 0.0);
        conversation.apply(None, &question_update(&child, &question), 0.0);
        assert_eq!(conversation.main.entries.len(), 1);
        assert!(matches!(&conversation.main.entries[0], crate::AgentEntry::Question { id, chosen: Some(chosen), .. }
            if id == "minerva-delegation-question:child:interaction" && chosen == "Answered"));
    }

    #[test]
    fn projection_reuses_child_conversation_and_replay_replaces_messages() {
        let child: DelegationSnapshot = serde_json::from_value(json!({
            "id":"child","workItemId":"work","activationId":"activation","depth":1,
            "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":"C:/project"},
            "request":{"operationId":"operation","target":{"kind":"minerva","route":"api:provider/model","effort":null},"prompt":"Inspect files"},
            "fallback":{"requestedTarget":{"kind":"ide-agent","agentId":"codex","options":{}},"reason":"Executable unavailable"},
            "state":"succeeded","handle":null,"completion":{"resultId":"result","state":"succeeded","summary":"Found two files"}
        })).unwrap();
        let mut conversation = crate::conversation::Conversation::default();
        let mut history = ChildHistory::default();
        let mut input = json!({"id":"input","role":"user","parts":[{"kind":"text","data":{"text":"Inspect files"}}]});
        assert!(history_updates(&child, "del_child", &input).is_some());
        input["extensions"] = json!({"minerva.origin":"manager"});
        assert!(history_updates(&child, "del_child", &input).is_none());
        let queued = json!({"id":"queued-input","role":"user",
            "extensions":{"minerva.origin":IDE_USER_FOLLOWUP_ORIGIN},
            "parts":[{"kind":"text","data":{"text":"Check one more file"}}]});
        for _ in 0..2 {
            for (_, update) in card_updates(&child) {
                conversation.apply(None, &update, 1.0);
            }
            for text in ["", "Found", "Found two files"] {
                let event = json!({"id":"reply","role":"assistant","parts":[
                    {"kind":"text","data":{"text":"Checking files"}},
                    {"kind":"extension","data":{"source":"delegation-followup","workItemId":"followup","text":"Narrow the search"}},
                    {"kind":"reasoning","data":{"text":"private"}},{"kind":"text","data":{"text":text}}
                ]});
                for update in history_updates(&child, "del_child", &event).unwrap() {
                    history.record(update.clone());
                    conversation.apply(Some("del_child"), &update, 1.0);
                }
            }
            for update in history_updates(&child, "del_child", &queued).unwrap() {
                history.record(update.clone());
                conversation.apply(Some("del_child"), &update, 1.0);
            }
            conversation = crate::conversation::Conversation::restore(
                conversation.main.entries.clone(),
                conversation.saved_subagents(),
            );
        }
        assert_eq!(conversation.main.entries.len(), 1);
        let crate::AgentEntry::Subagent { id, reply, prompt, status, fallback, .. } = &conversation.main.entries[0]
        else {
            panic!("Expected child card")
        };
        assert_eq!(id, "del_child");
        assert_eq!(reply.as_deref(), Some("Found two files"));
        assert_eq!(prompt.as_deref(), Some("Inspect files"));
        assert_eq!(*status, crate::AgentSubagentStatus::Done);
        let fallback = fallback.as_ref().unwrap();
        assert_eq!(fallback.requested, "codex");
        assert_eq!(fallback.reason, "Executable unavailable");
        let saved = conversation.saved_subagents();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].entries.len(), 4);
        assert_eq!(history.messages.len(), 4);
        assert!(matches!(&saved[0].entries[0], crate::AgentEntry::Message { text, .. } if text == "Checking files"));
        assert!(
            matches!(&saved[0].entries[1], crate::AgentEntry::User { text, steered: true, .. } if text == "Narrow the search")
        );
        assert!(matches!(&saved[0].entries[2], crate::AgentEntry::Message { text, .. } if text == "Found two files"));
        assert!(
            matches!(&saved[0].entries[3], crate::AgentEntry::User { text, steered: false, .. } if text == "Check one more file")
        );
    }
}
