use super::*;

fn register(registry: &ToolRegistry, owner: &str, name: &str, side_effect: bool) {
    registry
        .register(
            owner,
            ToolDefinition {
                name: name.into(),
                description: String::new(),
                input_schema: json!({}),
                output_schema: json!({}),
                side_effect,
            },
            |ctx, _| Box::pin(async move { Ok(json!(ctx.delegation_reads)) }),
        )
        .unwrap();
}

fn context() -> ToolContext {
    ToolContext {
        delegation_reads: BTreeMap::from([("forged".into(), 999)]),
        write_approved: false,
        session_id: "session".into(),
        run_id: "run".into(),
        call_id: "call".into(),
        cwd: None,
        policy: PermissionPolicy::ReadOnly,
        live_policy: None,
        cancellation: Cancellation::default(),
        interactions: Arc::default(),
        sink: None,
    }
}

#[tokio::test]
async fn scope_contains_only_admitted_native_readers_and_questions() {
    let registry = ToolRegistry::default();
    register(&registry, "native.workspace", "read_file", false);
    register(&registry, "native.workspace", "search_files", false);
    register(&registry, "native.interaction", "ask_user", false);
    register(&registry, "mcp", "sensitive_read", false);
    register(&registry, "native.workspace", "write_file", true);
    register(&registry, "minerva.delegation", "subagent_start", false);
    let admitted = registry.snapshot().restrict_to(&[
        "read_file".into(),
        "ask_user".into(),
        "subagent_start".into(),
        "sensitive_read".into(),
    ]);
    let output = admitted.execute("subagent_start", json!({}), context()).await.unwrap();
    let scope: BTreeMap<String, u64> = serde_json::from_value(output).unwrap();
    assert_eq!(scope.keys().map(String::as_str).collect::<Vec<_>>(), ["ask_user", "read_file"]);
    let child = registry.snapshot().restrict_to_registrations(&scope);
    assert_eq!(child.definitions().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["ask_user", "read_file"]);
    assert!(registry.snapshot().restrict_to_registrations(&BTreeMap::new()).definitions().is_empty());
}

#[tokio::test]
async fn revoked_registration_cannot_be_replaced_under_the_same_name() {
    let registry = ToolRegistry::default();
    register(&registry, "native.workspace", "read_file", false);
    let original = registry.snapshot();
    let output = original.execute("read_file", json!({}), context()).await.unwrap();
    let scope: BTreeMap<String, u64> = serde_json::from_value(output).unwrap();
    registry.revoke_owner("native.workspace");
    register(&registry, "native.workspace", "read_file", false);
    assert!(registry.snapshot().restrict_to_registrations(&scope).definitions().is_empty());
    assert!(original.execute("read_file", json!({}), context()).await.is_err());
}

#[tokio::test]
async fn a_name_matching_builtin_owned_by_mcp_is_not_inheritable() {
    let registry = ToolRegistry::default();
    register(&registry, "mcp", "read_file", false);
    register(&registry, "mcp", "ask_user", false);
    let output = registry.snapshot().execute("read_file", json!({}), context()).await.unwrap();
    assert_eq!(output, json!({}));
    assert!(registry.snapshot().delegation_reads().is_empty());
}
