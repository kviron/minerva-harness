use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn saved_minerva_tasks_resolve_removed_connections_without_replacing_failed_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = manager(tmp.path(), Arc::new(FakeAgent::default()));
    mgr.set_minerva_connections(vec![ide_minerva::Connection {
        id: "minerva.api.old".into(),
        name: "Old API".into(),
        model_prefix: "api:old/".into(),
    }]);
    mgr.providers();
    let task = mgr
        .create(NewAgentTask {
            prompt: text("again"),
            provider: "minerva.api.old".into(),
            isolation: Isolation::Local,
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for(&mgr, &task.id, AgentStatus::Ready).await;
    let saved = mgr.task(&task.id).unwrap().state.lock().session_id.clone().unwrap();
    mgr.shutdown();

    let agent = Arc::new(FakeAgent {
        load_error: Some((codes::INTERNAL_ERROR, "Saved Minerva session unavailable".into())),
        ..Default::default()
    });
    let restored = manager(tmp.path(), agent.clone());
    restored.providers();
    assert!(restored.shared.spec("minerva.api.old").is_none());
    let fallback = restored.shared.saved_task_spec("minerva.api.old").unwrap();
    assert_eq!(fallback.driver, provider::Driver::Minerva);
    assert_eq!(fallback.id, "minerva.api.old");
    assert_eq!(fallback.args, ["api:old/"]);
    for unknown in ["unknown", "minerva.api.", "minerva.plugin.old"] {
        assert!(restored.shared.saved_task_spec(unknown).is_none());
    }
    for _ in 0..2 {
        assert!(restored.task(&task.id).unwrap().ensure_session().await.is_err());
        assert_eq!(restored.task(&task.id).unwrap().state.lock().session_id.as_deref(), Some(saved.as_str()));
    }
    assert_eq!(
        agent.calls.lock().iter().filter(|call| call.starts_with("session/")).cloned().collect::<Vec<_>>(),
        ["session/load", "session/load"],
    );
    assert!(
        restored
            .create(NewAgentTask {
                prompt: text("new task"),
                provider: "minerva.api.old".into(),
                isolation: Isolation::Local,
                ..Default::default()
            })
            .await
            .is_err()
    );
    restored.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn minerva_provider_switch_keeps_session_and_rejects_old_connection_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = Arc::new(FakeAgent::default());
    let mgr = manager(tmp.path(), agent.clone());
    mgr.set_minerva_connections(
        ["one", "two"]
            .into_iter()
            .map(|id| ide_minerva::Connection {
                id: format!("minerva.api.{id}"),
                name: id.into(),
                model_prefix: format!("api:{id}/"),
            })
            .collect(),
    );
    mgr.providers();
    let task = mgr
        .create(NewAgentTask {
            prompt: text("again"),
            provider: "minerva.api.one".into(),
            isolation: Isolation::Local,
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for(&mgr, &task.id, AgentStatus::Ready).await;
    let handle = mgr.task(&task.id).unwrap();
    let saved = handle.state.lock().session_id.clone();
    let model = handle.state.lock().task.model.clone();
    let entries = mgr.entries(&task.id, None).unwrap().len();
    let old = Client::new(&handle, None);
    mgr.set_config(&task.id, "provider", "minerva.api.two".into()).await.unwrap();
    assert_eq!(handle.state.lock().session_id, saved);
    assert_eq!(handle.state.lock().task.provider, "minerva.api.two");
    assert_eq!(handle.state.lock().task.model, model);
    assert_eq!(mgr.entries(&task.id, None).unwrap().len(), entries);
    assert!(mgr.prompt(&task.id, text("wrong route"), false).await.is_err());
    assert_eq!(mgr.entries(&task.id, None).unwrap().len(), entries);
    handle.state.lock().task.queue.push(AgentQueued { id: "queued".into(), prompt: text("keep queued") });
    assert!(mgr.send_queued_now(&task.id, "queued").await.is_err());
    assert_eq!(handle.state.lock().task.queue.len(), 1);
    let config = handle.state.lock().task.config.clone();
    old.on_notification(
        "session/update",
        json!({"sessionId":saved,"update":{
            "sessionUpdate":"config_option_update", "configOptions":[{
                "id":"model","name":"Model","category":"model","type":"select",
                "currentValue":"stale","options":[{"value":"stale","name":"Stale"}]
            }]
        }}),
    );
    assert_eq!(handle.state.lock().task.config, config);
    assert!(old.on_request("session/request_permission", json!({})).is_err());
    assert!(mgr.set_config(&task.id, "provider", "claude".into()).await.is_err());

    handle.ensure_session().await.unwrap();
    let settings = handle.setting_changes.lock().await;
    {
        let mut state = handle.state.lock();
        state.task.config.push(
            serde_json::from_value(json!({
                "id":"late","name":"Late","current":"off","via":"option",
                "values":[{"value":"on","name":"On"}]
            }))
            .unwrap(),
        );
        state.wanted.push(AgentConfigChoice { id: "late".into(), value: "on".into() });
    }
    let calls_before = agent.calls.lock().len();
    handle.apply_late_choices();
    tokio::task::yield_now().await;
    mgr.switch_minerva_provider(&task.id, "minerva.api.one".into()).await.unwrap();
    drop(settings);
    let drained = handle.setting_changes.lock().await;
    drop(drained);
    assert_eq!(agent.calls.lock().len(), calls_before);
    mgr.set_config(&task.id, "provider", "minerva.api.two".into()).await.unwrap();

    let config = handle.state.lock().task.config.clone();
    let file = tmp.path().join("store").join(format!("{}.json", task.id));
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(mgr.set_config(&task.id, "provider", "minerva.api.one".into()).await.is_err());
    assert_eq!(handle.state.lock().task.provider, "minerva.api.two");
    assert_eq!(handle.state.lock().task.config, config);
    assert_eq!(handle.state.lock().session_id, saved);
    std::fs::remove_dir(&file).unwrap();
    handle.persist();
    mgr.shutdown();
    let restored = manager(tmp.path(), Arc::new(FakeAgent::default()));
    assert_eq!(restored.task(&task.id).unwrap().state.lock().task.provider, "minerva.api.two");
    assert_eq!(restored.task(&task.id).unwrap().state.lock().session_id, saved);
    restored.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn minerva_late_choice_does_not_overwrite_a_newer_choice_on_the_same_connection() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = Arc::new(FakeAgent::default());
    let mgr = manager(tmp.path(), agent.clone());
    mgr.set_minerva_connections(vec![ide_minerva::Connection {
        id: "minerva.api.one".into(),
        name: "One".into(),
        model_prefix: "api:one/".into(),
    }]);
    mgr.providers();
    let task = mgr
        .create(NewAgentTask {
            prompt: text("again"),
            provider: "minerva.api.one".into(),
            isolation: Isolation::Local,
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for(&mgr, &task.id, AgentStatus::Ready).await;
    let handle = mgr.task(&task.id).unwrap();
    let settings = handle.setting_changes.lock().await;
    {
        let mut state = handle.state.lock();
        state.task.config.push(
            serde_json::from_value(json!({
                "id":"late","name":"Late","current":"off","via":"option",
                "values":[{"value":"on","name":"On"},{"value":"off","name":"Off"}]
            }))
            .unwrap(),
        );
        state.wanted.push(AgentConfigChoice { id: "late".into(), value: "on".into() });
    }
    let calls_before = agent.calls.lock().len();
    handle.apply_late_choices();
    tokio::task::yield_now().await;
    handle.state.lock().wanted.iter_mut().find(|choice| choice.id == "late").unwrap().value = "off".into();
    drop(settings);
    let drained = handle.setting_changes.lock().await;
    drop(drained);
    assert_eq!(agent.calls.lock().len(), calls_before);
    assert_eq!(
        handle.state.lock().task.config.iter().find(|option| option.id == "late").unwrap().current.as_deref(),
        Some("off")
    );
    mgr.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn minerva_model_changes_clear_previous_effort_and_restore_it_on_failure() {
    for reject in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let agent = Arc::new(FakeAgent::default());
        for model in ["big", "small"] {
            agent.model_options.lock().insert(model.into(), effort_options(&["low", "high"], "low"));
        }
        let mgr = manager(tmp.path(), agent.clone());
        mgr.set_minerva_connections(vec![ide_minerva::Connection {
            id: "minerva.api.one".into(),
            name: "One".into(),
            model_prefix: "api:one/".into(),
        }]);
        mgr.providers();
        let task = mgr
            .create(NewAgentTask {
                prompt: text("again"),
                provider: "minerva.api.one".into(),
                isolation: Isolation::Local,
                ..Default::default()
            })
            .await
            .unwrap();
        wait_for(&mgr, &task.id, AgentStatus::Ready).await;
        mgr.set_config(&task.id, "effort", "high".into()).await.unwrap();
        let handle = mgr.task(&task.id).unwrap();
        handle.config_sent.lock().insert(("effort".into(), "high".into()));
        // A recovered legacy choice may no longer have a matching advertised control.
        handle.state.lock().wanted.push(AgentConfigChoice { id: "reasoning_effort".into(), value: "high".into() });
        handle.config_sent.lock().insert(("reasoning_effort".into(), "high".into()));
        let wanted_before = handle.state.lock().wanted.clone();
        if reject {
            *agent.reject_model.lock() = Some("small".into());
        }
        let result = mgr.set_config(&task.id, "model", "small".into()).await;
        if reject {
            assert!(result.is_err());
            let state = handle.state.lock();
            assert_eq!(state.task.model.as_deref(), Some("big"));
            assert_eq!(state.wanted, wanted_before);
            assert!(handle.config_sent.lock().contains(&("effort".into(), "high".into())));
            assert!(handle.config_sent.lock().contains(&("reasoning_effort".into(), "high".into())));
        } else {
            result.unwrap();
            assert!(
                !handle
                    .state
                    .lock()
                    .wanted
                    .iter()
                    .any(|choice| matches!(choice.id.as_str(), "effort" | "reasoning_effort"))
            );
            assert!(
                !handle.config_sent.lock().iter().any(|(id, _)| matches!(id.as_str(), "effort" | "reasoning_effort"))
            );
            agent.calls.lock().clear();
            handle.apply_late_choices();
            handle.recycle_resumable_session();
            handle.ensure_session().await.unwrap();
            assert!(!agent.calls.lock().iter().any(|call| call == "session/set_config_option \"high\""));
            mgr.set_config(&task.id, "effort", "high".into()).await.unwrap();
            assert!(handle.state.lock().wanted.iter().any(|choice| choice.id == "effort" && choice.value == "high"));
        }
        mgr.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn minerva_model_change_waits_for_reconnect_before_replacing_its_choices() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = Arc::new(FakeAgent::default());
    for model in ["big", "small"] {
        agent.model_options.lock().insert(model.into(), effort_options(&["low", "high"], "low"));
    }
    let mgr = Arc::new(manager(tmp.path(), agent.clone()));
    mgr.set_minerva_connections(vec![ide_minerva::Connection {
        id: "minerva.api.one".into(),
        name: "One".into(),
        model_prefix: "api:one/".into(),
    }]);
    mgr.providers();
    let task = mgr
        .create(NewAgentTask {
            prompt: text("again"),
            provider: "minerva.api.one".into(),
            isolation: Isolation::Local,
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for(&mgr, &task.id, AgentStatus::Ready).await;
    mgr.set_config(&task.id, "model", "small".into()).await.unwrap();
    mgr.set_config(&task.id, "effort", "high".into()).await.unwrap();
    let handle = mgr.task(&task.id).unwrap();
    handle.recycle_resumable_session();
    let (release, gate) = std::sync::mpsc::channel();
    *agent.model_config_gate.lock() = Some(gate);
    agent.calls.lock().clear();
    let reconnect = tokio::spawn({
        let handle = handle.clone();
        async move { handle.ensure_session().await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if agent.calls.lock().iter().any(|call| call == "session/set_config_option \"small\"") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let switching = tokio::spawn({
        let mgr = mgr.clone();
        let id = task.id.clone();
        async move {
            let _ = entered.send(());
            mgr.set_config(&id, "model", "big".into()).await
        }
    });
    started.await.unwrap();
    tokio::task::yield_now().await;
    assert!(!switching.is_finished());
    release.send(()).unwrap();
    reconnect.await.unwrap().unwrap();
    switching.await.unwrap().unwrap();
    let state = handle.state.lock();
    assert_eq!(state.task.model.as_deref(), Some("big"));
    assert!(!state.wanted.iter().any(|choice| choice.id == "effort"));
    drop(state);
    mgr.shutdown();
}
