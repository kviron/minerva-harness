use super::*;
use crate::{RunMessage, StartRunRequest};
use minerva_delegation::limits::Limits;

#[test]
fn question_wake_checks_the_current_branch_and_stop_generation_before_admission() {
    use super::wake::WakeQuestion;
    use minerva_plugin_sdk::tools::PendingInteraction;

    let dir = tempfile::tempdir().unwrap();
    let service = crate::sessions::SessionService::open(&dir.path().join("s.sqlite3")).unwrap();
    service.create("parent").unwrap();
    service.accept_user_run(&input("origin", None), "time").unwrap();
    service.fail_run("parent", "origin", false, "fixture").unwrap();
    let caller = caller();
    let repo = service.delegation_repository();
    let child = repo
        .admit(
            &caller,
            &StartRequest {
                operation_id: "child-op".into(),
                origin_run: Some("origin_input".into()),
                tool_registrations: Default::default(),
                prompt: "Ask".into(),
                target: Target::Minerva { route: "api:p/m".into(), effort: None },
            },
            &Limits::default(),
        )
        .unwrap()
        .snapshot;
    let child_session = format!("del_{}", child.id);
    let run_id = child.activation_id.clone();
    repo.started(&child.id, &run_id, &ExecutionHandle { session_id: child_session.clone(), run_id: run_id.clone() })
        .unwrap();
    let question = PendingInteraction {
        id: "question".into(),
        session_id: child_session,
        run_id: run_id.clone(),
        call_id: "call".into(),
        kind: "question".into(),
        title: "Question".into(),
        prompt: "Which?".into(),
        options: vec![],
        created_at: 0.0,
    };
    service.publish_delegation_question(&child.id, &run_id, &question).unwrap();
    let binding = WakeQuestion { child_id: child.id.clone(), question_id: question.id.clone() };
    service.accept_user_run(&input("unrelated", None), "time").unwrap();
    service.fail_run("parent", "unrelated", false, "fixture").unwrap();
    let token = service.delegation_wake_generation("parent").unwrap().unwrap();
    let wake = |head| input("wake_question_question", Some(head));
    assert!(service.accept_question_wake_run(&caller, token, &binding, &wake("unrelated_input"), "time").is_err());
    service.accept_user_run(&input("return", Some("origin_input")), "time").unwrap();
    service.fail_run("parent", "return", false, "fixture").unwrap();
    let token = service.delegation_wake_generation("parent").unwrap().unwrap();
    let request = wake("return_input");
    service.delegation_user_stop("parent").unwrap();
    assert!(service.accept_question_wake_run(&caller, token, &binding, &request, "time").is_err());
    service.accept_user_run(&input("resume", Some("return_input")), "time").unwrap();
    service.fail_run("parent", "resume", false, "fixture").unwrap();
    let token = service.delegation_wake_generation("parent").unwrap().unwrap();
    let request = wake("resume_input");
    assert!(service.accept_question_wake_run(&caller, token, &binding, &request, "time").unwrap());
    assert!(!service.accept_question_wake_run(&caller, token, &binding, &request, "time").unwrap());
    let stored = service.load("parent").unwrap();
    let wake = stored.iter().find(|message| message.id == request.message.id).unwrap();
    assert_eq!(wake.extensions["minerva.origin"], "manager");
    assert_eq!(wake.extensions["minerva.delegation.question"]["questionId"], "question");
    service.fail_run("parent", &request.request_id, false, "fixture").unwrap();

    let answered = PendingInteraction { id: "already-answered".into(), ..question.clone() };
    service.publish_delegation_question(&child.id, &run_id, &answered).unwrap();
    service.claim_delegation_answer(&caller, &child.id, &answered.id, "answer", "Selected").unwrap();
    service.settle_delegation_answer(&child.id, &answered.id, "answer", true).unwrap();
    let rejected = input("wake_question_already-answered", Some(&request.message.id));
    let binding = WakeQuestion { child_id: child.id.clone(), question_id: answered.id };
    let error = service.accept_question_wake_run(&caller, token, &binding, &rejected, "time").unwrap_err();
    assert!(error.contains("no longer pending"), "{error}");

    let stopped = PendingInteraction { id: "stop-tree".into(), ..question };
    service.publish_delegation_question(&child.id, &run_id, &stopped).unwrap();
    repo.request_stop_tree(&caller).unwrap();
    let rejected = input("wake_question_stop-tree", Some(&request.message.id));
    let binding = WakeQuestion { child_id: child.id, question_id: stopped.id };
    let error = service.accept_question_wake_run(&caller, token, &binding, &rejected, "time").unwrap_err();
    assert!(error.contains("tree is stopped"), "{error}");
}

fn caller() -> Caller {
    Caller {
        session_id: "parent".into(),
        root_session_id: "parent".into(),
        workspace_id: "w".into(),
        cwd: "C:/p".into(),
    }
}
fn input(id: &str, parent: Option<&str>) -> StartRunRequest {
    StartRunRequest {
        session_id: "parent".into(),
        request_id: id.into(),
        parent_id: parent.map(String::from),
        message: RunMessage { id: format!("{id}_input"), text: "continue".into() },
        model: "api:p/m".into(),
        options: serde_json::json!({}),
    }
}

#[test]
fn user_stop_survives_restart_and_only_user_admission_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let service = crate::sessions::SessionService::open(&path).unwrap();
    service.create("parent").unwrap();
    service.delegation_user_stop("parent").unwrap();
    drop(service);
    let service = crate::sessions::SessionService::open(&path).unwrap();
    assert_eq!(service.delegation_wake_generation("parent").unwrap(), None);
    service.accept_run(&input("internal", None), "time").unwrap();
    assert_eq!(service.delegation_wake_generation("parent").unwrap(), None);
    service.fail_run("parent", "internal", true, "stop").unwrap();
    service.accept_user_run(&input("user", Some("internal_input")), "time").unwrap();
    assert!(service.delegation_wake_generation("parent").unwrap().is_some());
}

#[test]
fn wake_admission_is_generation_fenced_and_atomically_binds_notice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let service = crate::sessions::SessionService::open(&path).unwrap();
    service.create("parent").unwrap();
    service.accept_user_run(&input("origin", None), "time").unwrap();
    service.fail_run("parent", "origin", false, "complete test fixture").unwrap();
    let request = StartRequest {
        tool_registrations: Default::default(),
        operation_id: "op".into(),
        target: Target::Minerva { route: "api:p/m".into(), effort: None },
        prompt: "look".into(),
        origin_run: Some("origin_input".into()),
    };
    let repo = service.delegation_repository();
    let child = repo.admit(&caller(), &request, &Limits::default()).unwrap().snapshot;
    repo.settle(
        &child.id,
        &child.activation_id,
        &Completion { result_id: "r".into(), state: ActivationState::Succeeded, summary: "done".into() },
    )
    .unwrap();
    let consumer = service.delegation_consumer(&caller(), "owner").unwrap();
    service.delegation_receive_results(&caller(), consumer, 8).unwrap();
    let token = service.delegation_wake_generation("parent").unwrap().unwrap();
    service.delegation_user_stop("parent").unwrap();
    let wake = input("wake", Some("origin_input"));
    assert!(service.accept_wake_run(&caller(), token, &wake, "time").is_err());
    assert_eq!(service.delegation_receive_results(&caller(), consumer, 8).unwrap().len(), 1);
    service.accept_user_run(&input("resume", Some("origin_input")), "time").unwrap();
    assert!(service.delegation_receive_results(&caller(), consumer, 8).unwrap().is_empty());
    assert_eq!(service.load("parent").unwrap().iter().filter(|m| m.id == "wake_input").count(), 0);
}

#[test]
fn accepted_wake_replays_without_duplicate_input_and_keeps_manager_origin() {
    let dir = tempfile::tempdir().unwrap();
    let service = crate::sessions::SessionService::open(&dir.path().join("s.sqlite3")).unwrap();
    service.create("parent").unwrap();
    service.accept_user_run(&input("origin", None), "time").unwrap();
    service.fail_run("parent", "origin", false, "fixture").unwrap();
    let request = StartRequest {
        tool_registrations: Default::default(),
        operation_id: "op".into(),
        target: Target::Minerva { route: "api:p/m".into(), effort: None },
        prompt: "look".into(),
        origin_run: Some("origin_input".into()),
    };
    let repo = service.delegation_repository();
    let mut unrelated = request.clone();
    unrelated.operation_id = "other_branch".into();
    unrelated.origin_run = Some("unrelated_input".into());
    let skipped = repo.admit(&caller(), &unrelated, &Limits::default()).unwrap().snapshot;
    repo.settle(
        &skipped.id,
        &skipped.activation_id,
        &Completion { result_id: "skipped".into(), state: ActivationState::Succeeded, summary: "other branch".into() },
    )
    .unwrap();
    let child = repo.admit(&caller(), &request, &Limits::default()).unwrap().snapshot;
    repo.settle(
        &child.id,
        &child.activation_id,
        &Completion { result_id: "r".into(), state: ActivationState::Succeeded, summary: "done".into() },
    )
    .unwrap();
    let consumer = service.delegation_consumer(&caller(), "owner").unwrap();
    service.delegation_receive_results(&caller(), consumer, 8).unwrap();
    service
        .store
        .with_delegation_db(|conn| {
            conn.execute(
                "UPDATE delegation_children SET activation_id='next', snapshot_json='{}', active=1 WHERE id=?1",
                [&child.id],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        service.delegation_unbound_results().unwrap().iter().map(|item| &item.activation_id).collect::<Vec<_>>(),
        vec![&child.activation_id]
    );
    let token = service.delegation_wake_generation("parent").unwrap().unwrap();
    let wake = input("wake", Some("origin_input"));
    assert!(service.accept_wake_run(&caller(), token, &wake, "time").unwrap());
    assert!(!service.accept_wake_run(&caller(), token, &wake, "time").unwrap());
    assert_eq!(service.delegation_receive_results(&caller(), consumer, 8).unwrap()[0].result_id, "skipped");
    assert!(service.delegation_unbound_results().unwrap().is_empty());
    let history = service.load("parent").unwrap();
    let notices: Vec<_> = history.iter().filter(|m| m.id == "wake_input").collect();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].extensions["minerva.origin"], "manager");
}
