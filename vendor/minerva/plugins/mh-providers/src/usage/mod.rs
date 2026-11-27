//! Read-only subscription metering. Credentials never enter the returned DTOs.
mod adapter;
mod cache;
mod parse;
mod snapshot;
mod transport;
#[cfg(test)]
use crate::codex;
use crate::ProviderService;
use serde::Serialize;
use snapshot::UsageIdentity;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const CACHE_TTL: Duration = Duration::from_secs(60);
const MANUAL_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "unit", content = "amount", rename_all = "camelCase")]
pub enum Balance {
    Tokens(u64),
    Usd(f64),
    Unlimited,
}
#[derive(Default)]
pub(crate) struct Reading {
    windows: Vec<UsageWindow>,
    balance: Option<Balance>,
}
impl From<Vec<UsageWindow>> for Reading {
    fn from(windows: Vec<UsageWindow>) -> Self {
        Self { windows, balance: None }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub window_seconds: Option<u64>,
    pub used_percent: f64,
    pub reset_at: Option<u64>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageProvider {
    pub provider_id: String,
    pub provider_name: String,
    pub status: String,
    pub windows: Vec<UsageWindow>,
    pub balance: Option<Balance>,
    pub updated_at: Option<u64>,
    pub error_code: Option<String>,
}
impl UsageProvider {
    fn empty(id: &str, name: &str) -> Self {
        Self {
            provider_id: id.into(),
            provider_name: name.into(),
            status: "not-configured".into(),
            windows: vec![],
            balance: None,
            updated_at: None,
            error_code: None,
        }
    }
    pub(crate) fn result(mut self, result: Result<Reading, &'static str>) -> Self {
        match result {
            Ok(reading) => {
                self.status = "ok".into();
                self.windows = reading.windows;
                self.balance = reading.balance;
                self.updated_at = Some(now());
            }
            Err(code) => {
                self.status = "unavailable".into();
                self.error_code = Some(code.into());
            }
        }
        self
    }
}
pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
#[derive(Default, Clone)]
pub(crate) struct Cache(Arc<Mutex<Option<Entry>>>);
struct Entry {
    key: UsageIdentity,
    at: Instant,
    rows: Vec<UsageProvider>,
}
impl ProviderService {
    pub async fn usage(&self, refresh: bool) -> Vec<UsageProvider> {
        // The gate also deduplicates calls from multiple windows. Keys remain backend-only.
        let mut cache = self.usage_cache.0.lock().await;
        if self.endpoint_policy == crate::network_policy::EndpointPolicy::Hosted {
            return vec![];
        }
        let snapshot = self.usage_snapshot().await;
        if let Some(entry) = cache.as_ref() {
            if entry.key == snapshot.identity
                && entry.at.elapsed() < if refresh { MANUAL_REFRESH_INTERVAL } else { CACHE_TTL }
            {
                let mut rows = entry.rows.clone();
                snapshot.apply_names(&mut rows);
                return rows;
            }
        }
        let mut rows = futures_util::future::join_all(snapshot.sources.iter().map(|source| source.fetch(self))).await;
        // A changed account/config may never publish an older snapshot.
        let current = self.usage_snapshot().await;
        if snapshot.identity != current.identity {
            return vec![];
        }
        let previous = cache.as_ref().filter(|entry| entry.key == snapshot.identity).map(|entry| entry.rows.as_slice());
        cache::retain_samples(&mut rows, previous, now());
        current.apply_names(&mut rows);
        *cache = Some(Entry { key: snapshot.identity, at: Instant::now(), rows: rows.clone() });
        rows
    }
}
pub(crate) use parse::codex as parse_codex;
pub(crate) use transport::{get, REQUEST_TIMEOUT};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credential_store::{CredentialScope, MemoryCredentialStore},
        network_policy::EndpointPolicy,
        Provider,
    };
    fn service(dir: &std::path::Path) -> ProviderService {
        ProviderService::new_with_store(
            dir.join("providers.json"),
            CredentialScope::new("usage-test").unwrap(),
            Arc::new(MemoryCredentialStore::default()),
            EndpointPolicy::Local,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn cache_invalidates_on_account_and_config_change() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path());
        let rows = service.usage(false).await;
        assert_eq!(rows[0].status, "not-configured");
        assert!(service.usage_cache.0.lock().await.is_some());
        service.providers.lock().await.push(Provider {
            credit_grants: None,
            model_windows: Default::default(),
            id: "opencode-go".into(),
            name: "Go".into(),
            base_url: "https://opencode.ai/inference/go".into(),
            models: vec![],
            protocol: "openai-chat".into(),
        });
        assert_eq!(service.usage(false).await.len(), 2);
        service.providers.lock().await[0].base_url = "https://gateway.example".into();
        assert_eq!(service.usage(false).await.len(), 1);
        service.codex.sign_out().await.unwrap();
        assert_eq!(service.usage(false).await[0].status, "not-configured");
    }
    #[tokio::test]
    async fn catalog_edits_keep_cached_sample_but_update_its_caption() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path());
        service.providers.lock().await.push(Provider {
            credit_grants: None,
            model_windows: Default::default(),
            id: "opencode-go".into(),
            name: "Original".into(),
            base_url: "https://opencode.ai/inference/go".into(),
            protocol: "openai-chat".into(),
            models: vec!["one".into(), "two".into()],
        });
        let snapshot = service.usage_snapshot().await;
        let row = UsageProvider::empty("opencode-go", "Original").result(Ok(vec![UsageWindow {
            id: "5h".into(),
            window_seconds: Some(18000),
            used_percent: 42.0,
            reset_at: None,
        }]
        .into()));
        let observed = row.updated_at;
        *service.usage_cache.0.lock().await =
            Some(Entry { key: snapshot.identity, at: Instant::now(), rows: vec![row] });
        {
            let mut providers = service.providers.lock().await;
            providers[0].name = "Renamed".into();
            providers[0].models.reverse();
            providers.push(Provider {
                credit_grants: None,
                model_windows: Default::default(),
                id: "unrelated".into(),
                name: "Other".into(),
                base_url: "https://example.com".into(),
                protocol: "openai-chat".into(),
                models: vec![],
            });
        }
        let rows = service.usage(false).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].windows[0].used_percent, 42.0);
        assert_eq!(rows[0].provider_name, "Renamed");
        assert_eq!(rows[0].updated_at, observed);
    }
    #[tokio::test]
    async fn credential_endpoint_protocol_and_account_changes_change_identity() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path());
        service.providers.lock().await.push(Provider {
            credit_grants: None,
            model_windows: Default::default(),
            id: "opencode-go".into(),
            name: "Go".into(),
            base_url: "https://opencode.ai/inference/go".into(),
            protocol: "openai-chat".into(),
            models: vec![],
        });
        let initial = service.usage_snapshot().await.identity;
        service.credentials.put(&service.scope, "opencode-go", "test-key").unwrap();
        let keyed = service.usage_snapshot().await.identity;
        assert!(initial != keyed);
        service.providers.lock().await[0].protocol = "other".into();
        assert!(keyed != service.usage_snapshot().await.identity);
        service.providers.lock().await[0].protocol = "openai-chat".into();
        service.providers.lock().await[0].base_url = "https://example.com".into();
        assert!(keyed != service.usage_snapshot().await.identity);
        service.providers.lock().await[0].base_url = "https://opencode.ai/inference/go/".into();
        assert!(keyed == service.usage_snapshot().await.identity);
        service.codex.sign_out().await.unwrap();
        assert!(keyed != service.usage_snapshot().await.identity);
    }
    #[tokio::test]
    #[ignore = "reads existing local subscription usage without generating messages"]
    async fn live_usage_probe() {
        let directory = std::env::var("MINERVA_CODEX_VERIFY_CONFIG_DIR").expect("config directory required");
        let service = ProviderService::new(std::path::Path::new(&directory).join("providers.json")).unwrap();
        let signed_in = service.codex.has_session().unwrap();
        let rows = service.usage(true).await;
        for row in &rows {
            println!("{}: {} ({:?}), {} windows", row.provider_id, row.status, row.error_code, row.windows.len());
        }
        if signed_in {
            assert_eq!(rows.iter().find(|r| r.provider_id == codex::ID).unwrap().status, "ok");
        }
    }
    #[tokio::test]
    async fn custom_credit_grants_is_opt_in_and_uses_the_configured_path_and_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let dir = tempfile::tempdir().unwrap();
        let service = service(dir.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/tenant/v1", listener.local_addr().unwrap());
        let mut input = serde_json::json!({
            "id":"custom", "name":"Custom", "baseUrl":base, "protocol":"openai-chat",
            "models":[], "apiKey":"fixture", "isNew":true
        });
        service.save(serde_json::from_value(input.clone()).unwrap()).await.unwrap();
        assert!(service.usage(false).await.iter().all(|row| row.provider_id != "custom"));

        input["isNew"] = false.into();
        input["apiKey"] = serde_json::Value::Null;
        input["creditGrants"] = true.into();
        service.save(serde_json::from_value(input.clone()).unwrap()).await.unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut req = vec![0; 4096];
            let size = socket.read(&mut req).await.unwrap();
            let req = String::from_utf8_lossy(&req[..size]).to_ascii_lowercase();
            assert!(req.starts_with("get /tenant/v1/dashboard/billing/credit_grants "));
            assert!(req.contains("authorization: bearer fixture"));
            let body = r#"{"total_available":0}"#;
            socket
                .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
                .await
                .unwrap();
        });
        let rows = service.usage(false).await;
        assert!(matches!(rows.iter().find(|r| r.provider_id == "custom").unwrap().balance, Some(Balance::Usd(0.0))));
        server.await.unwrap();
        input["creditGrants"] = false.into();
        service.save(serde_json::from_value(input).unwrap()).await.unwrap();
        assert!(service.usage(false).await.iter().all(|row| row.provider_id != "custom"));
    }

    #[tokio::test]
    async fn hosted_usage_never_reads_local_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = service(dir.path());
        service.endpoint_policy = EndpointPolicy::Hosted;
        assert!(service.usage(true).await.is_empty());
    }
}
