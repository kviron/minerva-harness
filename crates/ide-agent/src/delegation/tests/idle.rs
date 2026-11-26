use super::*;
use ide_jsonrpc::{Framing, Handler, Peer};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture(AtomicUsize);
impl Handler for Fixture {
    fn on_notification(&self, _: &str, _: Value) {}
    fn on_request(&self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        Ok(match method {
            "initialize" => json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true}}),
            "session/new" => json!({"sessionId":format!("session-{}",self.0.fetch_add(1,Ordering::SeqCst))}),
            "session/load" => json!({"sessionId":params["sessionId"]}),
            _ => json!({}),
        })
    }
}

async fn idle_task(manager: &crate::AgentManager, settled: bool) -> (Arc<crate::Task>, String) {
    let created = manager
        .create(crate::NewAgentTask {
            provider: "codex".into(),
            prompt: crate::AgentPrompt { text: "Inspect".into(), ..Default::default() },
            idle: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let task = manager.task(&created.id).unwrap();
    task.state.lock().delegated_read_only = true;
    let session = task.ensure_session().await.unwrap().session;
    let operation = format!("op-{}", task.id);
    let store = super::super::attempt::open_for_turn(&manager.shared.paths.store_dir).await.unwrap();
    store
        .reserve(Admission {
            requested_config: Default::default(),
            parent_session_id: "parent".into(),
            root_session_id: "parent".into(),
            delegation_id: task.id.clone(),
            operation_id: operation.clone(),
            payload_hash: operation.clone(),
            task_id: task.id.clone(),
            activation_id: format!("act-{}", task.id),
            request_id: format!("run-{}", task.id),
            codex_scoped_tools: false,
            host_turn_id: None,
            native_session_id: None,
            phase: AdmissionPhase::Reserved,
            completion: None,
        })
        .unwrap();
    store.claim_dispatch(&operation).unwrap();
    store.bind_native_session(&operation, &session).unwrap();
    if settled {
        store
            .complete_turn(
                &operation,
                "native-turn",
                Completion {
                    result_id: format!("result-{}", task.id),
                    state: ActivationState::Succeeded,
                    summary: "Done".into(),
                },
            )
            .unwrap();
    }
    drop(store);
    manager.shared.idle_delegated.retain(&task).await;
    (task, operation)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_native_cap_evicts_oldest_confirmed_connections_and_resumes_the_original_session() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Arc::new(Fixture(AtomicUsize::new(0)));
    let servers = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let retained = servers.clone();
    let manager = manager_with(
        dir.path(),
        Arc::new(move |_, _, handler, _| {
            let (a, b) = tokio::io::duplex(1 << 16);
            let (ar, aw) = tokio::io::split(a);
            let (br, bw) = tokio::io::split(b);
            retained.lock().push(Peer::connect_framed(br, bw, fixture.clone(), Framing::Lines));
            Ok(Peer::connect_framed(ar, aw, handler, Framing::Lines))
        }),
    );
    let (unknown, _) = idle_task(&manager, false).await;
    let (busy, _) = idle_task(&manager, true).await;
    busy.state.lock().busy = true;
    let (background, _) = idle_task(&manager, true).await;
    background.state.lock().background_tasks = vec![json!({"task_id":"background","status":"running"})];
    let (permission, _) = idle_task(&manager, true).await;
    let (send, _receive) = std::sync::mpsc::channel();
    permission.permissions.lock().insert(1, send);
    let mut tasks = Vec::new();
    for _ in 0..9 {
        tasks.push(idle_task(&manager, true).await.0);
    }
    let first = &tasks[0];
    assert!(first.live.lock().is_none(), "oldest confirmed idle connection is evicted");
    assert_eq!(tasks.iter().filter(|task| task.live.lock().is_some()).count(), 8);
    for protected in [&unknown, &busy, &background, &permission] {
        assert!(protected.live.lock().as_ref().is_some_and(|live| !live.peer.is_closed()));
    }
    background.on_update(None, &json!({"sessionUpdate":crate::BACKGROUND_TASKS_UPDATE,"tasks":[]}));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while tasks.iter().chain(std::iter::once(&background)).filter(|task| task.live.lock().is_some()).count() > 8 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("last background completion must enforce the idle cap immediately");
    let original = first.state.lock().session_id.clone().unwrap();
    let resumed = first.ensure_session().await.unwrap();
    assert_eq!(resumed.session, original);
    assert!(!first.start_fresh.load(Ordering::Acquire));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while tasks.iter().chain(std::iter::once(&background)).filter(|task| task.live.lock().is_some()).count() > 8 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reconnecting an evicted session must re-enroll it in the cap");
    assert!(first.live.lock().is_some(), "the recently resumed session is the newest idle owner");
    manager.shutdown();
}
