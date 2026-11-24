//! Minerva lives inside the IDE backend; its agent loop uses the configured API providers.
use minerva_runtime::{Runtime, RuntimeConfig};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

pub use minerva_runtime;

pub struct Connection {
    pub id: String,
    pub name: String,
    pub model_prefix: String,
}

pub struct Service {
    config: RuntimeConfig,
    runtime: Mutex<Option<Arc<Runtime>>>,
    stopped: Mutex<bool>,
}

impl Service {
    pub async fn connections(&self) -> Result<Vec<Connection>, String> {
        let runtime = self.runtime().await?;
        #[derive(serde::Deserialize)]
        struct Provider {
            id: String,
            name: String,
        }
        let providers: Vec<Provider> = serde_json::from_value(
            serde_json::to_value(runtime.plugins.providers()?.list().await).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(providers
            .into_iter()
            .map(|provider| Connection {
                id: format!("minerva.api.{}", provider.id),
                name: provider.name,
                model_prefix: format!("api:{}/", provider.id),
            })
            .collect())
    }

    pub fn new(data_dir: PathBuf, config_dir: PathBuf) -> Self {
        Self {
            config: RuntimeConfig {
                tenant_id: "local".into(),
                workspace_id: "minerva-ide".into(),
                profile: "desktop".into(),
                data_dir,
                config_dir,
            },
            runtime: Mutex::new(None),
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
        let runtime = Arc::new(Runtime::start_without_packages(self.config.clone()).await.map_err(|e| e.to_string())?);
        *slot = Some(runtime.clone());
        Ok(runtime)
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
    async fn configured_providers_are_connections_without_a_minerva_entry() {
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
        assert_eq!(connections.len(), 2);
        assert!(connections.iter().any(|connection| connection.id == "minerva.api.one"
            && connection.name == "Provider one"
            && connection.model_prefix == "api:one/"));
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
