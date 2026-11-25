//! Only subscription identity participates in cache admission; catalog metadata does not.
use super::UsageProvider;
use crate::ProviderService;

#[derive(PartialEq)]
pub(super) struct UsageIdentity {
    generation: u64,
    signed_in: bool,
    sources: Vec<SourceIdentity>,
}

// Credential-bearing identity stays private, in memory, without Debug or Serialize.
#[derive(PartialEq)]
struct SourceIdentity {
    id: String,
    endpoint: String,
    protocol: String,
    key: Option<String>,
}

pub(super) struct GoSource {
    pub id: String,
    pub name: String,
    pub key: Option<String>,
}

pub(super) struct Snapshot {
    pub identity: UsageIdentity,
    pub go: Vec<GoSource>,
}

impl Snapshot {
    pub fn apply_names(&self, rows: &mut [UsageProvider]) {
        for row in rows {
            if let Some(source) = self.go.iter().find(|source| source.id == row.provider_id) {
                row.provider_name.clone_from(&source.name);
            }
        }
    }
}

impl ProviderService {
    pub(super) async fn usage_snapshot(&self) -> Snapshot {
        let providers = self.providers.lock().await.clone();
        let mut sources = Vec::new();
        let mut go = Vec::new();
        for provider in providers {
            let endpoint = provider.base_url.trim_end_matches('/');
            if provider.id != "opencode-go"
                || endpoint != "https://opencode.ai/inference/go"
                || provider.protocol != "openai-chat"
            {
                continue;
            }
            let key = self
                .credentials
                .get(&self.scope, &provider.id)
                .ok()
                .flatten()
                .or_else(|| self.env_key(&provider.id, &provider.base_url));
            sources.push(SourceIdentity {
                id: provider.id.clone(),
                endpoint: endpoint.into(),
                protocol: provider.protocol,
                key: key.clone(),
            });
            go.push(GoSource { id: provider.id, name: provider.name, key });
        }
        Snapshot {
            identity: UsageIdentity {
                generation: self.codex.auth_generation(),
                signed_in: self.codex.has_session().unwrap_or(false),
                sources,
            },
            go,
        }
    }
}
