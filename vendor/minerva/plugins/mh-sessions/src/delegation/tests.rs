use super::*;
use minerva_delegation::{limits::Limits, repository::RepositoryPort};

#[test]
fn child_run_rejects_mismatched_activation_and_stopped_tree_before_writing_history() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = crate::sessions::SessionService::open(&dir.path().join("sessions.sqlite3")).unwrap();
    let root = caller();
    sessions.create(&root.session_id).unwrap();
    let mut delegation = request();
    delegation.target = Target::Minerva { route: "api:fixture/model".into(), effort: None };
    let child = sessions.store.admit(&root, &delegation, &Limits::default()).unwrap().snapshot;
    let session_id = format!("del_{}", child.id);
    sessions.create(&session_id).unwrap();
    let mut run = crate::StartRunRequest {
        session_id: session_id.clone(),
        request_id: "stale_activation".into(),
        parent_id: None,
        message: crate::RunMessage { id: format!("{}_input", child.activation_id), text: delegation.prompt },
        model: "api:fixture/model".into(),
        options: serde_json::json!({
            "liveSelection": false,
            "requireCurrentHead": false,
            "delegationToolRegistrations": {"unexpected": 1},
        }),
    };
    assert!(sessions.accept_delegated_child_run(&run, "time").is_err());
    run.request_id = child.activation_id;
    assert!(!sessions.confirms_child_stopped_before_run(&child.id, &run.request_id).unwrap());
    assert!(sessions.accept_delegated_child_run(&run, "time").is_err());
    run.options["delegationToolRegistrations"] = serde_json::to_value(delegation.tool_registrations).unwrap();
    sessions.store.request_stop_tree(&root).unwrap();
    assert!(sessions.accept_delegated_child_run(&run, "time").is_err());
    assert!(sessions.confirms_child_stopped_before_run(&child.id, &run.request_id).unwrap());
    assert!(sessions.load(&session_id).unwrap().is_empty());
}

#[test]
fn user_followup_projection_replays_unresolved_items_and_excludes_model_messages() {
    use minerva_delegation::IDE_USER_OPERATION_PREFIX;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    sessions.create("parent").unwrap();
    let db = &sessions.store;
    let root = caller();
    let limits = Limits::default();
    let child = db.admit(&root, &request(), &limits).unwrap().snapshot;
    let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
    db.started(&child.id, &child.activation_id, &handle).unwrap();
    let enqueue = |operation: &str, prompt: &str| {
        let mut next = request();
        next.operation_id = operation.into();
        next.prompt = prompt.into();
        db.enqueue(&root, &child.id, &next, &limits).unwrap()
    };
    enqueue("IDE-USER-model", "Model-only message");
    let queued = enqueue(&format!("{IDE_USER_OPERATION_PREFIX}queued"), "Queue this");
    let claimed = enqueue(&format!("{IDE_USER_OPERATION_PREFIX}claimed"), "Send during the run");
    let confirmed = enqueue(&format!("{IDE_USER_OPERATION_PREFIX}confirmed"), "Confirm this");
    for id in [&claimed.work_item_id, &confirmed.work_item_id] {
        assert!(db.prefer_delegation_followup_in_run(&child.id, &handle, id).unwrap());
        assert!(db.claim_delegation_followup(&child.id, &handle, id).unwrap().is_some());
    }
    assert!(db.confirm_delegation_followup(&child.id, &handle, &confirmed.work_item_id).unwrap());
    let expected = [
        UserFollowup { id: queued.work_item_id, prompt: "Queue this".into(), state: UserFollowupState::Queued },
        UserFollowup {
            id: claimed.work_item_id,
            prompt: "Send during the run".into(),
            state: UserFollowupState::Requested,
        },
    ];
    assert_eq!(sessions.list_user_delegation_followups(&root, &child.id).unwrap(), expected);
    let foreign = Caller { workspace_id: "another-workspace".into(), ..root.clone() };
    assert!(sessions.list_user_delegation_followups(&foreign, &child.id).is_err());
    drop(sessions);

    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    let mut expected = expected;
    expected[1].state = UserFollowupState::Unknown;
    assert_eq!(sessions.list_user_delegation_followups(&root, &child.id).unwrap(), expected);
}

#[test]
fn v13_question_migration_preserves_delegation_and_queued_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let db = store(&path);
    let caller = caller();
    let limits = Limits::default();
    let child = db.admit(&caller, &request(), &limits).unwrap().snapshot;
    let mut next = request();
    next.operation_id = "queued".into();
    next.prompt = "Check the result".into();
    let receipt = db.enqueue(&caller, &child.id, &next, &limits).unwrap();
    db.with_delegation_db(|conn| {
        conn.execute_batch(
            "DROP TABLE delegation_questions;
             DROP INDEX delegation_deadline;
             ALTER TABLE delegation_children DROP COLUMN admitted_at_ms;
             ALTER TABLE delegation_children DROP COLUMN deadline_stop;
             DROP INDEX delegation_followup_child;
             ALTER TABLE delegation_followup_queue DROP COLUMN delivery_state;
             ALTER TABLE delegation_followup_queue DROP COLUMN delivery_run;
             ALTER TABLE delegation_followup_queue DROP COLUMN preferred_run;
             CREATE INDEX delegation_followup_child ON delegation_followup_queue(delegation_id,sequence)
                 WHERE cancelled=0 AND activation_id IS NULL;
             PRAGMA user_version=13;",
        )
        .map_err(storage)
    })
    .unwrap();
    drop(db);

    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    let repo = sessions.delegation_repository();
    assert_eq!(repo.get(&caller, &child.id).unwrap(), child);
    assert_eq!(repo.lookup_queued(&caller, &child.id, &next.operation_id, &next.prompt).unwrap(), Some(receipt));
    assert!(sessions.list_delegation_questions(&caller, &child.id).unwrap().is_empty());
}

#[test]
fn claimed_followup_is_not_promoted_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let db = store(&path);
    let parent = caller();
    let limits = Limits::default();
    let child = db.admit(&parent, &request(), &limits).unwrap().snapshot;
    let mut followup = request();
    followup.operation_id = "claimed".into();
    let queued = db.enqueue(&parent, &child.id, &followup, &limits).unwrap();
    let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
    db.started(&child.id, &child.activation_id, &handle).unwrap();
    let wrong = ExecutionHandle { run_id: "another-run".into(), ..handle.clone() };
    assert!(!db.prefer_delegation_followup_in_run(&child.id, &wrong, &queued.work_item_id).unwrap());
    assert_eq!(db.claim_delegation_followup(&child.id, &handle, &queued.work_item_id).unwrap(), None);
    assert!(db.prefer_delegation_followup_in_run(&child.id, &handle, &queued.work_item_id).unwrap());
    assert_eq!(db.claim_delegation_followup(&child.id, &wrong, &queued.work_item_id).unwrap(), None);
    assert_eq!(
        db.claim_delegation_followup(&child.id, &handle, &queued.work_item_id).unwrap(),
        Some(followup.prompt.clone())
    );
    assert!(db.prefer_delegation_followup_in_run(&child.id, &handle, &queued.work_item_id).unwrap());
    assert!(!db.prefer_delegation_followup_in_run(&child.id, &wrong, &queued.work_item_id).unwrap());
    assert_eq!(db.claim_delegation_followup(&child.id, &handle, &queued.work_item_id).unwrap(), None);
    assert!(!db.confirm_delegation_followup(&child.id, &wrong, &queued.work_item_id).unwrap());

    followup.operation_id = "confirmed".into();
    let confirmed = db.enqueue(&parent, &child.id, &followup, &limits).unwrap();
    assert!(db.prefer_delegation_followup_in_run(&child.id, &handle, &confirmed.work_item_id).unwrap());
    assert!(db.claim_delegation_followup(&child.id, &handle, &confirmed.work_item_id).unwrap().is_some());
    assert!(db.confirm_delegation_followup(&child.id, &handle, &confirmed.work_item_id).unwrap());
    assert!(db.prefer_delegation_followup_in_run(&child.id, &handle, &confirmed.work_item_id).unwrap());
    assert!(db.claim_delegation_followup(&child.id, &handle, &confirmed.work_item_id).unwrap().is_none());
    assert!(!db.confirm_delegation_followup(&child.id, &handle, &confirmed.work_item_id).unwrap());
    let result = Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "Done".into() };
    db.settle(&child.id, &child.activation_id, &result).unwrap();
    drop(db);

    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    let db = &sessions.store;
    assert!(db.promote_delegation_followup(&parent, &child.id, &Default::default(), &limits).unwrap().is_none());
    let state: String = db
        .with_delegation_db(|conn| {
            conn.query_row(
                "SELECT delivery_state FROM delegation_followup_queue WHERE work_item_id=?1",
                [&queued.work_item_id],
                |row| row.get(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_eq!(state, "unknown");
    let state: String = db
        .with_delegation_db(|conn| {
            conn.query_row(
                "SELECT delivery_state FROM delegation_followup_queue WHERE work_item_id=?1",
                [&confirmed.work_item_id],
                |row| row.get(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_eq!(state, "delivered");
}

#[test]
fn question_answers_are_bound_to_the_child_and_recover_without_redelivery() {
    use super::questions::QuestionState;
    use minerva_plugin_sdk::tools::PendingInteraction;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("questions.sqlite3");
    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    sessions.create("parent").unwrap();
    let repo = sessions.delegation_repository();
    let parent = caller();
    let child = repo.admit(&parent, &request(), &Limits::default()).unwrap().snapshot;
    let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
    repo.started(&child.id, &child.activation_id, &handle).unwrap();
    let question = |id: &str| PendingInteraction {
        id: id.into(),
        session_id: handle.session_id.clone(),
        run_id: handle.run_id.clone(),
        call_id: format!("call_{id}"),
        kind: "question".into(),
        title: "Clarify choice".into(),
        prompt: "Which option?".into(),
        options: vec!["one".into()],
        created_at: 0.0,
    };
    let question_one = question("one");
    assert!(sessions.publish_delegation_question(&child.id, "old-activation", &question_one).is_err());
    let mut forged = question_one.clone();
    forged.run_id = "other-run".into();
    assert!(sessions.publish_delegation_question(&child.id, &child.activation_id, &forged).is_err());
    forged.run_id = handle.run_id.clone();
    forged.kind = "approval".into();
    assert!(sessions.publish_delegation_question(&child.id, &child.activation_id, &forged).is_err());
    for id in ["one", "two", "three", "expired"] {
        sessions.publish_delegation_question(&child.id, &child.activation_id, &question(id)).unwrap();
    }
    sessions.publish_delegation_question(&child.id, &child.activation_id, &question_one).unwrap();
    assert_eq!(sessions.list_delegation_questions(&parent, &child.id).unwrap().len(), 4);
    sessions.expire_delegation_question(&child.id, "wrong-activation", "expired").unwrap();
    assert_eq!(sessions.delegation_question(&parent, &child.id, "expired").unwrap().state, QuestionState::Pending);
    sessions.expire_delegation_question(&child.id, &child.activation_id, "expired").unwrap();
    assert!(sessions.claim_delegation_answer(&parent, &child.id, "expired", "too-late", "one").is_err());
    let foreign = Caller { session_id: "foreign".into(), ..parent.clone() };
    assert_eq!(
        sessions.claim_delegation_answer(&foreign, &child.id, "one", "answer-1", "one").err().unwrap().code,
        ErrorCode::Forbidden,
    );
    assert_eq!(
        sessions.claim_delegation_answer(&parent, &child.id, "one", "answer-1", "bad").err().unwrap().code,
        ErrorCode::Invalid,
    );
    let claim = |id, op| sessions.claim_delegation_answer(&parent, &child.id, id, op, "one").unwrap();
    assert!(claim("one", "answer-1").fresh);
    assert!(!claim("one", "answer-1").fresh);
    assert!(sessions.claim_delegation_answer(&parent, &child.id, "one", "other", "one").is_err());
    sessions.settle_delegation_answer(&child.id, "one", "answer-1", true).unwrap();
    assert_eq!(claim("one", "answer-1").question.state, QuestionState::Delivered);
    assert!(claim("two", "answer-2").fresh);
    drop(repo);
    drop(sessions);

    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    let replay = sessions.claim_delegation_answer(&parent, &child.id, "two", "answer-2", "one").unwrap();
    assert!(!replay.fresh);
    assert_eq!(replay.question.state, QuestionState::Unknown);
    assert_eq!(sessions.delegation_question(&parent, &child.id, "three").unwrap().state, QuestionState::Expired);
    assert_eq!(sessions.delegation_question(&parent, &child.id, "one").unwrap().state, QuestionState::Delivered);
}

#[test]
fn unknown_diagnostic_is_bounded_deduplicated_and_transactional() {
    let dir = tempfile::tempdir().unwrap();
    let service = crate::sessions::SessionService::open(&dir.path().join("s.sqlite3")).unwrap();
    service.create("parent").unwrap();
    let repo = service.delegation_repository();
    let snapshot = repo.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    let reason = "РЎРЏ".repeat(2048);
    repo.mark_unknown(&snapshot.id, &snapshot.activation_id, &reason).unwrap();
    repo.mark_unknown(&snapshot.id, &snapshot.activation_id, &reason).unwrap();
    let events = service.events_after("parent", 0, 100).unwrap();
    let diagnostics: Vec<_> = events.iter().filter(|e| e.kind == "delegation.diagnostic").collect();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].data["reason"].as_str().unwrap().chars().count(), 1024);
    assert_eq!(repo.get(&caller(), &snapshot.id).unwrap().state, ActivationState::Unknown);
}

fn caller() -> Caller {
    Caller {
        session_id: "parent".into(),
        root_session_id: "parent".into(),
        workspace_id: "workspace".into(),
        cwd: "C:/project".into(),
    }
}
fn request() -> StartRequest {
    StartRequest {
        origin_run: None,
        tool_registrations: Default::default(),
        operation_id: "op".into(),
        target: Target::Minerva { route: "api:test/model".into(), effort: None },
        prompt: "Research".into(),
    }
}
fn store(path: &std::path::Path) -> SessionStore {
    let db = SessionStore::open(path).unwrap();
    db.create("parent").unwrap();
    db
}

#[test]
fn admission_is_durable_idempotent_and_payload_bound() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let db = store(&path);
    let first = db.admit(&caller(), &request(), &Limits::default()).unwrap();
    assert!(first.fresh);
    assert!(!db.admit(&caller(), &request(), &Limits::default()).unwrap().fresh);
    let mut refreshed = request();
    refreshed.tool_registrations.insert("read_file".into(), 42);
    let replay = db.admit(&caller(), &refreshed, &Limits::default()).unwrap();
    assert!(!replay.fresh);
    assert_eq!(replay.snapshot, first.snapshot);
    let mut changed = request();
    changed.prompt = "Different".into();
    assert_eq!(db.admit(&caller(), &changed, &Limits::default()).err().unwrap().code, ErrorCode::Conflict);
    drop(db);
    let db = SessionStore::open(&path).unwrap();
    assert_eq!(RepositoryPort::get(&db, &caller(), &first.snapshot.id).unwrap(), first.snapshot);
}

#[test]
fn completion_and_outbox_are_atomic_and_terminal_is_monotonic() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("s.sqlite3"));
    let s = db.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    let result = Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "Done".into() };
    assert!(db.settle(&s.id, "stale", &result).is_err());
    db.settle(&s.id, &s.activation_id, &result).unwrap();
    db.settle(&s.id, &s.activation_id, &result).unwrap();
    db.mark_unknown(&s.id, &s.activation_id, "disconnected").unwrap();
    assert_eq!(RepositoryPort::get(&db, &caller(), &s.id).unwrap().state, ActivationState::Succeeded);
    assert_eq!(db.delegation_pending_results(&caller()).unwrap(), vec![result]);
}

#[test]
fn migrated_completion_survives_replacing_current_activation() {
    for version in [8, 9] {
        check_migrated_completion(version);
    }
}

fn check_migrated_completion(version: i64) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let db = store(&path);
    let child = db.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    let result = Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "Done".into() };
    db.settle(&child.id, &child.activation_id, &result).unwrap();
    db.with_delegation_db(|conn| {
        conn.execute_batch(
            "DROP TABLE delegation_questions; DROP TABLE delegation_followup_queue; DROP TABLE delegation_stopped_trees;
             DROP INDEX delegation_deadline;
             ALTER TABLE delegation_children DROP COLUMN admitted_at_ms;
             ALTER TABLE delegation_children DROP COLUMN deadline_stop;
             ALTER TABLE delegation_operations DROP COLUMN activation_id;",
        )
        .map_err(storage)?;
        if version == 8 {
            conn.execute_batch("DROP TABLE delegation_terminal_snapshots;").map_err(storage)?;
        }
        conn.pragma_update(None, "user_version", version).map_err(storage)
    })
    .unwrap();
    drop(db);

    let service = crate::sessions::SessionService::open(&path).unwrap();
    let mut next = child.clone();
    next.activation_id = "next".into();
    next.work_item_id = "next_work".into();
    next.request.operation_id = "followup".into();
    next.request.prompt = "Check the result".into();
    service
        .store
        .with_delegation_db(|conn| {
            conn.execute(
                "UPDATE delegation_children SET activation_id=?2, snapshot_json=?3, active=1 WHERE id=?1",
                rusqlite::params![child.id, next.activation_id, serde_json::to_string(&next).map_err(storage)?],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let replay = service.delegation_repository().admit(&caller(), &request(), &Limits::default()).unwrap();
    assert!(!replay.fresh);
    assert_eq!(replay.snapshot.activation_id, child.activation_id);
    assert_eq!(replay.snapshot.completion.as_ref(), Some(&result));
    let pending = service.delegation_pending().unwrap();
    assert_eq!(pending[0].activation_id, child.activation_id);
    let generation = service.delegation_consumer(&caller(), "worker").unwrap();
    let notices = service.delegation_receive_results(&caller(), generation, 10).unwrap();
    assert_eq!(notices[0].result_id, result.result_id);
    assert!(service.delegation_pending().unwrap().is_empty());
}

#[test]
fn child_deadline_age_and_stop_intent_survive_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let db = store(&path);
    let caller = caller();
    let child = db.admit(&caller, &request(), &Limits::default()).unwrap().snapshot;
    let handle = ExecutionHandle { session_id: "native-thread".into(), run_id: "native-run".into() };
    db.started(&child.id, &child.activation_id, &handle).unwrap();
    db.with_delegation_db(|conn| {
        conn.execute(
            "UPDATE delegation_children SET admitted_at_ms=CAST(unixepoch('subsec')*1000 AS INTEGER)-30000 WHERE id=?1",
            [&child.id],
        )
        .map_err(storage)?;
        Ok(())
    })
    .unwrap();
    drop(db);

    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    let expired = sessions.delegation_deadline_stops(10).unwrap();
    let [stopping] = expired.as_slice() else { panic!("expected one expired activation") };
    assert_eq!(stopping.activation_id, child.activation_id);
    assert_eq!(stopping.state, ActivationState::Stopping);
    assert_eq!(stopping.handle.as_ref(), Some(&handle));
    assert_eq!(sessions.delegation_active().unwrap(), expired);
    drop(sessions);
    let sessions = crate::sessions::SessionService::open(&path).unwrap();
    assert_eq!(sessions.delegation_deadline_stops(0).unwrap(), expired);
}

#[test]
fn followup_reserves_one_attempt_with_intersected_tool_scope() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("s.sqlite3"));
    let limits = Limits { children_per_root: 1, active_per_root: 1, ..Limits::default() };
    let mut first = request();
    first.tool_registrations.extend([("read".into(), 1), ("search".into(), 2)]);
    let child = db.admit(&caller(), &first, &limits).unwrap().snapshot;
    let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
    db.started(&child.id, &child.activation_id, &handle).unwrap();
    let result = Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "Done".into() };
    db.settle(&child.id, &child.activation_id, &result).unwrap();
    let mut next = request();
    next.operation_id = "followup".into();
    next.prompt = "Continue".into();
    next.tool_registrations.extend([("read".into(), 1), ("search".into(), 3), ("extra".into(), 4)]);
    db.with_delegation_db(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_followup BEFORE INSERT ON delegation_operations
         WHEN NEW.operation_id='followup' BEGIN SELECT RAISE(ABORT,'test failure'); END;",
        )
        .map_err(storage)
    })
    .unwrap();
    assert!(db.admit_followup(&caller(), &child.id, &next, &limits).is_err());
    assert_eq!(RepositoryPort::get(&db, &caller(), &child.id).unwrap().activation_id, child.activation_id);
    db.with_delegation_db(|conn| conn.execute_batch("DROP TRIGGER reject_followup;").map_err(storage)).unwrap();
    let reserved = db.admit_followup(&caller(), &child.id, &next, &limits).unwrap();
    assert!(reserved.fresh);
    assert_eq!(reserved.snapshot.id, child.id);
    assert_ne!(reserved.snapshot.activation_id, child.activation_id);
    assert_ne!(reserved.snapshot.work_item_id, child.work_item_id);
    assert!(reserved.snapshot.handle.is_none());
    assert_eq!(reserved.snapshot.previous_handle.as_ref(), Some(&handle));
    assert_eq!(reserved.snapshot.request.tool_registrations, [("read".into(), 1)].into());
    assert!(!db.admit_followup(&caller(), &child.id, &next, &limits).unwrap().fresh);
    let mut competitor = next.clone();
    competitor.operation_id = "competitor".into();
    assert_eq!(db.admit_followup(&caller(), &child.id, &competitor, &limits).err().unwrap().code, ErrorCode::Busy);
    assert_eq!(db.admit(&caller(), &next, &limits).err().unwrap().code, ErrorCode::Conflict);
    assert_eq!(db.admit(&caller(), &first, &limits).unwrap().snapshot.activation_id, child.activation_id);
    assert!(db.settle(&child.id, &child.activation_id, &result).is_err());
    next.prompt = "Different".into();
    assert_eq!(db.admit_followup(&caller(), &child.id, &next, &limits).err().unwrap().code, ErrorCode::Conflict);
    assert_eq!(db.delegation_pending_results(&caller()).unwrap(), vec![result]);
}

#[test]
fn nested_followups_apply_the_current_root_and_direct_parent_limits() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("s.sqlite3"));
    let root = caller();
    let limits = Limits::default();
    let parent = db.admit(&root, &request(), &limits).unwrap().snapshot;
    let parent_session = format!("del_{}", parent.id);
    db.create(&parent_session).unwrap();
    let caller = Caller { session_id: parent_session, ..root.clone() };
    let mut initial = request();
    initial.tool_registrations = [("read_file".into(), 1), ("search_files".into(), 2)].into();
    let child = db.admit(&caller, &initial, &limits).unwrap().snapshot;
    let handle = |run: &str| ExecutionHandle { session_id: format!("del_{}", child.id), run_id: run.into() };
    db.started(&child.id, &child.activation_id, &handle(&child.activation_id)).unwrap();
    for (session, allowed) in
        [(&root.session_id, vec!["search_files"]), (&caller.session_id, vec!["read_file", "search_files"])]
    {
        db.accept_run(
            &crate::StartRunRequest {
                session_id: session.clone(),
                request_id: "latest".into(),
                parent_id: None,
                message: crate::RunMessage { id: "latest_input".into(), text: "Narrow".into() },
                model: "api:p/m".into(),
                options: serde_json::json!({"toolAllowlist":allowed}),
            },
            "time",
        )
        .unwrap();
    }
    let mut next = initial.clone();
    next.operation_id = "queued".into();
    db.enqueue(&caller, &child.id, &next, &limits).unwrap();
    db.settle(
        &child.id,
        &child.activation_id,
        &Completion { result_id: "first-result".into(), state: ActivationState::Succeeded, summary: "Done".into() },
    )
    .unwrap();
    let promoted = db.promote(&caller, &child.id, &next.tool_registrations, &limits).unwrap().unwrap();
    let narrowed = [("search_files".into(), 2)].into();
    assert_eq!(promoted.request.tool_registrations, narrowed);
    db.started(&child.id, &promoted.activation_id, &handle(&promoted.activation_id)).unwrap();
    db.settle(
        &child.id,
        &promoted.activation_id,
        &Completion { result_id: "second-result".into(), state: ActivationState::Succeeded, summary: "Done".into() },
    )
    .unwrap();
    db.fail_run(&root.session_id, "latest", false, "fixture").unwrap();
    db.accept_run(
        &crate::StartRunRequest {
            session_id: root.session_id.clone(),
            request_id: "narrower".into(),
            parent_id: Some("latest_input".into()),
            message: crate::RunMessage { id: "narrower_input".into(), text: "Narrow further".into() },
            model: "api:p/m".into(),
            options: serde_json::json!({"toolAllowlist":[]}),
        },
        "time",
    )
    .unwrap();
    next.operation_id = "immediate".into();
    let immediate = db.admit_followup(&caller, &child.id, &next, &limits).unwrap();
    assert!(immediate.snapshot.request.tool_registrations.is_empty());
}

#[test]
fn tree_stop_fences_admission_without_affecting_other_roots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let db = store(&path);
    let parent = caller();
    let limits = Limits { max_depth: 2, ..Limits::default() };
    let child = db.admit(&parent, &request(), &limits).unwrap().snapshot;
    let child_id = format!("del_{}", child.id);
    db.create(&child_id).unwrap();
    let child_caller = Caller { session_id: child_id, ..parent.clone() };
    let grandchild = db.admit(&child_caller, &request(), &limits).unwrap().snapshot;
    assert_eq!(db.request_stop_tree(&child_caller).err().unwrap().code, ErrorCode::Forbidden);
    let stopped = db.request_stop_tree(&parent).unwrap();
    assert_eq!(stopped.iter().map(|s| &s.id).collect::<Vec<_>>(), [&child.id, &grandchild.id]);
    assert!(stopped.iter().all(|s| s.state == ActivationState::Stopping));
    assert!(!db.admit(&parent, &request(), &limits).unwrap().fresh);
    drop(db);

    let service = crate::sessions::SessionService::open(&path).unwrap();
    assert_eq!(
        service.internal_delegation_ancestors(&format!("del_{}", grandchild.id)).unwrap(),
        [child_caller.session_id.as_str(), parent.session_id.as_str()]
    );
    assert!(service.internal_delegation_ancestors(&parent.session_id).unwrap().is_empty());
    let grandchild_session = format!("del_{}", grandchild.id);
    assert_eq!(
        service.internal_delegation_descendants(&child_caller.session_id).unwrap(),
        [grandchild_session.as_str()]
    );
    let descendants = service.internal_delegation_descendants(&parent.session_id).unwrap();
    assert_eq!(descendants.len(), 2);
    assert!(descendants.contains(&child_caller.session_id));
    assert!(descendants.contains(&grandchild_session));
    assert!(service.internal_delegation_descendants("unrelated").unwrap().is_empty());
    let repo = service.delegation_repository();
    let mut next = request();
    next.operation_id = "next".into();
    assert_eq!(repo.admit(&parent, &next, &limits).err().unwrap().code, ErrorCode::Cancelled);
    assert_eq!(repo.admit(&child_caller, &next, &limits).err().unwrap().code, ErrorCode::Cancelled);
    assert_eq!(repo.admit_followup(&parent, &child.id, &next, &limits).err().unwrap().code, ErrorCode::Cancelled);
    service.create("other").unwrap();
    let other = Caller { session_id: "other".into(), root_session_id: "other".into(), ..parent.clone() };
    assert!(repo.admit(&other, &next, &limits).unwrap().fresh);
    service
        .accept_user_run(
            &crate::StartRunRequest {
                session_id: parent.session_id.clone(),
                request_id: "user_resume".into(),
                parent_id: None,
                message: crate::RunMessage { id: "resume_input".into(), text: "Continue".into() },
                model: "api:test/model".into(),
                options: serde_json::json!({}),
            },
            "time",
        )
        .unwrap();
    assert!(repo.admit(&parent, &next, &limits).unwrap().fresh);
    let stale_child_wake = crate::StartRunRequest {
        session_id: child_caller.session_id.clone(),
        request_id: "late_wake".into(),
        parent_id: None,
        message: crate::RunMessage { id: "wake_input".into(), text: "Continue".into() },
        model: "api:test/model".into(),
        options: serde_json::json!({}),
    };
    assert_eq!(service.delegation_wake_generation(&child_caller.session_id).unwrap(), None);
    assert!(service
        .accept_wake_run(&child_caller, 0, &stale_child_wake, "time")
        .unwrap_err()
        .contains("user-stop generation"));
}

#[test]
fn active_child_queue_is_durable_bounded_and_cancelled_by_stop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let db = store(&path);
    let parent = caller();
    let limits = Limits { queued_per_child: 1, ..Limits::default() };
    let child = db.admit(&parent, &request(), &limits).unwrap().snapshot;
    let mut queued = request();
    queued.operation_id = "queued".into();
    queued.prompt = "Follow up after completion".into();
    let receipt = db.enqueue_delegation_followup(&parent, &child.id, &queued, &limits).unwrap();
    assert!(!receipt.cancelled);
    assert_eq!(db.enqueue_delegation_followup(&parent, &child.id, &queued, &limits).unwrap(), receipt);
    assert_eq!(db.admit(&parent, &queued, &limits).err().unwrap().code, ErrorCode::Conflict);
    assert_eq!(db.admit_followup(&parent, &child.id, &queued, &limits).err().unwrap().code, ErrorCode::Conflict);
    let mut overflow = queued.clone();
    overflow.operation_id = "overflow".into();
    assert_eq!(
        db.enqueue_delegation_followup(&parent, &child.id, &overflow, &limits).err().unwrap().code,
        ErrorCode::Busy
    );
    drop(db);

    let db = SessionStore::open(&path).unwrap();
    assert_eq!(db.enqueue_delegation_followup(&parent, &child.id, &queued, &limits).unwrap(), receipt);
    assert_eq!(RepositoryPort::get(&db, &parent, &child.id).unwrap().activation_id, child.activation_id);
    db.request_stop_tree(&parent).unwrap();
    let after_stop = db.enqueue_delegation_followup(&parent, &child.id, &queued, &limits).unwrap();
    assert_eq!(after_stop.work_item_id, receipt.work_item_id);
    assert!(after_stop.cancelled);
    assert_eq!(
        db.enqueue_delegation_followup(&parent, &child.id, &overflow, &limits).err().unwrap().code,
        ErrorCode::Cancelled
    );
}

#[test]
fn queue_promotes_oldest_only_once_and_persists_its_activation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sqlite3");
    let db = store(&path);
    let parent = caller();
    let limits = Limits { active_per_root: 1, ..Limits::default() };
    let mut initial = request();
    initial.tool_registrations.extend([("read".into(), 1), ("search".into(), 2), ("list".into(), 3)]);
    let child = db.admit(&parent, &initial, &limits).unwrap().snapshot;
    let mut first = initial;
    first.operation_id = format!("{}first", minerva_delegation::IDE_USER_OPERATION_PREFIX);
    let mut second = first.clone();
    second.operation_id = "second".into();
    let original = db.enqueue_delegation_followup(&parent, &child.id, &first, &limits).unwrap();
    let later = db.enqueue_delegation_followup(&parent, &child.id, &second, &limits).unwrap();
    let parent_reads = [("read".into(), 1), ("search".into(), 2)].into();
    assert!(db.promote_delegation_followup(&parent, &child.id, &parent_reads, &limits).unwrap().is_none());
    let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
    db.started(&child.id, &child.activation_id, &handle).unwrap();
    let result = Completion { result_id: "result".into(), state: ActivationState::Succeeded, summary: "Done".into() };
    db.settle(&child.id, &child.activation_id, &result).unwrap();
    db.accept_run(
        &crate::StartRunRequest {
            session_id: parent.session_id.clone(),
            request_id: "tightened".into(),
            parent_id: None,
            message: crate::RunMessage { id: "tightened_input".into(), text: "Continue".into() },
            model: "api:test/model".into(),
            options: serde_json::json!({"toolAllowlist": ["read", "list"]}),
        },
        "time",
    )
    .unwrap();
    db.with_delegation_db(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER reject_promotion BEFORE UPDATE OF activation_id ON delegation_followup_queue
         BEGIN SELECT RAISE(ABORT,'test failure'); END;",
        )
        .map_err(storage)
    })
    .unwrap();
    assert!(db.promote_delegation_followup(&parent, &child.id, &parent_reads, &limits).is_err());
    assert_eq!(RepositoryPort::get(&db, &parent, &child.id).unwrap().activation_id, child.activation_id);
    db.with_delegation_db(|conn| conn.execute_batch("DROP TRIGGER reject_promotion;").map_err(storage)).unwrap();
    let promoted = std::thread::scope(|scope| {
        let promote = || db.promote_delegation_followup(&parent, &child.id, &parent_reads, &limits).unwrap();
        let a = scope.spawn(promote);
        let b = scope.spawn(promote);
        let promoted: Vec<_> = [a.join().unwrap(), b.join().unwrap()].into_iter().flatten().collect();
        assert_eq!(promoted.len(), 1);
        promoted.into_iter().next().unwrap()
    });
    assert_eq!(promoted.work_item_id, original.work_item_id);
    assert_eq!(promoted.previous_handle.as_ref(), Some(&handle));
    assert_eq!(promoted.request.tool_registrations, [("read".into(), 1)].into());
    db.with_delegation_db(|conn| {
        let tx = conn.transaction().map_err(storage)?;
        let mut run = crate::StartRunRequest {
            session_id: handle.session_id.clone(),
            request_id: promoted.activation_id.clone(),
            parent_id: Some(handle.run_id.clone()),
            message: crate::RunMessage { id: "followup-input".into(), text: promoted.request.prompt.clone() },
            model: "api:provider/model".into(),
            options: serde_json::json!({}),
        };
        let Target::Minerva { route, .. } = &promoted.request.target else { panic!("expected local Minerva") };
        run.model.clone_from(route);
        let mut extras = serde_json::json!({});
        run_input::set_followup_origin(&tx, &run, &mut extras).map_err(storage)?;
        assert_eq!(extras["minerva.origin"], IDE_USER_FOLLOWUP_ORIGIN);
        run.message.text.push_str(" changed");
        let mut extras = serde_json::json!({});
        run_input::set_followup_origin(&tx, &run, &mut extras).map_err(storage)?;
        assert!(extras["minerva.origin"].is_null());
        Ok(())
    })
    .unwrap();
    assert_eq!(db.delegation_pending_results(&parent).unwrap(), vec![result]);
    drop(db);

    let service = crate::sessions::SessionService::open(&path).unwrap();
    let db = &service.store;
    assert_eq!(
        db.enqueue_delegation_followup(&parent, &child.id, &first, &limits).unwrap().activation_id,
        Some(promoted.activation_id.clone())
    );
    assert!(service.recoverable_queue_activation(&promoted).unwrap());
    service.revoke_persisted_delegation_reads().unwrap();
    let recovered = RepositoryPort::get(db.as_ref(), &parent, &child.id).unwrap();
    assert!(recovered.request.tool_registrations.is_empty());
    assert!(service.recoverable_queue_activation(&recovered).unwrap());
    let pending_request = db
        .with_delegation_db(|conn| {
            let raw: String = conn
                .query_row(
                    "SELECT request_json FROM delegation_followup_queue WHERE work_item_id=?1",
                    [&later.work_item_id],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            serde_json::from_str::<StartRequest>(&raw).map_err(storage)
        })
        .unwrap();
    assert!(pending_request.tool_registrations.is_empty());
    service.create(&handle.session_id).unwrap();
    service
        .accept_run(
            &crate::StartRunRequest {
                session_id: handle.session_id,
                request_id: promoted.activation_id.clone(),
                parent_id: None,
                message: crate::RunMessage { id: "admitted_input".into(), text: "Continue".into() },
                model: "api:test/model".into(),
                options: serde_json::json!({}),
            },
            "time",
        )
        .unwrap();
    assert!(!service.recoverable_queue_activation(&promoted).unwrap());
    db.request_stop_tree(&parent).unwrap();
    assert!(!db.enqueue_delegation_followup(&parent, &child.id, &first, &limits).unwrap().cancelled);
    let cancelled = db.enqueue_delegation_followup(&parent, &child.id, &second, &limits).unwrap();
    assert_eq!(cancelled.work_item_id, later.work_item_id);
    assert!(cancelled.cancelled && cancelled.activation_id.is_none());
}

#[test]
fn capacity_and_lineage_cannot_be_forged() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("s.sqlite3"));
    let limits = Limits { active_per_root: 1, ..Limits::default() };
    let s = db.admit(&caller(), &request(), &limits).unwrap().snapshot;
    let mut req = request();
    req.operation_id = "other".into();
    assert_eq!(db.admit(&caller(), &req, &limits).err().unwrap().code, ErrorCode::Busy);
    let mut forged = caller();
    forged.root_session_id = "foreign".into();
    assert_eq!(db.admit(&forged, &req, &limits).err().unwrap().code, ErrorCode::Forbidden);
    let child = format!("del_{}", s.id);
    let resolved = db.with_delegation_db(|conn| resolve_caller(conn, &child, "workspace", "C:/project")).unwrap();
    assert_eq!(resolved.0.root_session_id, "parent");
    assert_eq!(resolved.1, 1);
    assert!(db.with_delegation_db(|conn| resolve_caller(conn, &child, "foreign", "C:/project")).is_err());
}

#[test]
fn stop_during_start_remains_stopping_when_handle_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("s.sqlite3"));
    let s = db.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    db.request_stop(&caller(), &s.id).unwrap();
    let handle = ExecutionHandle { session_id: format!("del_{}", s.id), run_id: s.activation_id.clone() };
    db.started(&s.id, &s.activation_id, &handle).unwrap();
    let s = RepositoryPort::get(&db, &caller(), &s.id).unwrap();
    assert_eq!(s.state, ActivationState::Stopping);
    assert_eq!(s.handle, Some(handle));
}

#[test]
fn failed_outbox_write_rolls_back_result_and_terminal_state() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("s.sqlite3"));
    let s = db.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    db.with_delegation_db(|conn|conn.execute_batch("CREATE TRIGGER reject_result BEFORE INSERT ON delegation_outbox BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").map_err(storage)).unwrap();
    let result = Completion { result_id: "r".into(), state: ActivationState::Succeeded, summary: "done".into() };
    assert!(db.settle(&s.id, &s.activation_id, &result).is_err());
    assert_eq!(RepositoryPort::get(&db, &caller(), &s.id).unwrap().state, ActivationState::Starting);
    let count: i64 = db
        .with_delegation_db(|conn| {
            conn.query_row("SELECT count(*) FROM delegation_results", [], |r| r.get(0)).map_err(storage)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn notice_receipt_replays_until_bound_and_fences_old_consumer() {
    let dir = tempfile::tempdir().unwrap();
    let service = crate::sessions::SessionService::open(&dir.path().join("s.sqlite3")).unwrap();
    service.create("parent").unwrap();
    let repo = service.delegation_repository();
    let s = repo.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    repo.settle(
        &s.id,
        &s.activation_id,
        &Completion { result_id: "r".into(), state: ActivationState::Succeeded, summary: "done".into() },
    )
    .unwrap();
    let generation = service.delegation_consumer(&caller(), "first").unwrap();
    let notices = service.delegation_receive_results(&caller(), generation, 10).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(service.delegation_receive_results(&caller(), generation, 10).unwrap(), notices);
    let events = service.events_after("parent", 0, 100).unwrap();
    assert_eq!(events.iter().filter(|e| e.kind == "delegation.result").count(), 1);
    let next = service.delegation_consumer(&caller(), "replacement").unwrap();
    assert!(next > generation);
    assert!(service.delegation_receive_results(&caller(), generation, 10).is_err());
    assert!(service.delegation_bind_notices(&caller(), next, &["r".into()], "missing").is_err());
    service.store.with_delegation_db(|db|db.execute("INSERT INTO runs(session_id,id,parent_id,request_json,status,created_at) VALUES('parent','run','input','{}','running',0)",[]).map(|_|()).map_err(storage)).unwrap();
    service.delegation_bind_notices(&caller(), next, &["r".into()], "run").unwrap();
    assert!(service.delegation_receive_results(&caller(), next, 10).unwrap().is_empty());
}

#[test]
fn next_input_binds_report_only_on_its_origin_branch() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::sessions::SessionService::open(&dir.path().join("s.sqlite3")).unwrap();
    db.create("parent").unwrap();
    for id in ["origin", "alternate"] {
        db.save(
            "parent",
            crate::sessions::SessionMessage {
                id: id.into(),
                parent_id: None,
                role: "user".into(),
                created_at: "2026-11-26T00:00:00Z".into(),
                parts: vec![crate::sessions::ContentPart::text("text", id)],
                extensions: Default::default(),
            },
            None,
        )
        .unwrap();
    }
    let repo = db.delegation_repository();
    let mut start = request();
    start.origin_run = Some("origin".into());
    let child = repo.admit(&caller(), &start, &Limits::default()).unwrap().snapshot;
    repo.settle(
        &child.id,
        &child.activation_id,
        &Completion { result_id: "report".into(), state: ActivationState::Succeeded, summary: "found it".into() },
    )
    .unwrap();
    let generation = db.delegation_consumer(&caller(), "test").unwrap();
    db.delegation_receive_results(&caller(), generation, 8).unwrap();
    let input = |id: &str, parent: &str| crate::StartRunRequest {
        session_id: "parent".into(),
        request_id: format!("run_{id}"),
        parent_id: Some(parent.into()),
        message: crate::RunMessage { id: id.into(), text: "Continue".into() },
        model: "api:fixture/model".into(),
        options: serde_json::json!({}),
    };
    let time = "2026-11-26T00:00:00Z";
    let mut stale = input("stale", "origin");
    stale.options["requireCurrentHead"] = serde_json::json!(true);
    assert!(db.accept_run(&stale, time).unwrap_err().contains("session head changed"));
    assert_eq!(db.load("parent").unwrap().len(), 2);
    db.accept_run(&input("wrong", "alternate"), time).unwrap();
    assert_eq!(db.delegation_receive_results(&caller(), generation, 8).unwrap().len(), 1);
    db.fail_run("parent", "run_wrong", false, "test").unwrap();
    let next = input("right", "origin");
    db.store.with_delegation_db(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_input BEFORE INSERT ON runs WHEN NEW.id='run_right' BEGIN SELECT RAISE(ABORT,'test failure'); END;").map_err(storage)
    }).unwrap();
    assert!(db.accept_run(&next, time).is_err());
    assert_eq!(db.delegation_receive_results(&caller(), generation, 8).unwrap().len(), 1);
    db.store.with_delegation_db(|conn| conn.execute_batch("DROP TRIGGER reject_input").map_err(storage)).unwrap();
    assert!(db.accept_run(&next, time).unwrap());
    assert!(!db.accept_run(&next, time).unwrap());
    let history = db.load("parent").unwrap();
    let saved = history.iter().find(|message| message.id == "right").unwrap();
    assert_eq!(saved.extensions["minerva.delegation"][0]["resultId"], "report");
    assert!(db.delegation_receive_results(&caller(), generation, 8).unwrap().is_empty());
}

#[test]
fn notice_event_failure_does_not_ack_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let service = crate::sessions::SessionService::open(&dir.path().join("s.sqlite3")).unwrap();
    service.create("parent").unwrap();
    let repo = service.delegation_repository();
    let s = repo.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    repo.settle(
        &s.id,
        &s.activation_id,
        &Completion { result_id: "r".into(), state: ActivationState::Succeeded, summary: "done".into() },
    )
    .unwrap();
    let generation = service.delegation_consumer(&caller(), "owner").unwrap();
    service.store.with_delegation_db(|db|db.execute_batch("CREATE TRIGGER fail_notice BEFORE INSERT ON delegation_notices BEGIN SELECT RAISE(ABORT,'failure'); END;").map_err(storage)).unwrap();
    assert!(service.delegation_receive_results(&caller(), generation, 10).is_err());
    assert_eq!(service.delegation_pending().unwrap().len(), 1);
    assert!(!service.events_after("parent", 0, 100).unwrap().iter().any(|e| e.kind == "delegation.result"));
}

fn scoped_followup_fixture(
    options: serde_json::Value,
) -> (tempfile::TempDir, SessionStore, DelegationSnapshot, StartRequest) {
    let dir = tempfile::tempdir().unwrap();
    let db = store(&dir.path().join("scoped.sqlite3"));
    let mut initial = request();
    initial.tool_registrations = [("read_file".into(), 1), ("search_files".into(), 2)].into();
    let child = db.admit(&caller(), &initial, &Limits::default()).unwrap().snapshot;
    let handle = ExecutionHandle { session_id: format!("del_{}", child.id), run_id: child.activation_id.clone() };
    db.started(&child.id, &child.activation_id, &handle).unwrap();
    db.settle(
        &child.id,
        &child.activation_id,
        &Completion { result_id: "initial-result".into(), state: ActivationState::Succeeded, summary: "Done".into() },
    )
    .unwrap();
    db.accept_run(
        &crate::StartRunRequest {
            session_id: caller().session_id,
            request_id: "current-parent".into(),
            parent_id: None,
            message: crate::RunMessage { id: "parent-input".into(), text: "Continue".into() },
            model: "api:test/model".into(),
            options,
        },
        "time",
    )
    .unwrap();
    let mut next = request();
    next.operation_id = "scoped-next".into();
    next.prompt = "Edit the result".into();
    next.origin_run = Some("current-parent".into());
    next.tool_registrations = [
        ("read_file".into(), 1),
        ("search_files".into(), 3),
        ("write_file".into(), 10),
        ("patch_file".into(), 11),
        ("shell".into(), 12),
    ]
    .into();
    let child = RepositoryPort::get(&db, &caller(), &child.id).unwrap();
    (dir, db, child, next)
}

#[test]
fn scoped_followup_expands_only_trusted_writes_and_replays_its_own_namespace() {
    let (dir, db, child, next) = scoped_followup_fixture(serde_json::json!({}));
    let limits = Limits::default();
    let admission = db.admit_scoped_followup(&caller(), &child.id, &next, &limits).unwrap();
    assert!(admission.fresh);
    assert_eq!(admission.snapshot.id, child.id);
    assert_eq!(admission.snapshot.previous_handle.as_ref().unwrap().run_id, child.activation_id);
    assert_eq!(
        admission.snapshot.request.tool_registrations,
        [("read_file".into(), 1), ("write_file".into(), 10), ("patch_file".into(), 11)].into()
    );
    assert!(!db.admit_scoped_followup(&caller(), &child.id, &next, &limits).unwrap().fresh);
    assert_eq!(
        db.lookup_followup(&caller(), &child.id, &next.operation_id, &next.prompt).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(db.admit_followup(&caller(), &child.id, &next, &limits).err().unwrap().code, ErrorCode::Conflict);
    drop(db);
    let db = SessionStore::open(&dir.path().join("scoped.sqlite3")).unwrap();
    let replay = db.admit_scoped_followup(&caller(), &child.id, &next, &limits).unwrap();
    assert!(!replay.fresh);
    assert_eq!(replay.snapshot, admission.snapshot);
    let mut changed = next.clone();
    changed.prompt.push('!');
    assert_eq!(
        db.admit_scoped_followup(&caller(), &child.id, &changed, &limits).err().unwrap().code,
        ErrorCode::Conflict
    );
}

#[test]
fn scoped_followup_requires_the_exact_running_parent_and_preserves_older_queue() {
    let (_dir, db, child, mut next) = scoped_followup_fixture(serde_json::json!({}));
    for origin in [None, Some("stale-parent".into())] {
        next.origin_run = origin;
        assert_eq!(
            db.admit_scoped_followup(&caller(), &child.id, &next, &Limits::default()).err().unwrap().code,
            ErrorCode::Forbidden
        );
        assert_eq!(RepositoryPort::get(&db, &caller(), &child.id).unwrap().activation_id, child.activation_id);
    }
    next.origin_run = Some("current-parent".into());
    let mut queued = next.clone();
    queued.operation_id = "older-queued".into();
    db.enqueue(&caller(), &child.id, &queued, &Limits::default()).unwrap();
    assert_eq!(
        db.lookup_scoped_followup(&caller(), &child.id, &queued.operation_id, &queued.prompt).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        db.admit_scoped_followup(&caller(), &child.id, &next, &Limits::default()).err().unwrap().code,
        ErrorCode::Busy
    );
    assert_eq!(RepositoryPort::get(&db, &caller(), &child.id).unwrap().activation_id, child.activation_id);
}

#[test]
fn scoped_followup_observes_current_parent_tool_limits_and_rejects_finished_parent() {
    let (_dir, db, child, next) = scoped_followup_fixture(serde_json::json!({
        "toolAllowlist":["read_file","write_file"],
        "delegationToolRegistrations":{"read_file":1,"write_file":10,"patch_file":11}
    }));
    let admitted = db.admit_scoped_followup(&caller(), &child.id, &next, &Limits::default()).unwrap();
    assert_eq!(
        admitted.snapshot.request.tool_registrations,
        [("read_file".into(), 1), ("write_file".into(), 10)].into()
    );
    let (_dir, db, child, next) = scoped_followup_fixture(serde_json::json!({}));
    db.fail_run("parent", "current-parent", false, "Stopped").unwrap();
    assert_eq!(
        db.admit_scoped_followup(&caller(), &child.id, &next, &Limits::default()).err().unwrap().code,
        ErrorCode::Forbidden
    );
}

#[test]
fn scoped_followup_rejects_ordinary_receipts_and_rolls_back_failed_admission() {
    let (_dir, db, child, next) = scoped_followup_fixture(serde_json::json!({}));
    let ordinary = db.admit_followup(&caller(), &child.id, &next, &Limits::default()).unwrap();
    assert_eq!(ordinary.snapshot.request.tool_registrations, [("read_file".into(), 1)].into());
    assert_eq!(
        db.lookup_scoped_followup(&caller(), &child.id, &next.operation_id, &next.prompt).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        db.admit_scoped_followup(&caller(), &child.id, &next, &Limits::default()).err().unwrap().code,
        ErrorCode::Conflict
    );

    let (_dir, db, child, next) = scoped_followup_fixture(serde_json::json!({}));
    db.with_delegation_db(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_scoped BEFORE INSERT ON delegation_operations WHEN NEW.operation_id='scoped-next' BEGIN SELECT RAISE(ABORT,'test failure'); END;").map_err(storage)
    }).unwrap();
    assert!(db.admit_scoped_followup(&caller(), &child.id, &next, &Limits::default()).is_err());
    assert_eq!(RepositoryPort::get(&db, &caller(), &child.id).unwrap(), child);
    assert!(db.lookup_scoped_followup(&caller(), &child.id, &next.operation_id, &next.prompt).unwrap().is_none());
}
