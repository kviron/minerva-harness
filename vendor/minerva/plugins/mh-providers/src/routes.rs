//! A run registers a finite set of routes; endpoint or credential changes revoke them.
use super::{openai_config, Provider, ProviderService};
use rig_core::serve::{adapters::ModelAdapter, ErasedHandler};
use rig_core::{operation::Completion, providers::anthropic::AnthropicConfig, DynModel};
use std::{collections::HashMap, sync::Arc};

pub(crate) struct Route {
    pub label: String,
    pub provider: Arc<Provider>,
    pub model: String,
    credential: RouteCredential,
    pub handler: ErasedHandler,
    pub spec: Option<rig_core::catalog::ModelSpec>,
    pub reasoning_levels: Vec<String>,
}

enum RouteCredential {
    ApiKey(String),
    Codex(u64),
}
pub(crate) type Routes = Arc<HashMap<String, Arc<Route>>>;
type ModelFactory = Box<dyn Fn(&str) -> DynModel<Completion>>;

impl ProviderService {
    pub(crate) async fn run_routes(&self, active_provider: &str) -> Result<Routes, String> {
        let providers = self.providers.lock().await;
        let mut routes = HashMap::new();
        for provider in providers.iter() {
            let provider = Arc::new(provider.clone());
            if provider.protocol == "openai-codex" {
                let signed_in = match self.codex.has_session() {
                    Ok(value) => value,
                    Err(error) if provider.id == active_provider => return Err(error),
                    Err(_) => continue,
                };
                if !signed_in {
                    continue;
                }
                let generation = self.codex.auth_generation();
                for model in &provider.models {
                    let Some((spec, reasoning_levels)) = self.codex.model_snapshot(model) else { continue };
                    let handler = match self.codex.handler_with_spec(spec.clone()) {
                        Ok(handler) => handler,
                        Err(_) => continue,
                    };
                    let label = format!("api:{}/{}", provider.id, model);
                    routes.insert(
                        label.clone(),
                        Arc::new(Route {
                            label,
                            provider: provider.clone(),
                            model: model.clone(),
                            credential: RouteCredential::Codex(generation),
                            handler: ErasedHandler::new(handler),
                            spec: Some(spec),
                            reasoning_levels,
                        }),
                    );
                }
                continue;
            }
            if self.endpoint_policy.require_safe_transport(&provider.base_url).is_err() {
                continue;
            }
            let key = match self.route_key(&provider) {
                Ok(Some(key)) => key,
                Err(error) if provider.id == active_provider => return Err(error),
                _ => continue,
            };
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
                        handler: ErasedHandler::new(ModelAdapter::new(label.clone(), driver)),
                        label,
                        provider: provider.clone(),
                        model: model.clone(),
                        credential: RouteCredential::ApiKey(key.clone()),
                        spec: self.model_spec(&provider, model),
                        reasoning_levels: Vec::new(),
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
        match &route.credential {
            RouteCredential::Codex(generation) => {
                if self.codex.auth_generation() != *generation || !self.codex.has_session()? {
                    return Err("Codex account changed during this run; start a new turn".into());
                }
            }
            RouteCredential::ApiKey(key) => {
                self.endpoint_policy.require_safe_transport(&current.base_url)?;
                if self.route_key(current)?.as_deref() != Some(key.as_str()) {
                    return Err("The selected route credentials changed during this run; start a new turn".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credential_store::{CredentialScope, CredentialStore, MemoryCredentialStore},
        network_policy::EndpointPolicy,
        ProviderInput,
    };

    #[tokio::test]
    async fn run_admits_other_providers_and_revokes_a_changed_credential() {
        let dir = tempfile::tempdir().unwrap();
        let scope = CredentialScope::new("route-test").unwrap();
        let store = Arc::new(MemoryCredentialStore::default());
        let service = ProviderService::new_with_store(
            dir.path().join("providers.json"),
            scope.clone(),
            store.clone(),
            EndpointPolicy::Local,
        )
        .unwrap();
        for id in ["first", "second", "keyless"] {
            service
                .save(ProviderInput {
                    id: id.into(),
                    name: id.into(),
                    base_url: "http://127.0.0.1:12345/v1".into(),
                    models: vec!["model".into()],
                    api_key: (id != "keyless").then(|| "test-only-key".into()),
                    is_new: true,
                    protocol: "openai-chat".into(),
                })
                .await
                .unwrap();
        }
        let routes = service.run_routes("first").await.unwrap();
        assert_eq!(routes.len(), 2);
        let second = routes.get("api:second/model").unwrap();
        service.validate_route(second).await.unwrap();
        store.put(&scope, "second", "changed-test-only-key").unwrap();
        assert!(service.validate_route(second).await.unwrap_err().contains("credentials changed"));
        service.validate_route(routes.get("api:first/model").unwrap()).await.unwrap();
    }
}
