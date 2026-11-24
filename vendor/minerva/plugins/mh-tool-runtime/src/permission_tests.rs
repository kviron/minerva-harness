use super::*;
use std::sync::atomic::AtomicUsize;

fn context(policy: PermissionPolicy) -> ToolContext {
    ToolContext {
        session_id: "session".into(),
        run_id: "run".into(),
        call_id: "call".into(),
        cwd: None,
        policy,
        live_policy: None,
        cancellation: Cancellation::default(),
        interactions: Arc::default(),
        sink: None,
    }
}

fn tools(side_effect: bool, executions: Arc<AtomicUsize>) -> AdmittedTools {
    registry(side_effect, executions).snapshot()
}

fn registry(side_effect: bool, executions: Arc<AtomicUsize>) -> ToolRegistry {
    let registry = ToolRegistry::default();
    registry
        .register(
            "test",
            ToolDefinition {
                name: "operation".into(),
                description: String::new(),
                input_schema: json!({}),
                output_schema: json!({}),
                side_effect,
            },
            move |_, _| {
                executions.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(Value::Null) })
            },
        )
        .unwrap();
    registry
}

#[derive(Default)]
struct Events(Mutex<Vec<String>>);
impl ToolEventSink for Events {
    fn emit(
        &self,
        _: &ToolContext,
        _: &str,
        _: &Value,
        status: &str,
        _: Option<&Result<Value, String>>,
    ) -> Result<(), String> {
        self.0.lock().unwrap().push(status.into());
        Ok(())
    }
}

#[tokio::test]
async fn revocation_or_cancellation_during_approval_never_dispatches() {
    for cancel in [false, true] {
        let executions = Arc::new(AtomicUsize::new(0));
        let registry = registry(true, executions.clone());
        let admitted = registry.snapshot();
        let mut context = context(PermissionPolicy::Ask);
        let cancellation = context.cancellation.clone();
        let interactions = context.interactions.clone();
        let events = Arc::new(Events::default());
        context.sink = Some(events.clone());
        let task = tokio::spawn(async move { admitted.execute("operation", json!({}), context).await });
        let pending = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(pending) = interactions.list("session").into_iter().next() {
                    break pending;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if cancel {
            assert!(interactions.answer(InteractionAnswer {
                session_id: "session".into(),
                id: pending.id,
                decision: "allow".into(),
                answer: None,
            }));
            cancellation.cancel();
        } else {
            registry.revoke_owner("test");
            assert!(interactions.answer(InteractionAnswer {
                session_id: "session".into(),
                id: pending.id,
                decision: "allow".into(),
                answer: None,
            }));
        }
        assert!(task.await.unwrap().is_err());
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(interactions.list("session").is_empty());
        let events = events.0.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(&events[..2], ["pending", "requires-approval"]);
        assert_eq!(events[2], if cancel { "cancelled" } else { "failed" });
    }
}

#[tokio::test]
async fn read_only_rejects_writes_without_asking() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tools = tools(true, executions.clone());
    let context = context(PermissionPolicy::ReadOnly);
    let interactions = context.interactions.clone();
    let result = tokio::time::timeout(Duration::from_secs(1), tools.execute("operation", json!({}), context))
        .await
        .expect("read-only rejection must not wait for approval");
    assert_eq!(result.unwrap_err(), "tool writes are forbidden in read-only mode");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(interactions.list("session").is_empty());
}

#[tokio::test]
async fn read_only_allows_reads() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tools = tools(false, executions.clone());
    tools.execute("operation", json!({}), context(PermissionPolicy::ReadOnly)).await.unwrap();
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ask_executes_only_after_approval() {
    for (decision, expected_executions) in [("allow", 1), ("deny", 0)] {
        let executions = Arc::new(AtomicUsize::new(0));
        let tools = tools(true, executions.clone());
        let mut context = context(PermissionPolicy::Allow);
        context.live_policy = Some(Arc::new(MutablePolicy(Mutex::new(PermissionPolicy::Ask))));
        let interactions = context.interactions.clone();
        let task = tokio::spawn(async move { tools.execute("operation", json!({}), context).await });
        let pending = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(pending) = interactions.list("session").into_iter().next() {
                    break pending;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Ask must publish an approval request");
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(interactions.answer(InteractionAnswer {
            session_id: "session".into(),
            id: pending.id,
            decision: decision.into(),
            answer: None,
        }));
        let result = task.await.unwrap();
        assert_eq!(result.is_ok(), decision == "allow");
        assert_eq!(executions.load(Ordering::SeqCst), expected_executions);
    }
}

#[tokio::test]
async fn allow_executes_without_approval() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tools = tools(true, executions.clone());
    tools.execute("operation", json!({}), context(PermissionPolicy::Allow)).await.unwrap();
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

struct MutablePolicy(Mutex<PermissionPolicy>);
impl LivePermissionPolicy for MutablePolicy {
    fn read_policy(&self, _: &str) -> Result<PermissionPolicy, String> {
        Ok(*self.0.lock().unwrap())
    }
}

#[tokio::test]
async fn read_only_change_invalidates_pending_approval() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tools = tools(true, executions.clone());
    let policy = Arc::new(MutablePolicy(Mutex::new(PermissionPolicy::Ask)));
    let mut context = context(PermissionPolicy::Ask);
    context.live_policy = Some(policy.clone());
    let interactions = context.interactions.clone();
    let task = tokio::spawn(async move { tools.execute("operation", json!({}), context).await });
    let pending = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(pending) = interactions.list("session").into_iter().next() {
                break pending;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    *policy.0.lock().unwrap() = PermissionPolicy::ReadOnly;
    assert!(interactions.answer(InteractionAnswer {
        session_id: "session".into(),
        id: pending.id,
        decision: "allow".into(),
        answer: None,
    }));
    assert_eq!(task.await.unwrap().unwrap_err(), "tool writes are forbidden in read-only mode");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[test]
fn live_policy_can_only_narrow_the_admission_snapshot() {
    for (snapshot, live, expected) in [
        (PermissionPolicy::ReadOnly, PermissionPolicy::Allow, PermissionPolicy::ReadOnly),
        (PermissionPolicy::Ask, PermissionPolicy::Allow, PermissionPolicy::Ask),
        (PermissionPolicy::Allow, PermissionPolicy::Ask, PermissionPolicy::Ask),
        (PermissionPolicy::Allow, PermissionPolicy::ReadOnly, PermissionPolicy::ReadOnly),
    ] {
        let mut context = context(snapshot);
        context.live_policy = Some(Arc::new(MutablePolicy(Mutex::new(live))));
        assert_eq!(context.effective_policy().unwrap(), expected);
    }
}

struct FailedPolicy;
impl LivePermissionPolicy for FailedPolicy {
    fn read_policy(&self, _: &str) -> Result<PermissionPolicy, String> {
        Err("policy unavailable".into())
    }
}

#[tokio::test]
async fn unavailable_live_policy_does_not_execute_writes() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tools = tools(true, executions.clone());
    let mut context = context(PermissionPolicy::Allow);
    context.live_policy = Some(Arc::new(FailedPolicy));
    assert_eq!(tools.execute("operation", json!({}), context).await.unwrap_err(), "policy unavailable");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
