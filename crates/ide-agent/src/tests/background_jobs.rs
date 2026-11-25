use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn finished_background_output_survives_idle_recycling_without_holding_the_peer() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = Arc::new(FakeAgent::default());
    let mgr = manager(tmp.path(), agent.clone());
    mgr.set_idle_release(Duration::from_millis(200));
    let task = mgr
        .create(NewAgentTask {
            prompt: text("background"),
            provider: "claude".into(),
            isolation: Isolation::Local,
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for(&mgr, &task.id, AgentStatus::Ready).await;
    agent.update(
        "s1",
        json!({"sessionUpdate":BACKGROUND_TASKS_UPDATE,"tasks":[{
            "task_id":"bsh1","task_type":"local_bash","description":"sleep 900",
            "status":"completed","output":"Finished successfully"
        }]}),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let completed = {
                let native = mgr.task(&task.id).unwrap();
                let state = native.state.lock();
                let completed =
                    state.task.background.first().is_some_and(|row| row.status.as_deref() == Some("completed"));
                if completed {
                    assert!(state.background_tasks.is_empty());
                }
                completed
            };
            if completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while mgr.task(&task.id).unwrap().live.lock().as_ref().is_some_and(|live| !live.peer.is_closed()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let native = mgr.task(&task.id).unwrap();
    let state = native.state.lock();
    assert_eq!(state.task.background.len(), 1);
    assert_eq!(state.task.background[0].output.as_deref(), Some("Finished successfully"));
    drop(state);
    mgr.shutdown();
}
