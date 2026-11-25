//! Subscription credentials and model transport; the agent loop stays in Rig.
use super::credential_store::{CredentialScope, CredentialStore};
use rig_core::{
    catalog::Catalog,
    client::signed_in::{CredentialSource, SignedInModel},
    http_client::DynHttpClient,
    providers::chatgpt::auth::{AuthSource, Authenticator, DeviceCodeHandler, TokenStore},
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

pub const ID: &str = "openai-codex";
pub const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

#[cfg(test)]
#[path = "codex_integration_tests.rs"]
mod integration_tests;

#[path = "codex_store.rs"]
mod store;
use store::Store;
#[path = "codex_catalog.rs"]
mod catalog;
#[path = "codex_usage.rs"]
mod usage;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub authorization_url: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone)]
pub struct Service {
    auth: Authenticator,
    store: Store,
    state: Arc<Mutex<Status>>,
    generation: Arc<AtomicU64>,
    login: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    http: DynHttpClient,
    enabled: bool,
    models: catalog::Models,
    catalog_gate: Arc<Mutex<()>>,
}
#[derive(Clone)]
pub struct Credential {
    auth: Authenticator,
    generation: Arc<AtomicU64>,
    expected: u64,
    state: Arc<Mutex<Status>>,
}
impl CredentialSource for Credential {
    async fn auth_context(
        &self,
        http: &DynHttpClient,
    ) -> Result<rig_core::providers::chatgpt::auth::AuthContext, rig_core::providers::chatgpt::auth::AuthError> {
        use rig_core::providers::chatgpt::auth::AuthError;
        let admitted = || {
            self.generation.load(Ordering::SeqCst) == self.expected
                && self.state.lock().is_ok_and(|state| !matches!(state.state.as_str(), "signing-in" | "signing-out"))
        };
        if !admitted() {
            return Err(AuthError::Message("Codex connection changed".into()));
        }
        let context =
            self.auth.auth_context(http).await.map_err(|_| {
                AuthError::Message("Codex authorization failed; sign in again in Minerva settings".into())
            })?;
        if !admitted() {
            return Err(AuthError::Message("Codex connection changed".into()));
        }
        Ok(context)
    }
}
impl Service {
    pub fn new(path: PathBuf, scope: CredentialScope, credentials: Arc<dyn CredentialStore>, enabled: bool) -> Self {
        let models = catalog::Models::new(path.with_file_name("codex-models.json"));
        let store = Store::new(path, scope, credentials);
        Self {
            auth: Authenticator::new(AuthSource::OAuth, store.clone(), DeviceCodeHandler::default(), false),
            store,
            state: Arc::new(Mutex::new(Status { state: "signed-out".into(), authorization_url: None, error: None })),
            generation: Arc::new(AtomicU64::new(0)),
            login: Arc::new(Mutex::new(None)),
            http: rig_reqwest::shared(),
            enabled,
            models,
            catalog_gate: Arc::new(Mutex::new(())),
        }
    }
    pub fn has_session(&self) -> Result<bool, String> {
        if !self.enabled {
            return Ok(false);
        }
        if self.state.lock().map_err(|_| "Codex state unavailable")?.state == "signing-out" {
            return Ok(false);
        }
        let Some(record) = self.store.read()? else { return Ok(false) };
        let record: serde_json::Value =
            serde_json::from_str(&record).map_err(|_| "Stored Codex authorization is invalid; sign in again")?;
        Ok(record.get("access_token").and_then(serde_json::Value::as_str).is_some_and(|token| !token.is_empty()))
    }
    pub fn auth_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub fn status(&self) -> Result<Status, String> {
        let mut status = self.state.lock().map_err(|_| "Codex state unavailable")?.clone();
        if !matches!(status.state.as_str(), "signing-in" | "signing-out" | "error") {
            status.state = if self.has_session()? { "signed-in" } else { "signed-out" }.into();
        }
        Ok(status)
    }
    pub fn sign_in(&self) -> Result<Status, String> {
        if !self.enabled {
            return Err("Subscription sign-in is available only in local desktop sessions".into());
        }
        let mut login = self.login.lock().map_err(|_| "Codex state unavailable")?;
        if self.state.lock().map_err(|_| "Codex state unavailable")?.state == "signing-out" {
            return Err("Codex sign-out is still in progress".into());
        }
        if login.as_ref().is_some_and(|task| !task.is_finished()) {
            return self.status();
        }
        {
            let _gate = self.catalog_gate.lock().map_err(|_| "Codex catalog unavailable")?;
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.models.clear()?;
        }
        *self.state.lock().map_err(|_| "Codex state unavailable")? =
            Status { state: "signing-in".into(), authorization_url: None, error: None };
        let this = self.clone();
        *login = Some(tokio::spawn(async move {
            let state = this.state.clone();
            let result = this
                .auth
                .sign_in_with_browser(&this.http, move |page| {
                    if let Ok(mut state) = state.lock() {
                        state.authorization_url = Some(page.authorize_url);
                    }
                })
                .await;
            if let Ok(mut state) = this.state.lock() {
                *state = Status {
                    state: if result.is_ok() { "signed-in" } else { "error" }.into(),
                    authorization_url: None,
                    error: result.err().map(|_| "Codex sign-in failed. Please try again.".into()),
                };
            }
        }));
        self.status()
    }
    pub async fn sign_out(&self) -> Result<Status, String> {
        let task = {
            let mut login = self.login.lock().map_err(|_| "Codex state unavailable")?;
            let mut state = self.state.lock().map_err(|_| "Codex state unavailable")?;
            if state.state == "signing-out" {
                return Ok(state.clone());
            }
            state.state = "signing-out".into();
            let _gate = self.catalog_gate.lock().map_err(|_| "Codex catalog unavailable")?;
            self.generation.fetch_add(1, Ordering::SeqCst);
            if let Err(error) = self.models.clear() {
                *state = Status { state: "error".into(), authorization_url: None, error: Some(error.clone()) };
                return Err(error);
            }
            login.take()
        };
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        // Local logout must not depend on a remote revocation endpoint being reachable.
        let result = async {
            let _held =
                self.store.lock("chatgpt").await.map_err(|_| "Codex credential lock unavailable".to_string())?;
            self.store.clear()?;
            self.auth.forget();
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = result {
            *self.state.lock().map_err(|_| "Codex state unavailable")? = Status {
                state: "error".into(),
                authorization_url: None,
                error: Some("Could not remove Codex authorization. Please try again.".into()),
            };
            return Err(error);
        }
        *self.state.lock().map_err(|_| "Codex state unavailable")? =
            Status { state: "signed-out".into(), authorization_url: None, error: None };
        self.status()
    }
    pub async fn shutdown(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let task = self.login.lock().ok().and_then(|mut login| login.take());
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
    pub fn models(&self) -> Vec<String> {
        if let Some(models) = self.models.ids() {
            return models;
        }
        Catalog::builtin()
            .iter()
            .filter(|model| Some(model.provider) == rig_core::providers::registry::ProviderId::catalog("chatgpt"))
            .map(|model| model.id.clone())
            .collect()
    }
    pub fn model_spec(&self, model: &str) -> Option<rig_core::catalog::ModelSpec> {
        self.model_snapshot(model).map(|(spec, _)| spec)
    }
    pub fn reasoning_levels(&self, model: &str) -> Option<Vec<String>> {
        self.models.levels(model)
    }
    pub fn model_snapshot(&self, model: &str) -> Option<(rig_core::catalog::ModelSpec, Vec<String>)> {
        let _gate = self.catalog_gate.lock().ok()?;
        if self.models.ids().is_some() {
            return self.models.snapshot(model);
        }
        let spec = Catalog::builtin().resolve(&format!("chatgpt/{model}")).ok()?.spec.clone();
        let levels = spec
            .reasoning
            .choices()
            .iter()
            .filter(|choice| choice.name != "default")
            .map(|choice| choice.name.to_owned())
            .collect();
        Some((spec, levels))
    }
    pub async fn discover_models(&self) -> Result<Vec<String>, String> {
        use futures_util::StreamExt;
        use rig_core::http_client::{HttpClientExt, Request};
        let generation = self.auth_generation();
        let credentials = Credential {
            auth: self.auth.clone(),
            generation: self.generation.clone(),
            expected: generation,
            state: self.state.clone(),
        };
        let work = async {
            let context =
                credentials.auth_context(&self.http).await.map_err(|_| "Codex authorization failed; sign in again")?;
            let mut request = Request::builder()
                .uri(format!("{BASE_URL}/models?client_version=0.172.0"))
                .method("GET")
                .header("Authorization", format!("Bearer {}", context.api_key.expose()))
                .header("Accept", "application/json")
                .header("originator", "codex_cli_rs");
            if let Some(account) = context.account_id {
                request = request.header("ChatGPT-Account-Id", account);
            }
            let response = self
                .http
                .send_streaming(request.body(Vec::<u8>::new()).map_err(|_| "Cannot create Codex catalog request")?)
                .await
                .map_err(|_| "Cannot fetch Codex model catalog")?;
            if !response.status().is_success() {
                return Err(format!("Codex model catalog request failed (HTTP {})", response.status().as_u16()));
            }
            let mut bytes = Vec::new();
            let mut body = response.into_body();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|_| "Cannot read Codex model catalog")?;
                if bytes.len() + chunk.len() > 1024 * 1024 {
                    return Err("Codex model catalog is too large".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            let rows = catalog::Models::parse(&bytes)?;
            let _gate = self.catalog_gate.lock().map_err(|_| "Codex catalog unavailable")?;
            if generation != self.auth_generation() {
                return Err("Codex account changed while fetching models".into());
            }
            self.models.publish(rows)?;
            Ok(self.models())
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), work)
            .await
            .map_err(|_| "Codex model catalog request timed out".to_string())?
    }
    pub fn handler(&self, model: &str) -> Result<SignedInModel<Credential>, String> {
        if !self.has_session()? {
            return Err("Sign in to Codex in Minerva settings".into());
        }
        let spec = self.model_spec(model).ok_or("Unknown Codex model")?;
        self.handler_with_spec(spec)
    }
    pub fn handler_with_spec(&self, spec: rig_core::catalog::ModelSpec) -> Result<SignedInModel<Credential>, String> {
        if !self.has_session()? {
            return Err("Sign in to Codex in Minerva settings".into());
        }
        SignedInModel::new(
            spec,
            Credential {
                auth: self.auth.clone(),
                generation: self.generation.clone(),
                expected: self.auth_generation(),
                state: self.state.clone(),
            },
            self.http.clone(),
        )
        .map_err(|_| "Cannot configure Codex model".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{credential_store::MemoryCredentialStore, network_policy::EndpointPolicy, ProviderService};

    #[tokio::test]
    #[ignore = "requires explicit Minerva Codex account configuration"]
    async fn live_account_model_discovery() {
        let config = std::env::var_os("MINERVA_CODEX_VERIFY_CONFIG_DIR").expect("explicit config directory");
        let service = ProviderService::new(std::path::PathBuf::from(config).join("providers.json")).unwrap();
        let models = service.codex.discover_models().await.unwrap();
        assert!(!models.is_empty());
        for model in models {
            println!("{model}: {:?}", service.codex.reasoning_levels(&model).unwrap_or_default());
        }
    }

    #[tokio::test]
    async fn oauth_records_stay_in_injected_credential_store() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentialStore::default());
        let service =
            Service::new(dir.path().join("locks"), CredentialScope::new("test").unwrap(), credentials.clone(), true);
        let record = "{\"access_token\":\"secret-record\"}";
        service.store.save("chatgpt", record).await.unwrap();
        assert!(service.has_session().unwrap());
        assert_eq!(service.store.load("chatgpt").await.unwrap().as_deref(), Some(record));
        let status = serde_json::to_string(&service.status().unwrap()).unwrap();
        assert!(!status.contains("secret-record"));
        assert!(!dir.path().join("locks").exists());
        let before = service.auth_generation();
        service.sign_out().await.unwrap();
        assert!(!service.has_session().unwrap());
        assert!(service.auth_generation() > before);
    }

    #[tokio::test]
    async fn changed_login_rejects_credentials_before_network_access() {
        let generation = Arc::new(AtomicU64::new(1));
        let auth = Authenticator::new(
            AuthSource::OAuth,
            rig_core::providers::chatgpt::auth::MemoryTokenStore::default(),
            DeviceCodeHandler::default(),
            false,
        );
        let credential = Credential {
            auth,
            generation,
            expected: 0,
            state: Arc::new(Mutex::new(Status { state: "signed-in".into(), authorization_url: None, error: None })),
        };
        assert!(credential.auth_context(&rig_reqwest::shared()).await.is_err());
    }

    #[tokio::test]
    async fn different_configurations_have_separate_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentialStore::default());
        let scope = CredentialScope::new("test").unwrap();
        let a = Service::new(dir.path().join("a"), scope.clone(), credentials.clone(), true);
        let b = Service::new(dir.path().join("b"), scope, credentials, true);
        a.store.save("chatgpt", "{\"access_token\":\"fixture\"}").await.unwrap();
        assert!(a.has_session().unwrap());
        assert!(!b.has_session().unwrap());
        a.store.save("chatgpt", "broken").await.unwrap();
        assert!(a.has_session().is_err());
    }

    #[tokio::test]
    async fn subscription_catalog_persists_only_public_route_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let service = ProviderService::new_with_store(
            path.clone(),
            CredentialScope::new("test").unwrap(),
            Arc::new(MemoryCredentialStore::default()),
            EndpointPolicy::Local,
        )
        .unwrap();
        let models = service.codex.models();
        assert!(!models.is_empty());
        assert!(service.codex_configure(vec!["unknown-model".into()]).await.is_err());
        assert!(service.codex_configure(vec![models[0].clone(), models[0].clone()]).await.is_err());
        service.codex_configure(vec![models[0].clone()]).await.unwrap();
        let providers: Vec<super::super::Provider> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(providers[0].id, ID);
        assert_eq!(providers[0].base_url, BASE_URL);
        assert!(service.codex.handler(&models[0]).is_err());
    }

    #[test]
    fn subscription_wire_uses_account_and_stateless_codex_contract() {
        use rig_core::{
            completion::CompletionRequest,
            operation::Completion,
            wire::{Body, Mode, Operation, Wire},
        };
        let config = rig_core::providers::openai::OpenAIConfig::with_key(
            &rig_core::providers::chatgpt::DIALECT,
            "fixture-token",
        )
        .with_account_id("fixture-account");
        let wire = rig_core::providers::openai::responses_api::wire::Responses::new(config, "gpt-5.4");
        let request = Completion::prepare(CompletionRequest::new("hello"), &wire.describe()).unwrap();
        let encoded = wire.encode(request, Mode::Unary).unwrap();
        assert_eq!(encoded.request.uri().to_string(), format!("{BASE_URL}/responses"));
        assert_eq!(encoded.request.headers().get("authorization").unwrap(), "Bearer fixture-token");
        assert_eq!(encoded.request.headers().get("chatgpt-account-id").unwrap(), "fixture-account");
        let Body::Bytes(bytes) = encoded.request.body() else { panic!("expected JSON request") };
        let body: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(body["store"], false);
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }
}
