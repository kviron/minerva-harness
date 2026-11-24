//! A run registers a finite set of routes; endpoint or credential changes revoke them.
use super::{openai_config, Provider, ProviderService};
use rig_core::{operation::Completion, providers::anthropic::AnthropicConfig, DynModel};
use std::{collections::HashMap, sync::Arc};

pub(crate) struct Route {
    pub label: String,
    pub provider: Arc<Provider>,
    pub model: String,
    key: String,
    pub driver: DynModel<Completion>,
}
pub(crate) type Routes = Arc<HashMap<String, Arc<Route>>>;
type ModelFactory = Box<dyn Fn(&str) -> DynModel<Completion>>;

impl ProviderService {
    pub(crate) async fn run_routes(&self, active_provider: &str) -> Result<Routes, String> {
        let providers = self.providers.lock().await;
        let mut routes = HashMap::new();
        for provider in providers.iter().filter(|provider| provider.id == active_provider) {
            let provider = Arc::new(provider.clone());
            if self.endpoint_policy.require_safe_transport(&provider.base_url).is_err() {
                continue;
            }
            let Some(key) = self.route_key(&provider)? else { continue };
            let connect: ModelFactory = match provider.protocol.as_str() {
                "openai-chat" | "ollama" => {
                    let client = openai_config(&provider.id, &provider.base_url, key.clone()).client();
                    Box::new(move |model| client.chat(model).into())
                }
                "openai-responses" => {
                    let client = openai_config(&provider.id, &provider.base_url, key.clone()).client();
                    Box::new(move |model| client.responses(model).into())
                }
                "anthropic-messages" => {
                    let client = AnthropicConfig::new(key.clone()).with_base_url(&provider.base_url).client();
                    Box::new(move |model| client.completion(model).into())
                }
                _ => continue,
            };
            for model in &provider.models {
                let driver = connect(model);
                let label = format!("api:{}/{}", provider.id, model);
                routes.insert(
                    label.clone(),
                    Arc::new(Route {
                        label,
                        provider: provider.clone(),
                        model: model.clone(),
                        key: key.clone(),
                        driver,
                    }),
                );
            }
        }
        Ok(Arc::new(routes))
    }

    fn route_key(&self, provider: &Provider) -> Result<Option<String>, String> {
        Ok(self
            .credentials
            .get(&self.scope, &provider.id)?
            .or_else(|| self.env_key(&provider.id, &provider.base_url))
            .or_else(|| (provider.protocol == "ollama").then(|| "ollama".into())))
    }

    pub(crate) async fn validate_route(&self, route: &Route) -> Result<(), String> {
        let providers = self.providers.lock().await;
        let current = providers
            .iter()
            .find(|provider| provider.id == route.provider.id)
            .ok_or("The selected provider was removed")?;
        if current.base_url != route.provider.base_url
            || current.protocol != route.provider.protocol
            || !current.models.contains(&route.model)
        {
            return Err("The selected route changed during this run; start a new turn".into());
        }
        self.endpoint_policy.require_safe_transport(&current.base_url)?;
        if self.route_key(current)?.as_deref() != Some(route.key.as_str()) {
            return Err("The selected route credentials changed during this run; start a new turn".into());
        }
        Ok(())
    }
}
