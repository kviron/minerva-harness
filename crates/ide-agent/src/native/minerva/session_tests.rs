use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_changes_keep_the_session_and_require_an_explicit_available_model() {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(Service::new(dir.path().join("data"), dir.path().join("config")));
    let runtime = service.runtime().await.unwrap();
    let first_provider = format!("continuity-{}", uuid::Uuid::new_v4());
    let second_provider = format!("continuity-{}", uuid::Uuid::new_v4());
    let first_prefix = format!("api:{first_provider}/");
    let second_prefix = format!("api:{second_provider}/");
    let first_model = format!("{first_prefix}model");
    let second_model = format!("{second_prefix}model");
    for id in [&first_provider, &second_provider] {
        runtime
            .plugins
            .providers()
            .unwrap()
            .save(
                serde_json::from_value(json!({
                    "id":id,"name":id,"baseUrl":"http://localhost:1234/v1","models":["model"],
                    "apiKey":null,"isNew":true,"protocol":"ollama"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let first = connect(service.clone(), dir.path(), Arc::new(ide_jsonrpc::NoopHandler), Some(first_prefix));
    let second = connect(service.clone(), dir.path(), Arc::new(ide_jsonrpc::NoopHandler), Some(second_prefix.clone()));
    let new: Value = first.request("session/new", json!({}), None).await.unwrap();
    let id = new["sessionId"].as_str().unwrap();
    runtime
        .sessions
        .save(
            id,
            serde_json::from_value(json!({
                "id":"original","parentId":null,"role":"user","createdAt":"1",
                "parts":[{"kind":"text","data":{"text":"Original question"}}],"extensions":{}
            }))
            .unwrap(),
            None,
        )
        .unwrap();
    let history = serde_json::to_value(runtime.sessions.load(id).unwrap()).unwrap();
    let loaded: Value = second.request("session/load", json!({"sessionId":id}), None).await.unwrap();
    assert_eq!(loaded["configOptions"][0]["currentValue"], first_model);
    assert_eq!(loaded["configOptions"][0]["options"].as_array().unwrap().len(), 1);
    assert_eq!(loaded["configOptions"][0]["options"][0]["value"], second_model);
    assert_eq!(runtime.sessions.get(id).unwrap().model.as_deref(), Some(first_model.as_str()));
    assert!(
        second
            .request::<_, Value>(
                "session/prompt",
                json!({
                    "sessionId":id,"prompt":[{"type":"text","text":"must select first"}]
                }),
                None
            )
            .await
            .is_err()
    );
    let selected: Value = second
        .request(
            "session/set_config_option",
            json!({
                "sessionId":id,"configId":"model","value":second_model
            }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(selected["configOptions"][0]["currentValue"], second_model);
    runtime.plugins.providers().unwrap().delete(&first_provider).await.unwrap();
    let resumed: Value = second.request("session/resume", json!({"sessionId":id}), None).await.unwrap();
    assert_eq!(resumed["configOptions"][0]["currentValue"], second_model);
    assert_eq!(runtime.sessions.list().unwrap().len(), 1);
    assert_eq!(serde_json::to_value(runtime.sessions.load(id).unwrap()).unwrap(), history);

    runtime.plugins.providers().unwrap().delete(&second_provider).await.unwrap();
    let missing: Value = second.request("session/load", json!({"sessionId":id}), None).await.unwrap();
    assert_eq!(missing["configOptions"][0]["currentValue"], second_model);
    assert!(missing["configOptions"][0]["options"].as_array().unwrap().is_empty());
    assert_eq!(runtime.sessions.get(id).unwrap().model.as_deref(), Some(second_model.as_str()));
    let other_root = dir.path().join("other");
    std::fs::create_dir(&other_root).unwrap();
    let other = connect(service.clone(), &other_root, Arc::new(ide_jsonrpc::NoopHandler), Some(second_prefix.clone()));
    assert!(other.request::<_, Value>("session/load", json!({"sessionId":id}), None).await.is_err());
    first.kill();
    second.kill();
    other.kill();
    service.shutdown().await.unwrap();
}
