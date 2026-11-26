use ide_minerva::minerva_runtime::tools::{Cancellation, ToolContext};
use minerva_plugin_sdk::{
    delegation::{DelegationSnapshot, NativeCheckout},
    tools::PermissionPolicy,
};
use parking_lot::Mutex;
use serde_json::Value;

pub(crate) struct ScopedNative {
    snapshot: DelegationSnapshot,
    grant: NativeCheckout,
    binding: Mutex<Option<(String, String)>>,
    cancellation: Cancellation,
    file_calls: tokio::sync::RwLock<()>,
    file_call_ids: Mutex<std::collections::HashSet<String>>,
}

impl ScopedNative {
    pub(super) fn new(snapshot: DelegationSnapshot, grant: NativeCheckout) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            snapshot,
            grant,
            binding: Mutex::new(None),
            cancellation: Cancellation::default(),
            file_calls: tokio::sync::RwLock::new(()),
            file_call_ids: Mutex::new(Default::default()),
        })
    }

    #[cfg(windows)]
    pub(crate) fn bind_process(&self, session: &str, turn: &str, job: &str) -> Result<(), String> {
        self.grant.record_process(&self.snapshot, job)?;
        *self.binding.lock() = Some((session.into(), turn.into()));
        Ok(())
    }

    #[cfg(windows)]
    pub(crate) fn revoke(&self) {
        self.cancel();
        self.binding.lock().take();
    }

    #[cfg(windows)]
    pub(crate) fn stopped(&self, job: &str) {
        self.grant.confirm_process_stopped(job);
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    #[cfg(windows)]
    pub(crate) async fn drain_file_calls(&self) {
        let _drained = self.file_calls.write().await;
    }

    fn bound_policy(&self, params: &Value) -> PermissionPolicy {
        let binding = self.binding.lock();
        if !self.cancellation.is_cancelled()
            && binding.as_ref().is_some_and(|(session, turn)| {
                params["sessionId"].as_str() == Some(session)
                    && params["turnId"].as_str() == Some(turn)
                    && params["toolUseId"].as_str().is_some_and(|id| !id.is_empty())
            })
        {
            self.grant.write_policy(&self.snapshot)
        } else {
            PermissionPolicy::ReadOnly
        }
    }

    pub(crate) async fn apply_file_tool(
        self: &std::sync::Arc<Self>,
        params: &Value,
        service: Option<&std::sync::Arc<ide_minerva::Service>>,
    ) -> Result<Value, String> {
        let _active = self.file_calls.read().await;
        let name = match params["name"].as_str() {
            Some(name @ ("write_file" | "patch_file")) => name,
            _ => return Err("Tool is unavailable in scoped file editing".into()),
        };
        let policy = self.bound_policy(params);
        if policy == PermissionPolicy::ReadOnly {
            return Err("No active checkout write grant".into());
        }
        let call = params["toolUseId"].as_str().ok_or("Missing file tool call ID")?;
        {
            let mut seen = self.file_call_ids.lock();
            if call.len() > 256 || seen.len() >= 1024 || !seen.insert(call.to_owned()) {
                return Err("Repeated or invalid file tool call".into());
            }
        }
        let service = service.ok_or("OpenIDEA file tool host is unavailable")?;
        let runtime = service.runtime().await?;
        let run = self.snapshot.request.origin_run.as_ref().ok_or("Missing parent run")?;
        let tools = runtime
            .plugins
            .harness
            .tools
            .snapshot()
            .restrict_delegation(&self.snapshot.request.tool_registrations, true);
        let context = ToolContext {
            delegation_reads: Default::default(),
            write_approved: false,
            session_id: self.snapshot.caller.session_id.clone(),
            run_id: run.clone(),
            call_id: format!(
                "delegation:{}:{}",
                self.snapshot.activation_id,
                params["toolUseId"].as_str().unwrap_or_default()
            ),
            cwd: Some(self.snapshot.caller.cwd.clone().into()),
            policy,
            live_policy: Some(std::sync::Arc::new(FilePolicy { scope: self.clone(), params: params.clone() })),
            cancellation: self.cancellation.clone(),
            interactions: runtime.plugins.harness.interactions.clone(),
            sink: None,
        };
        tools.execute(name, params["arguments"].clone(), context).await
    }

    fn write_policy(&self, params: &Value) -> PermissionPolicy {
        if matches!(params["name"].as_str(), Some("Write" | "Edit"))
            && params["path"]
                .as_str()
                .is_some_and(|path| std::path::Path::new(path).starts_with(&self.snapshot.caller.cwd))
        {
            self.bound_policy(params)
        } else {
            PermissionPolicy::ReadOnly
        }
    }

    pub(crate) async fn authorize(
        &self,
        params: &Value,
        service: Option<&std::sync::Arc<ide_minerva::Service>>,
    ) -> bool {
        match self.write_policy(params) {
            PermissionPolicy::ReadOnly => return false,
            PermissionPolicy::Allow => return true,
            PermissionPolicy::Ask => {}
        }
        let Some(service) = service else { return false };
        let Ok(runtime) = service.runtime().await else { return false };
        let Some(run) = self.snapshot.request.origin_run.as_ref() else { return false };
        let Some(name) = params["name"].as_str() else { return false };
        let Some(path) = params["path"].as_str() else { return false };
        let Some(tool_use) = params["toolUseId"].as_str() else { return false };
        if self.write_policy(params) == PermissionPolicy::ReadOnly {
            return false;
        }
        let context = ToolContext {
            delegation_reads: Default::default(),
            write_approved: false,
            session_id: self.snapshot.caller.session_id.clone(),
            run_id: run.clone(),
            call_id: format!("delegation:{}:{tool_use}", self.snapshot.activation_id),
            cwd: Some(self.snapshot.caller.cwd.clone().into()),
            policy: PermissionPolicy::Ask,
            live_policy: None,
            cancellation: self.cancellation.clone(),
            interactions: runtime.plugins.harness.interactions.clone(),
            sink: None,
        };
        let prompt = format!("Allow {name} on {path:?} for this call?");
        context
            .interactions
            .request(&context, "approval", name, &prompt, vec![])
            .await
            .is_ok_and(|answer| answer == "allow")
            && self.write_policy(params) != PermissionPolicy::ReadOnly
    }
}

struct FilePolicy {
    scope: std::sync::Arc<ScopedNative>,
    params: Value,
}

impl ide_minerva::minerva_runtime::tools::LivePermissionPolicy for FilePolicy {
    fn read_policy(&self, _: &str) -> Result<PermissionPolicy, String> {
        Ok(self.scope.bound_policy(&self.params))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use minerva_plugin_sdk::delegation::NativeProcessJournal;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Journal;
    impl NativeProcessJournal for Journal {
        fn record(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn confirm_stopped(&self, _: &str) {}
    }

    #[test]
    fn approval_requires_the_journaled_turn_and_current_write_policy() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot: DelegationSnapshot = serde_json::from_value(json!({
            "id":"child","workItemId":"work","activationId":"activation","depth":1,"state":"starting",
            "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":dir.path()},
            "request":{"operationId":"operation","target":{"kind":"ide-agent","agentId":"claude","options":{}},"prompt":"Edit"}
        })).unwrap();
        let writable = Arc::new(AtomicBool::new(true));
        let current = writable.clone();
        let grant = NativeCheckout::new(&snapshot, PermissionPolicy::Allow, move || {
            if current.load(Ordering::Acquire) { PermissionPolicy::Allow } else { PermissionPolicy::ReadOnly }
        })
        .with_process_journal(Some(Arc::new(Journal)));
        let scope = ScopedNative::new(snapshot, grant);
        let mut request = json!({"sessionId":"session","turnId":"turn","toolUseId":"edit","name":"Write","path":dir.path().join("file.txt")});
        assert_eq!(scope.write_policy(&request), PermissionPolicy::ReadOnly);
        scope.bind_process("session", "turn", "job").unwrap();
        assert_eq!(scope.write_policy(&request), PermissionPolicy::Allow);
        request["turnId"] = json!("stale");
        assert_eq!(scope.write_policy(&request), PermissionPolicy::ReadOnly);
        request["turnId"] = json!("turn");
        writable.store(false, Ordering::Release);
        assert_eq!(scope.write_policy(&request), PermissionPolicy::ReadOnly);
        writable.store(true, Ordering::Release);
        scope.revoke();
        scope.stopped("job");
        assert_eq!(scope.write_policy(&request), PermissionPolicy::ReadOnly);
    }
    #[tokio::test]
    async fn native_ask_routes_to_the_parent_broker_and_rechecks_live_policy() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(ide_minerva::Service::new(dir.path().join("data"), dir.path().join("config")));
        let runtime = service.runtime().await.unwrap();
        let broker = runtime.plugins.harness.interactions.clone();
        let snapshot: DelegationSnapshot = serde_json::from_value(json!({
            "id":"child","workItemId":"work","activationId":"activation","depth":1,"state":"starting",
            "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":dir.path()},
            "request":{"operationId":"operation","originRun":"parent-run","target":{"kind":"ide-agent","agentId":"claude","options":{}},"prompt":"Edit"}
        })).unwrap();
        let writable = Arc::new(AtomicBool::new(true));
        let current = writable.clone();
        let grant = NativeCheckout::new(&snapshot, PermissionPolicy::Ask, move || {
            if current.load(Ordering::Acquire) { PermissionPolicy::Ask } else { PermissionPolicy::ReadOnly }
        })
        .with_process_journal(Some(Arc::new(Journal)));
        let scope = ScopedNative::new(snapshot, grant);
        scope.bind_process("session", "turn", "job").unwrap();
        for expected in [true, false] {
            let params = json!({"sessionId":"session","turnId":"turn","toolUseId":"edit","name":"Write","path":dir.path().join("file.txt")});
            let approve = scope.authorize(&params, Some(&service));
            let answer = async {
                let pending = loop {
                    if let Some(pending) = broker.list("parent").into_iter().next() {
                        break pending;
                    }
                    tokio::task::yield_now().await;
                };
                assert_eq!(pending.run_id, "parent-run");
                assert_eq!(pending.call_id, "delegation:activation:edit");
                assert!(pending.prompt.contains("file.txt"));
                writable.store(expected, Ordering::Release);
                assert!(broker.answer(minerva_plugin_sdk::tools::InteractionAnswer {
                    session_id: pending.session_id,
                    id: pending.id,
                    decision: "allow".into(),
                    answer: None,
                }));
            };
            let (approved, ()) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async { tokio::join!(approve, answer) })
                    .await
                    .unwrap();
            assert_eq!(approved, expected);
        }
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn codex_file_calls_use_host_tools_once_and_cannot_escape_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let service = Arc::new(ide_minerva::Service::new(dir.path().join("data"), dir.path().join("config")));
        let mut snapshot: DelegationSnapshot = serde_json::from_value(json!({
            "id":"child","workItemId":"work","activationId":"activation","depth":1,"state":"starting",
            "caller":{"sessionId":"parent","rootSessionId":"parent","workspaceId":"workspace","cwd":checkout},
            "request":{"operationId":"operation","originRun":"parent-run","target":{"kind":"ide-agent","agentId":"codex","options":{}},"prompt":"Edit"}
        })).unwrap();
        let runtime = service.runtime().await.unwrap();
        let registry = &runtime.plugins.harness.tools;
        snapshot.request.tool_registrations = registry.snapshot().delegation_writes();
        let grant = NativeCheckout::new(&snapshot, PermissionPolicy::Allow, || PermissionPolicy::Allow)
            .with_process_journal(Some(Arc::new(Journal)));
        let scope = ScopedNative::new(snapshot, grant);
        scope.bind_process("thread", "host-grant", "job").unwrap();
        let mut request = json!({"sessionId":"thread","turnId":"host-grant","toolUseId":"file","name":"write_file","arguments":{"path":"file.txt","content":"first"}});
        scope.apply_file_tool(&request, Some(&service)).await.unwrap();
        request["arguments"]["content"] = json!("duplicate");
        assert!(scope.apply_file_tool(&request, Some(&service)).await.is_err());
        assert_eq!(std::fs::read_to_string(checkout.join("file.txt")).unwrap(), "first");
        request["toolUseId"] = json!("outside");
        request["arguments"]["path"] = json!("../outside.txt");
        assert!(scope.apply_file_tool(&request, Some(&service)).await.is_err());
        assert!(!dir.path().join("outside.txt").exists());
        let definition = registry.snapshot().definitions().into_iter().find(|tool| tool.name == "write_file").unwrap();
        registry.revoke_owner("native.workspace");
        registry.register("native.workspace", definition, |_, _| panic!("replacement tool was invoked")).unwrap();
        request["toolUseId"] = json!("replaced");
        request["arguments"]["path"] = json!("replaced.txt");
        assert!(scope.apply_file_tool(&request, Some(&service)).await.is_err());
        assert!(!checkout.join("replaced.txt").exists());
        request["toolUseId"] = json!("stale");
        request["arguments"]["path"] = json!("stale.txt");
        scope.revoke();
        assert!(scope.apply_file_tool(&request, Some(&service)).await.is_err());
        assert!(!checkout.join("stale.txt").exists());
        service.shutdown().await.unwrap();
    }
}
