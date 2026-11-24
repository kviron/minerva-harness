use super::*;
use crate::sessions::SessionService;

fn request(model: &str) -> crate::StartRunRequest {
    crate::StartRunRequest {
        session_id: "session".into(),
        request_id: "run".into(),
        parent_id: None,
        message: crate::RunMessage { id: "question".into(), text: "inspect".into() },
        model: model.into(),
        options: json!({"liveSelection":true}),
    }
}

fn dispatch(effect: &str, selection: RouteSelection) -> RouteDispatch {
    RouteDispatch { run_id: "run".into(), effect_id: effect.into(), turn: 1, selection }
}

#[test]
fn frozen_admission_never_consumes_a_newer_selection_and_effects_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let service = SessionService::open(&dir.path().join("sessions.sqlite3")).unwrap();
    service.create("session").unwrap();
    let a = service.request_selection("session", "api:p/a", None).unwrap();
    assert_eq!(service.request_selection("session", "api:p/a", Some("default")).unwrap(), a);
    let b = service.request_selection("session", "api:p/b", Some("high")).unwrap();
    let c = service.request_selection("session", "api:p/c", None).unwrap();
    service.accept_run(&request("api:p/a"), "now").unwrap();
    assert_eq!(service.get("session").unwrap().model.as_deref(), Some("api:p/c"));
    assert_eq!(service.ensure_selection("session", "api:p/a", Some("low")).unwrap(), c);
    let frozen = dispatch("effect-b", b.clone());
    assert_eq!(
        service.record_selection_dispatch("session", &frozen).unwrap(),
        DispatchAdmission::Recorded { changed: true }
    );
    assert_eq!(service.selection("session").unwrap(), Some(c.clone()));
    assert_eq!(service.record_selection_dispatch("session", &frozen).unwrap(), DispatchAdmission::Duplicate);
    assert!(service.record_selection_dispatch("session", &dispatch("effect-b", c.clone())).is_err());
    assert_eq!(
        service.record_selection_dispatch("session", &dispatch("effect-c", c)).unwrap(),
        DispatchAdmission::Recorded { changed: true }
    );
    let a_again = service.request_selection("session", "api:p/a", None).unwrap();
    assert!(a_again.revision > a.revision);
    assert_eq!(
        service.record_selection_dispatch("session", &dispatch("effect-a", a_again)).unwrap(),
        DispatchAdmission::Recorded { changed: true }
    );
    let events = service.events_after("session", 0, 100).unwrap();
    assert_eq!(events.iter().filter(|event| event.kind == "run.selection_dispatch_intent").count(), 3);
    service.fail_run("session", "run", false, "finished").unwrap();
    assert_eq!(service.record_selection_dispatch("session", &frozen).unwrap(), DispatchAdmission::Duplicate);
    assert!(service.record_selection_dispatch("session", &dispatch("late", b)).is_err());
}

#[test]
fn effort_cas_and_transaction_failure_cannot_restore_an_old_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let service = SessionService::open(&path).unwrap();
    service.create("session").unwrap();
    let a = service.request_selection("session", "api:p/a", None).unwrap();
    let b = service.request_selection("session", "api:p/b", None).unwrap();
    assert!(service.set_model("session", "api:p/b\n").is_err());
    assert!(service.request_selection("session", "api:p/b\n", None).is_err());
    assert!(service.request_selection_if_revision("session", a.revision, "api:p/a", Some("high")).is_err());
    assert_eq!(service.selection("session").unwrap(), Some(b.clone()));
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_selection BEFORE INSERT ON session_events WHEN NEW.kind='session.selection_requested' BEGIN SELECT RAISE(ABORT,'disk failure fixture'); END;").unwrap();
    assert!(service.request_selection("session", "api:p/c", Some("high")).is_err());
    assert_eq!(service.selection("session").unwrap(), Some(b));
    assert_eq!(service.get("session").unwrap().model.as_deref(), Some("api:p/b"));
}

#[test]
fn admissions_validate_session_revision_and_never_rewrite_run_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let service = SessionService::open(&path).unwrap();
    service.create("session").unwrap();
    service.create("other").unwrap();
    let selected = service.request_selection("session", "api:p/a", None).unwrap();
    service.request_selection("other", "api:p/b", None).unwrap();
    let input = request("api:p/a");
    service.accept_run(&input, "now").unwrap();
    let good = dispatch("effect", selected.clone());
    assert_eq!(
        service.record_selection_dispatch("session", &good).unwrap(),
        DispatchAdmission::Recorded { changed: false }
    );
    assert!(service.record_selection_dispatch("other", &good).is_err());
    let mut forged = dispatch("forged", selected);
    forged.selection.model = "api:p/b".into();
    assert!(service.record_selection_dispatch("session", &forged).is_err());
    assert!(service.request_selection("session", "api:other/b", None).is_err());
    service.request_selection("session", "api:p/c", Some("max")).unwrap();
    assert!(!service.accept_run(&input, "later").unwrap());
    let db = Connection::open(&path).unwrap();
    let stored: String = db.query_row("SELECT request_json FROM runs WHERE id='run'", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), json!(input));
}

#[test]
fn ordinary_headless_runs_do_not_enroll_in_live_selection() {
    let dir = tempfile::tempdir().unwrap();
    let service = SessionService::open(&dir.path().join("sessions.sqlite3")).unwrap();
    service.create("session").unwrap();
    let selection = service.request_selection("session", "api:p/a", None).unwrap();
    let mut input = request("api:p/a");
    input.options = json!({});
    service.accept_run(&input, "now").unwrap();
    assert!(service.record_selection_dispatch("session", &dispatch("headless", selection)).is_err());
    assert!(!service
        .events_after("session", 0, 100)
        .unwrap()
        .iter()
        .any(|event| event.kind == "run.selection_dispatch_intent"));
}

#[test]
fn restart_keeps_pending_choice_but_never_resumes_an_old_effect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let pending;
    {
        let service = SessionService::open(&path).unwrap();
        service.create("session").unwrap();
        service.request_selection("session", "api:p/a", None).unwrap();
        service.accept_run(&request("api:p/a"), "now").unwrap();
        pending = service.request_selection("session", "api:p/b", Some("high")).unwrap();
    }
    let service = SessionService::open(&path).unwrap();
    assert_eq!(service.selection("session").unwrap(), Some(pending.clone()));
    assert_eq!(service.run_snapshot("session", "run").unwrap().status, "interrupted");
    assert!(service.record_selection_dispatch("session", &dispatch("after-restart", pending)).is_err());
}

#[test]
fn v6_migration_preserves_legacy_model_without_inventing_an_effort() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    {
        let service = SessionService::open(&path).unwrap();
        service.create("session").unwrap();
        service.set_model("session", "api:p/a").unwrap();
    }
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP TABLE run_selection_dispatches; DROP TABLE session_selections; PRAGMA user_version=6;")
            .unwrap();
    }
    let service = SessionService::open(&path).unwrap();
    assert_eq!(service.get("session").unwrap().model.as_deref(), Some("api:p/a"));
    assert!(service.selection("session").unwrap().is_none());
    let selected = service.ensure_selection("session", "api:p/a", Some("high")).unwrap();
    assert_eq!(selected.reasoning_effort.as_deref(), Some("high"));
    service.set_model("session", "api:p/a").unwrap();
    assert_eq!(service.selection("session").unwrap(), Some(selected));
    service.set_model("session", "api:p/b").unwrap();
    assert!(service.selection("session").unwrap().unwrap().reasoning_effort.is_none());
    assert!(dir.path().read_dir().unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("pre-v7")));
}
