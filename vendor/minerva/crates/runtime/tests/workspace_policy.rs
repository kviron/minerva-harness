use minerva_runtime::{Runtime, RuntimeConfig};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn permission_action_is_narrow_and_available_during_an_active_run() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        Runtime::start(RuntimeConfig {
            tenant_id: "t".into(),
            workspace_id: "w".into(),
            profile: "desktop".into(),
            data_dir: dir.path().join("data"),
            config_dir: dir.path().join("config"),
        })
        .await
        .unwrap(),
    );
    runtime.create_session("one", None).unwrap();
    let mut before = runtime.harness_call("workspace.get", json!({"sessionId":"one"})).await.unwrap();
    before["contextWindow"] = json!(8192);
    before["autoCompact"] = json!(false);
    before["modelEffort"] = json!(["model", "high"]);
    let before = runtime.harness_call("workspace.save", json!({"sessionId":"one","settings":before})).await.unwrap();
    runtime.sessions.begin_run("one", "active", "user", &json!({}), None).unwrap();
    assert!(runtime.sessions.baseline("one").unwrap().active_run.is_some());
    let after = runtime
        .harness_call("workspace.set_permission_policy", json!({"sessionId":"one","permissionPolicy":"allow"}))
        .await
        .unwrap();
    let mut expected = before.clone();
    expected["permissionPolicy"] = json!("allow");
    assert_eq!(after, "allow");
    assert_eq!(runtime.harness_call("workspace.get", json!({"sessionId":"one"})).await.unwrap(), expected);
    let error =
        runtime.harness_call("workspace.save", json!({"sessionId":"one","settings":expected})).await.unwrap_err();
    assert!(error.contains("active run"), "{error}");
    for params in [
        json!({"sessionId":"one","permissionPolicy":"invalid"}),
        json!({"sessionId":"one","permissionPolicy":"ask","cwd":"elsewhere"}),
        json!({"sessionId":"missing","permissionPolicy":"allow"}),
    ] {
        assert!(runtime.harness_call("workspace.set_permission_policy", params).await.is_err());
    }
    assert_eq!(runtime.harness_call("workspace.get", json!({"sessionId":"one"})).await.unwrap(), expected);
    assert_eq!(runtime.sessions.baseline("one").unwrap().active_run.unwrap().run_id, "active");
    runtime.sessions.fail_run("one", "active", true, "fixture finished").unwrap();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn hosted_runtime_cannot_use_the_internal_live_policy_action() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        Runtime::start_without_packages(RuntimeConfig {
            tenant_id: "t".into(),
            workspace_id: "w".into(),
            profile: "hosted".into(),
            data_dir: dir.path().join("data"),
            config_dir: dir.path().join("config"),
        })
        .await
        .unwrap(),
    );
    runtime.create_session("one", None).unwrap();
    let before = runtime.harness_call("workspace.get", json!({"sessionId":"one"})).await.unwrap();
    let error = runtime
        .harness_call("workspace.set_permission_policy", json!({"sessionId":"one","permissionPolicy":"allow"}))
        .await
        .unwrap_err();
    assert!(error.contains("unavailable in this profile"), "{error}");
    assert_eq!(runtime.harness_call("workspace.get", json!({"sessionId":"one"})).await.unwrap(), before);
    runtime.shutdown().await.unwrap();
}
