//! Minerva lives inside the IDE backend; its agent loop uses the configured API providers.
use minerva_runtime::{Runtime, RuntimeConfig};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
mod delegation;
mod run_leases;
mod writable_delegation;
pub use run_leases::{ChildCheckoutLease, NativeRunLease};

pub use minerva_runtime;

pub struct Connection {
    pub id: String,
    pub name: String,
    pub model_prefix: String,
}

pub struct Service {
    config: RuntimeConfig,
    executors: Arc<delegation::ProjectExecutors>,
    runtime: Mutex<Option<Arc<Runtime>>>,
    run_leases: run_leases::NativeRunLeases,
    stopped: Mutex<bool>,
}

impl Service {
    pub fn bind_native_run_lease<T: Send + Sync + 'static>(
        self: &Arc<Self>,
        session: &str,
        run: &str,
        owner: Arc<T>,
    ) -> Result<NativeRunLease, String> {
        self.run_leases.bind(session, run, owner, Arc::downgrade(self), None)
    }

    pub fn bind_journaled_native_run_lease<T: minerva_runtime::delegation_contract::NativeProcessJournal + 'static>(
        self: &Arc<Self>,
        session: &str,
        run: &str,
        owner: Arc<T>,
    ) -> Result<NativeRunLease, String> {
        self.run_leases.bind(session, run, owner.clone(), Arc::downgrade(self), Some(owner))
    }

    pub fn native_run_lease(&self, session: &str, run: &str) -> Option<NativeRunLease> {
        self.run_leases.get(session, run)
    }

    /// Invoked with the parent tool dispatch barrier held. The launch pause and
    /// checkout reservation survive until the confirmed activation settles.
    pub async fn begin_native_run_handoff(
        &self,
        session: &str,
        run: &str,
        child_session: &str,
        activation: &str,
    ) -> Result<NativeRunLease, String> {
        if activation.is_empty() || !child_session.starts_with("del_") {
            return Err("Missing child activation".into());
        }
        let lease = self.native_run_lease(session, run).ok_or("Native run has no active checkout lease")?;
        let runtime = self.runtime().await?;
        if runtime.get_run(session, run)?.status != "running" {
            return Err("Parent run is no longer running".into());
        }
        let pause = runtime.plugins.jobs()?.try_pause_launches(session)?;
        lease.begin_handoff(child_session, activation, Box::new(pause))?;
        Ok(lease)
    }

    pub async fn connections(&self) -> Result<Vec<Connection>, String> {
        self.runtime().await?;
        Ok(vec![Connection { id: "minerva".into(), name: "Minerva".into(), model_prefix: String::new() }])
    }

    pub async fn shell_session_is_quiescent(&self, session: &str) -> bool {
        let runtime = self.runtime.lock().await;
        runtime
            .as_ref()
            .and_then(|runtime| runtime.plugins.jobs().ok())
            .is_some_and(|jobs| jobs.session_is_quiescent(session))
    }

    pub fn new(data_dir: PathBuf, config_dir: PathBuf) -> Self {
        Self {
            executors: Arc::default(),
            config: RuntimeConfig {
                tenant_id: "local".into(),
                workspace_id: "minerva-ide".into(),
                profile: "desktop".into(),
                data_dir,
                config_dir,
            },
            runtime: Mutex::new(None),
            run_leases: Default::default(),
            stopped: Mutex::new(false),
        }
    }

    pub async fn runtime(&self) -> Result<Arc<Runtime>, String> {
        let stopped = self.stopped.lock().await;
        if *stopped {
            return Err("Minerva is stopped".into());
        }
        let mut slot = self.runtime.lock().await;
        if let Some(runtime) = slot.as_ref() {
            return Ok(runtime.clone());
        }
        let runtime = Arc::new(
            Runtime::start_with_host_executor(self.config.clone(), self.executors.clone())
                .await
                .map_err(|e| e.to_string())?,
        );
        *slot = Some(runtime.clone());
        Ok(runtime)
    }

    pub fn register_project_executor(
        &self,
        root: &Path,
        executor: Arc<dyn minerva_runtime::delegation_contract::ExecutionPort>,
    ) -> std::io::Result<()> {
        self.executors.insert(root, executor)
    }

    /// WorkspaceService canonicalizes paths and deduplicates Windows casing.
    pub async fn workspace(&self, root: &Path) -> Result<minerva_runtime::workspaces::WorkspaceInfo, String> {
        let runtime = self.runtime().await?;
        let path = root.to_str().ok_or("Project path must be UTF-8")?;
        let value = runtime.harness_call("workspaces.create", serde_json::json!({"path":path})).await?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let mut stopped = self.stopped.lock().await;
        *stopped = true;
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.shutdown().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn configured_providers_share_one_minerva_connection() {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::new(dir.path().join("data"), dir.path().join("config"));
        let runtime = service.runtime().await.unwrap();
        for id in ["one", "two"] {
            runtime
                .plugins
                .providers()
                .unwrap()
                .save(
                    serde_json::from_value(serde_json::json!({
                        "id":id,"name":format!("Provider {id}"),"baseUrl":"http://localhost:1234/v1",
                        "models":["model"],"apiKey":null,"isNew":true,"protocol":"ollama"
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        let connections = service.connections().await.unwrap();
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].id, "minerva");
        assert_eq!(connections[0].name, "Minerva");
        assert!(connections[0].model_prefix.is_empty());
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn project_workspaces_are_deduplicated_and_keep_their_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let service = Service::new(dir.path().join("data"), dir.path().join("config"));
        let first = service.workspace(&a).await.unwrap();
        let runtime = service.runtime().await.unwrap();
        runtime.create_session("kept", Some(&first.id)).unwrap();
        let second = service.workspace(&b).await.unwrap();
        assert_ne!(first.id, second.id);
        let restored = service.workspace(&a.join(".")).await.unwrap();
        assert_eq!(first.id, restored.id);
        assert_eq!(restored.session_ids, ["kept"]);
        service.shutdown().await.unwrap();
        assert!(service.runtime().await.is_err());
        let reopened = Service::new(dir.path().join("data"), dir.path().join("config"));
        assert_eq!(reopened.workspace(&a).await.unwrap().session_ids, ["kept"]);
        reopened.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn installed_packages_are_ignored_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("package");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("backend.exe"), b"not executed").unwrap();
        std::fs::write(source.join("plugin.json"), r#"{"id":"test.import","name":"Imported package","version":"1.0.0","description":"Fixture","apiVersion":1,"requires":{},"capabilities":[],"backend":{"kind":"process","entry":"backend.exe"}}"#).unwrap();
        let service = Service::new(dir.path().join("data"), dir.path().join("config"));
        minerva_plugin_host::install_local(&source, &dir.path().join("config")).unwrap();
        let runtime = service.runtime().await.unwrap();
        let list = serde_json::to_value(runtime.plugins.list().await.unwrap()).unwrap();
        assert!(!list.as_array().unwrap().iter().any(|p| p["id"] == "test.import"));
        assert!(dir.path().join("config/plugins/test.import/backend.exe").is_file());
        service.shutdown().await.unwrap();
    }
}
