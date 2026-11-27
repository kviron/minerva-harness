use super::{parse, transport, Balance, Reading, UsageProvider};
use crate::{codex, route_preset, Provider, ProviderService};
use serde_json::Value;

pub(super) enum Adapter {
    Codex,
    OpenCodeGo,
    AnyModel,
    TokenHarbor,
    CreditGrants(String),
}
impl Adapter {
    pub fn for_provider(provider: &Provider) -> Option<Self> {
        match route_preset(provider).map(|p| p.id) {
            Some("opencode-go") => Some(Self::OpenCodeGo),
            Some("anymodel") => Some(Self::AnyModel),
            Some("tokenharbor") => Some(Self::TokenHarbor),
            _ if provider.credit_grants == Some(true)
                && matches!(provider.protocol.as_str(), "openai-chat" | "openai-responses") =>
            {
                Some(Self::CreditGrants(format!(
                    "{}/dashboard/billing/credit_grants",
                    provider.base_url.trim_end_matches('/')
                )))
            }
            _ => None,
        }
    }

    async fn query(&self, key: &str) -> Result<Reading, &'static str> {
        let url = match self {
            Self::OpenCodeGo => "https://opencode.ai/zen/go/v1/usage",
            Self::AnyModel => "https://anymodel.org/v1/balance",
            Self::TokenHarbor => "https://tokenharbor.ai/api/cli/free-tier",
            Self::CreditGrants(url) => url,
            Self::Codex => unreachable!("Codex uses its managed session"),
        };
        self.read(&transport::get(url, key, None).await?)
    }

    fn read(&self, json: &Value) -> Result<Reading, &'static str> {
        let balance = match self {
            Self::OpenCodeGo => return parse::go(json).map(Into::into),
            Self::TokenHarbor => return parse::token_harbor(json).map(Into::into),
            Self::AnyModel if json["unlimited"].as_bool() == Some(true) => Balance::Unlimited,
            Self::AnyModel => Balance::Tokens(json["balance"].as_u64().ok_or("invalid-response")?),
            Self::CreditGrants(_) => {
                Balance::Usd(json["total_available"].as_f64().filter(|n| n.is_finite()).ok_or("invalid-response")?)
            }
            Self::Codex => unreachable!("Codex uses its managed session"),
        };
        Ok(Reading { balance: Some(balance), ..Reading::default() })
    }
}

pub(super) struct Source {
    pub adapter: Adapter,
    pub id: String,
    pub name: String,
    pub key: Option<String>,
}
impl Source {
    pub fn codex() -> Self {
        Self { adapter: Adapter::Codex, id: codex::ID.into(), name: "Codex".into(), key: None }
    }

    pub async fn fetch(&self, service: &ProviderService) -> UsageProvider {
        let row = UsageProvider::empty(&self.id, &self.name);
        if matches!(self.adapter, Adapter::Codex) {
            return if service.codex.has_session().unwrap_or(false) {
                row.result(service.codex.subscription_usage().await.map(Into::into))
            } else {
                row
            };
        }
        match self.key.as_deref().filter(|k| !k.trim().is_empty()) {
            Some(key) => row.result(self.adapter.query(key).await),
            None => row,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_balance_units_and_distinguishes_zero_from_missing() {
        let grants = Adapter::CreditGrants("https://gateway.example/v1/dashboard/billing/credit_grants".into());
        assert!(matches!(grants.read(&json!({"total_available":0.0})).unwrap().balance, Some(Balance::Usd(0.0))));
        assert!(grants.read(&json!({"total_granted":100})).is_err());
        assert!(matches!(Adapter::AnyModel.read(&json!({"balance":0})).unwrap().balance, Some(Balance::Tokens(0))));
        assert!(matches!(
            Adapter::AnyModel.read(&json!({"unlimited":true})).unwrap().balance,
            Some(Balance::Unlimited)
        ));
        assert!(Adapter::AnyModel.read(&json!({})).is_err());
    }

    #[test]
    fn harbor_only_reports_enabled_allowances() {
        let payload = json!({"on_plan":false,"plan_used_pct":0,"models_enabled":true,"used_pct":20,"reset_at":"2026-11-27T12:00:00Z"});
        let data = Adapter::TokenHarbor.read(&payload).unwrap();
        assert_eq!(data.windows.len(), 1);
        assert_eq!(data.windows[0].id, "free");
        assert_eq!(data.windows[0].used_percent, 20.0);
        assert!(data.windows[0].reset_at.is_some());
        assert!(Adapter::TokenHarbor
            .read(&json!({"on_plan":false,"models_enabled":false}))
            .unwrap()
            .windows
            .is_empty());
    }
}
