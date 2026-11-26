use crate::{
    limits::Limits,
    repository::{Admission, RepositoryPort},
    *,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
struct Store(Mutex<Option<DelegationSnapshot>>);
impl RepositoryPort for Store {
    fn request_stop_tree(&self, _: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
        Err(DelegationError::new(ErrorCode::Unsupported, "Fixture has no tree"))
    }
    fn lookup_operation(
        &self,
        caller: &Caller,
        request: &StartRequest,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        let slot = self.0.lock().unwrap();
        match slot.as_ref() {
            Some(s) if s.caller == *caller && s.request.operation_id == request.operation_id => {
                if s.request != *request {
                    return Err(DelegationError::new(ErrorCode::Conflict, "payload mismatch"));
                }
                Ok(Some(s.clone()))
            }
            _ => Ok(None),
        }
    }
    fn admit(&self, caller: &Caller, request: &StartRequest, _: &Limits) -> DelegationResult<Admission> {
        let mut slot = self.0.lock().unwrap();
        if let Some(snapshot) = slot.as_ref() {
            if snapshot.caller != *caller || snapshot.request != *request {
                return Err(DelegationError::new(ErrorCode::Conflict, "conflict"));
            }
            return Ok(Admission { snapshot: snapshot.clone(), fresh: false });
        }
        let snapshot = DelegationSnapshot {
            fallback: None,
            id: "child".into(),
            work_item_id: "work".into(),
            activation_id: "activation".into(),
            caller: caller.clone(),
            request: request.clone(),
            depth: 1,
            state: ActivationState::Starting,
            previous_handle: None,
            handle: None,
            completion: None,
        };
        *slot = Some(snapshot.clone());
        Ok(Admission { snapshot, fresh: true })
    }
    fn get(&self, caller: &Caller, _: &str) -> DelegationResult<DelegationSnapshot> {
        let slot = self.0.lock().unwrap();
        let snapshot = slot.as_ref().unwrap();
        if snapshot.caller != *caller {
            return Err(DelegationError::new(ErrorCode::Forbidden, "foreign"));
        }
        Ok(snapshot.clone())
    }
    fn admit_followup(&self, _: &Caller, _: &str, _: &StartRequest, _: &Limits) -> DelegationResult<Admission> {
        Err(DelegationError::new(ErrorCode::Unsupported, "Fixture does not reuse children"))
    }
    fn lookup_followup(&self, _: &Caller, _: &str, _: &str, _: &str) -> DelegationResult<Option<DelegationSnapshot>> {
        Ok(None)
    }
    fn list(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
        Ok(vec![self.get(caller, "child")?])
    }
    fn started(&self, _: &str, _: &str, handle: &ExecutionHandle) -> DelegationResult<()> {
        let mut slot = self.0.lock().unwrap();
        let s = slot.as_mut().unwrap();
        s.handle = Some(handle.clone());
        s.state = ActivationState::Running;
        Ok(())
    }
    fn settle(&self, _: &str, _: &str, completion: &Completion) -> DelegationResult<()> {
        let mut slot = self.0.lock().unwrap();
        let s = slot.as_mut().unwrap();
        s.state = completion.state;
        s.completion = Some(completion.clone());
        Ok(())
    }
    fn mark_unknown(&self, _: &str, _: &str, _: &str) -> DelegationResult<()> {
        self.0.lock().unwrap().as_mut().unwrap().state = ActivationState::Unknown;
        Ok(())
    }
    fn request_stop(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot> {
        let snapshot = self.get(caller, id)?;
        if !snapshot.state.terminal() {
            self.0.lock().unwrap().as_mut().unwrap().state = ActivationState::Stopping;
        }
        self.get(caller, id)
    }
}
struct Executor {
    starts: AtomicUsize,
    unknown: bool,
    verified: bool,
}

struct DelayedCatalog {
    executor: Executor,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl ExecutionPort for DelayedCatalog {
    fn capabilities(&self, target: &Target) -> ExecutionCapabilities {
        self.executor.capabilities(target)
    }
    fn discover<'a>(&'a self, caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.resume.notified().await;
            self.executor.discover(caller).await
        })
    }
    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        self.executor.start(snapshot)
    }
    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        self.executor.observe(snapshot)
    }
    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        self.executor.interrupt(snapshot)
    }
}
impl ExecutionPort for Executor {
    fn capabilities(&self, _: &Target) -> ExecutionCapabilities {
        let capability = if self.verified {
            Capability::Supported { scope: "readonly sandbox".into() }
        } else {
            Capability::Unknown { reason: "not verified".into() }
        };
        ExecutionCapabilities {
            read_only: capability.clone(),
            interrupt: capability.clone(),
            continuation: capability.clone(),
            nested_delegation_control: capability,
        }
    }
    fn discover<'a>(&'a self, _: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async { Ok(vec![Candidate { target: target(), read_only: true, unavailable_reason: None }]) })
    }
    fn start<'a>(&'a self, _: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.unknown {
                Err(DelegationError::new(ErrorCode::OutcomeUnknown, "receipt lost"))
            } else {
                Ok(ExecutionHandle { session_id: "session".into(), run_id: "run".into() })
            }
        })
    }
    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        Box::pin(async {
            if snapshot.handle.is_none() {
                return Err(DelegationError::new(ErrorCode::OutcomeUnknown, "no executor record yet"));
            }
            Ok(None)
        })
    }
    fn interrupt<'a>(&'a self, _: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}
fn target() -> Target {
    Target::Minerva { route: "api:test/model".into(), effort: None }
}
fn caller() -> Caller {
    Caller {
        session_id: "parent".into(),
        root_session_id: "parent".into(),
        workspace_id: "workspace".into(),
        cwd: "C:/workspace".into(),
    }
}
fn request() -> StartRequest {
    StartRequest {
        origin_run: None,
        tool_registrations: Default::default(),
        operation_id: "operation".into(),
        target: target(),
        prompt: "inspect files".into(),
    }
}

#[tokio::test]
async fn observation_cannot_mark_a_start_unknown_before_dispatch_records_a_handle() {
    let store = Arc::new(Store::default());
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: false, verified: true });
    let service = DelegationService::new(store.clone(), executor, Limits::default());
    let accepted = store.admit(&caller(), &request(), &Limits::default()).unwrap().snapshot;
    let observed = service.poll(&caller(), &accepted.id).await.unwrap();
    assert_eq!(observed.state, ActivationState::Starting);
    assert!(observed.handle.is_none());
}

#[tokio::test]
async fn observation_preserves_a_stop_requested_before_dispatch() {
    let store = Arc::new(Store::default());
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: false, verified: true });
    let service = DelegationService::new(store.clone(), executor.clone(), Limits::default());
    let accepted = service.start(&caller(), &request()).await.unwrap();
    store.request_stop(&caller(), &accepted.id).unwrap();
    assert_eq!(service.poll(&caller(), &accepted.id).await.unwrap().state, ActivationState::Stopping);
    service.drain_starts().await;
    assert_eq!(service.poll(&caller(), &accepted.id).await.unwrap().state, ActivationState::Cancelled);
    assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn accepted_start_outlives_its_caller_handle() {
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: false, verified: true });
    let store = Arc::new(Store::default());
    let service = DelegationService::new(store.clone(), executor.clone(), Limits::default());
    let accepted = service.start(&caller(), &request()).await.unwrap();
    assert_eq!(accepted.state, ActivationState::Starting);
    drop(service);
    tokio::task::yield_now().await;
    assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
    assert_eq!(store.get(&caller(), "child").unwrap().state, ActivationState::Running);
}

#[tokio::test]
async fn duplicate_operation_does_not_start_twice() {
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: false, verified: true });
    let service = DelegationService::new(Arc::new(Store::default()), executor.clone(), Limits::default());
    let first = service.start(&caller(), &request()).await.unwrap();
    service.drain_starts().await;
    let second = service.start(&caller(), &request()).await.unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
    let mut changed = request();
    changed.prompt = "different".into();
    assert_eq!(service.start(&caller(), &changed).await.unwrap_err().code, ErrorCode::Conflict);
}

#[tokio::test]
async fn uncertain_start_is_retained_and_never_retried() {
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: true, verified: true });
    let service = DelegationService::new(Arc::new(Store::default()), executor.clone(), Limits::default());
    service.start(&caller(), &request()).await.unwrap();
    service.drain_starts().await;
    assert_eq!(service.get(&caller(), "child").unwrap().state, ActivationState::Unknown);
    service.start(&caller(), &request()).await.unwrap();
    assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unverified_readonly_is_rejected_before_admission() {
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: false, verified: false });
    let store = Arc::new(Store::default());
    let service = DelegationService::new(store.clone(), executor.clone(), Limits::default());
    assert_eq!(service.start(&caller(), &request()).await.unwrap_err().code, ErrorCode::Unsupported);
    assert!(store.0.lock().unwrap().is_none());
    assert_eq!(executor.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn closing_admission_keeps_prior_receipt_but_rejects_new_work() {
    let executor = Arc::new(Executor { starts: AtomicUsize::new(0), unknown: false, verified: true });
    let service = DelegationService::new(Arc::new(Store::default()), executor.clone(), Limits::default());
    let first = service.start(&caller(), &request()).await.unwrap();
    service.close_admission();
    let replay = service.start(&caller(), &request()).await.unwrap();
    assert_eq!(replay.id, first.id);
    let mut new = request();
    new.operation_id = "another".into();
    assert_eq!(service.start(&caller(), &new).await.unwrap_err().code, ErrorCode::Unavailable);
    service.drain_starts().await;
    assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn close_during_preflight_rejects_before_commit() {
    let executor = Arc::new(DelayedCatalog {
        executor: Executor { starts: AtomicUsize::new(0), unknown: false, verified: true },
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let store = Arc::new(Store::default());
    let service = Arc::new(DelegationService::new(store.clone(), executor.clone(), Limits::default()));
    let worker = service.clone();
    let start = tokio::spawn(async move { worker.start(&caller(), &request()).await });
    executor.entered.notified().await;
    service.close_admission();
    executor.resume.notify_one();
    assert_eq!(start.await.unwrap().unwrap_err().code, ErrorCode::Unavailable);
    assert!(store.0.lock().unwrap().is_none());
    assert_eq!(executor.executor.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn caller_cancel_during_discovery_cannot_admit_work() {
    let executor = Arc::new(DelayedCatalog {
        executor: Executor { starts: AtomicUsize::new(0), unknown: false, verified: true },
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let store = Arc::new(Store::default());
    let service = Arc::new(DelegationService::new(store.clone(), executor.clone(), Limits::default()));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cancelled.clone();
    let start =
        tokio::spawn(async move { service.start_if(&caller(), &request(), || !flag.load(Ordering::SeqCst)).await });
    executor.entered.notified().await;
    cancelled.store(true, Ordering::SeqCst);
    executor.resume.notify_one();
    assert_eq!(start.await.unwrap().unwrap_err().code, ErrorCode::Cancelled);
    assert!(store.0.lock().unwrap().is_none());
}
