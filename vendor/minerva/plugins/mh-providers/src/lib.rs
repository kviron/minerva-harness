//! Model provider configuration and write-only OS credentials.
pub use mh_context as context;
pub use mh_runtime_support::{credential_store, network_policy};
pub use mh_sessions::{sessions, transcript};
pub use mh_tool_runtime as tools;
mod anymodel;
mod budget;
pub mod codex;
mod execution;
mod followups;
pub mod model_window;
mod reasoning;
mod route_handler;
mod routes;
mod selection;
mod selection_hook;
#[cfg(test)]
mod selection_tests;
mod step_compaction;
mod step_context;
mod timeout;
pub mod usage;
pub use selection::SelectionControl;
#[cfg(test)]
mod reasoning_http_tests;
pub mod rig_history;
#[cfg(test)]
mod route_switch_tests;
use cordis::{Context, Plugin, Service};
use rig_agent::AgentBuilder;
use rig_core::providers::{anthropic::AnthropicConfig, openai::OpenAIConfig};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, fs, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
#[ts(rename = "ProviderConfig")]
pub struct Provider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credit_grants: Option<bool>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub model_windows: std::collections::BTreeMap<String, model_window::ModelWindow>,
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}
fn default_protocol() -> String {
    "openai-chat".into()
}
/// A provider this build knows by name: choosing it fills the endpoint, so only the key is typed.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct KnownProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub protocol: String,
    pub models: Vec<String>,
    /// Environment variable read when no key is stored, if this preset has one.
    pub env_var: Option<String>,
}
struct Preset {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    public_catalog_base: Option<&'static str>,
    protocol: &'static str,
    models: &'static [&'static str],
    env_var: Option<&'static str>,
}
/// Endpoints are rig-core's own dialect bases, and `id` is its dialect name where it has one,
/// so a known endpoint reaches the vendor's routes and headers without extra configuration.
// shortcut: models list a first guess; a vendor's `/models` route (discover) is the real source.
const PRESETS: &[Preset] = &[
    Preset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &["gpt-4.1-mini"],
        env_var: Some("OPENAI_API_KEY"),
    },
    Preset {
        id: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com",
        public_catalog_base: None,
        protocol: "anthropic-messages",
        models: &["claude-sonnet-4-6"],
        env_var: Some("ANTHROPIC_API_KEY"),
    },
    Preset {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &["deepseek-chat"],
        env_var: Some("DEEPSEEK_API_KEY"),
    },
    Preset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("OPENROUTER_API_KEY"),
    },
    Preset {
        id: "tokenharbor",
        name: "Token Harbor",
        base_url: "https://tokenharbor.ai/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &["qwen3.7-max"],
        env_var: Some("TH_API_KEY"),
    },
    Preset {
        id: "anymodel",
        name: "AnyModel",
        base_url: anymodel::BASE_URL,
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &["am/gpt-5.6-terra"],
        env_var: Some("ANYMODEL_API_KEY"),
    },
    Preset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("GROQ_API_KEY"),
    },
    Preset {
        id: "mistral",
        name: "Mistral AI",
        base_url: "https://api.mistral.ai",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("MISTRAL_API_KEY"),
    },
    Preset {
        id: "xai",
        name: "xAI",
        base_url: "https://api.x.ai",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("XAI_API_KEY"),
    },
    Preset {
        id: "together",
        name: "Together AI",
        base_url: "https://api.together.xyz",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("TOGETHER_API_KEY"),
    },
    Preset {
        id: "perplexity",
        name: "Perplexity",
        base_url: "https://api.perplexity.ai",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("PERPLEXITY_API_KEY"),
    },
    Preset {
        id: "moonshot",
        name: "Moonshot (Kimi)",
        base_url: "https://api.moonshot.ai/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("MOONSHOT_API_KEY"),
    },
    Preset {
        id: "zai",
        name: "Z.AI",
        base_url: "https://api.z.ai/api/paas/v4",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("ZAI_API_KEY"),
    },
    Preset {
        id: "zai-coding",
        name: "Z.AI Coding",
        base_url: "https://api.z.ai/api/coding/paas/v4",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("ZAI_API_KEY"),
    },
    Preset {
        id: "minimax",
        name: "MiniMax",
        base_url: "https://api.minimax.io/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("MINIMAX_API_KEY"),
    },
    Preset {
        id: "venice",
        name: "Venice AI",
        base_url: "https://api.venice.ai/api/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("VENICE_API_KEY"),
    },
    Preset {
        id: "hyperbolic",
        name: "Hyperbolic",
        base_url: "https://api.hyperbolic.xyz",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("HYPERBOLIC_API_KEY"),
    },
    Preset {
        id: "cohere",
        name: "Cohere",
        base_url: "https://api.cohere.ai/compatibility/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("COHERE_API_KEY"),
    },
    Preset {
        id: "huggingface",
        name: "Hugging Face",
        base_url: "https://router.huggingface.co",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("HUGGINGFACE_API_KEY"),
    },
    Preset {
        id: "mira",
        name: "Mira",
        base_url: "https://api.mira.network",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("MIRA_API_KEY"),
    },
    Preset {
        id: "doubleword",
        name: "Doubleword",
        base_url: "https://api.doubleword.ai/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("DOUBLEWORD_API_KEY"),
    },
    Preset {
        id: "xiaomimimo",
        name: "Xiaomi MiMo",
        base_url: "https://api.xiaomimimo.com/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: Some("XIAOMI_MIMO_API_KEY"),
    },
    Preset {
        id: "opencode",
        name: "OpenCode Zen",
        base_url: "https://opencode.ai/inference",
        public_catalog_base: Some("https://opencode.ai/inference/v1"),
        protocol: "openai-chat",
        models: &[],
        env_var: Some("OPENCODE_API_KEY"),
    },
    Preset {
        id: "opencode-go",
        name: "OpenCode Go",
        base_url: "https://opencode.ai/inference/go",
        public_catalog_base: Some("https://opencode.ai/inference/go/v1"),
        protocol: "openai-chat",
        models: &[],
        env_var: Some("OPENCODE_API_KEY"),
    },
    Preset {
        id: "ollama",
        name: "Ollama",
        base_url: "http://localhost:11434/v1",
        public_catalog_base: None,
        protocol: "ollama",
        models: &[],
        env_var: None,
    },
    Preset {
        id: "llamacpp",
        name: "llama.cpp",
        base_url: "http://localhost:8080/v1",
        public_catalog_base: None,
        protocol: "openai-chat",
        models: &[],
        env_var: None,
    },
];
fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}
/// The rig dialect for a known endpoint, so vendor-specific routes and session headers apply.
/// A preset whose endpoint the user edited keeps the plain client instead.
fn dialect(_id: &str, base_url: &str) -> Option<&'static rig_core::providers::openai::wire::Dialect> {
    let preset = PRESETS.iter().find(|preset| preset.base_url == base_url.trim_end_matches('/'))?;
    if preset.id == "zai-coding" {
        return Some(&rig_core::providers::openai::wire::ZAI_CODING);
    }
    rig_core::providers::openai::wire::by_name(preset.id)
}
fn openai_config(id: &str, base_url: &str, key: String) -> OpenAIConfig {
    if id == codex::ID {
        return OpenAIConfig::with_key(&rig_core::providers::chatgpt::DIALECT, key);
    }
    match dialect(id, base_url) {
        Some(dialect) => OpenAIConfig::with_key(dialect, key),
        None => OpenAIConfig::new(key),
    }
    .with_base_url(base_url)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct ProviderView {
    #[serde(flatten)]
    provider: Provider,
    has_key: bool,
    /// Per model, the reasoning settings a picker offers (`default`, `off`, effort levels),
    /// as DeepSeek Harness exposes `reasoning.efforts`; absent for models without controls.
    reasoning: std::collections::BTreeMap<String, Vec<String>>,
}
impl ProviderView {
    pub fn provider(&self) -> &Provider {
        &self.provider
    }

    pub fn reasoning(&self, model: &str) -> &[String] {
        self.reasoning.get(model).map(Vec::as_slice).unwrap_or_default()
    }
}
fn route_preset(provider: &Provider) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| {
        preset.base_url == provider.base_url.trim_end_matches('/')
            && (preset.protocol == provider.protocol
                || (provider.protocol == "openai-responses" && preset.protocol == "openai-chat"))
    })
}

fn catalog_spec(provider: &Provider, model: &str) -> Option<&'static rig_core::catalog::ModelSpec> {
    if provider.protocol == codex::ID {
        return rig_core::catalog::Catalog::builtin()
            .resolve(&format!("chatgpt/{model}"))
            .ok()
            .map(|resolved| resolved.spec);
    }
    let preset = route_preset(provider)?;
    let vendor = if preset.id == "zai-coding" { "zai" } else { preset.id };
    rig_core::catalog::Catalog::builtin().resolve(&format!("{vendor}/{model}")).ok().map(|resolved| resolved.spec)
}

fn reasoning_choices(provider: &Provider, model: &str) -> Vec<String> {
    reasoning::choices(provider, model)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(ts_rs::TS)]
pub struct ProviderInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credit_grants: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub model_windows: Option<std::collections::BTreeMap<String, model_window::ModelWindow>>,
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub api_key: Option<String>,
    pub is_new: bool,
    pub protocol: String,
}
use crate::credential_store::{CredentialScope, CredentialStore, LocalKeyring};
use crate::network_policy::EndpointPolicy;
use crate::sessions::Turn;
pub const PREAMBLE: &str = "РўС‹ РїРѕРјРѕС‰РЅРёРє СЂР°Р·СЂР°Р±РѕС‚С‡РёРєР°. РћС‚РІРµС‡Р°Р№ РЅР° СЂСѓСЃСЃРєРѕРј, РµСЃР»Рё РїРѕР»СЊР·РѕРІР°С‚РµР»СЊ РЅРµ РїСЂРѕСЃРёС‚ РёРЅР°С‡Рµ.";

pub struct AgentRunContext {
    pub session_id: String,
    pub run_id: String,
    pub tools: crate::tools::AdmittedTools,
    pub tool_context: crate::tools::ToolContext,
    pub context: Arc<context::ContextService>,
    pub messages: Vec<crate::sessions::SessionMessage>,
    /// A choice name from `ProviderView::reasoning`; `None` keeps the provider default.
    pub reasoning_effort: Option<String>,
    pub selection: Option<Arc<dyn SelectionControl>>,
    pub followups: Option<Arc<dyn FollowupControl>>,
}

pub trait FollowupControl: Send + Sync {
    fn preferred(&self) -> Result<Option<mh_sessions::delegation::PreferredFollowup>, String>;
    fn claim(&self, work_item_id: &str) -> Result<bool, String>;
    fn confirm(&self, work_item_id: &str) -> Result<bool, String>;
}

pub type ProviderRunContext = AgentRunContext;
struct ProviderStepHook {
    compactor: step_compaction::StepCompactor,
    source: ProviderService,
    run: ProviderRunContext,
    schema_tokens: usize,
    model: String,
    window: Option<usize>,
    input_limit: Option<usize>,
    output_limit: Option<u64>,
    required_output: Option<u64>,
    followups: followups::StepFollowups,
    history: rig_history::HistoryHook,
}
impl rig_agent::agent::hook::AgentHook for ProviderStepHook {
    async fn on_completion_call(
        &self,
        _: &rig_agent::agent::hook::HookContext,
        event: rig_agent::agent::hook::CompletionCallEvent<'_>,
    ) -> rig_agent::agent::hook::CompletionCallAction {
        use rig_agent::agent::hook::{CompletionCallAction, RequestPatch};
        if self.run.tool_context.cancellation.is_cancelled() {
            return CompletionCallAction::Stop("Run cancelled".into());
        }
        let _write_barrier = match self.run.tools.try_quiesce_writes() {
            Ok(barrier) => barrier,
            Err(error) => return CompletionCallAction::Stop(error),
        };
        let prepared =
            match self.run.context.prepare_for_model(&self.run.session_id, &self.run.messages, self.window).await {
                Ok(value) => value,
                Err(error) => return CompletionCallAction::Stop(error),
            };
        let preamble = format!("{}\n{}", PREAMBLE, prepared.system_prompt);
        let (history, pending) = match self.followups.prepare(event.history) {
            Ok(value) => value,
            Err(error) => return CompletionCallAction::Stop(error),
        };
        let view = match step_context::project(
            &self.compactor,
            &self.source,
            &self.run,
            &self.model,
            step_context::StepInput {
                history: &history,
                prompt: event.prompt,
                preamble: &preamble,
                tools: self.schema_tokens,
            },
            step_context::StepLimits {
                window: prepared.budget.context_window,
                input_limit: self.input_limit,
                required_output: self.required_output,
                output_limit: self.output_limit,
            },
        )
        .await
        {
            Ok(view) => view,
            Err(error) => return CompletionCallAction::Stop(error),
        };
        if let Err(error) = self.followups.claim(event.history.len(), pending, &self.history) {
            return CompletionCallAction::Stop(error);
        }
        CompletionCallAction::Patch(
            RequestPatch::new().preamble(preamble).history(view.history).max_tokens(view.output),
        )
    }
    async fn on_model_turn_finished(
        &self,
        _: &rig_agent::agent::hook::HookContext,
        event: rig_agent::agent::hook::ModelTurnFinished<'_>,
    ) -> rig_agent::agent::hook::ModelTurnAction {
        use rig_agent::agent::hook::ModelTurnAction;
        match self
            .run
            .context
            .confirm_request(&self.run.session_id, &self.run.run_id, event.usage.input_tokens)
            .and_then(|_| self.followups.confirm())
        {
            Ok(()) => ModelTurnAction::Continue,
            Err(error) => ModelTurnAction::Stop(error),
        }
    }
    async fn on_dispatch(
        &self,
        _: &rig_agent::agent::hook::HookContext,
        event: rig_agent::agent::hook::DispatchEvent<'_>,
    ) -> rig_agent::agent::hook::DispatchAction {
        use rig_agent::agent::hook::DispatchAction;
        if let rig_core::effect::EffectKind::Completion { request, stream } = event.kind {
            let mut projected = (**request).clone();
            projected.chat_history =
                request.chat_history.iter().map(|message| Arc::new(project_message(message))).collect();
            if let Err(error) =
                step_context::finalize(&self.run, &mut projected, self.input_limit, self.required_output)
            {
                return DispatchAction::Deny(rig_core::error::ErrorReport::new(
                    rig_core::error::ErrorKind::Denied,
                    error,
                ));
            }
            // RequestPatch.history excludes the current prompt, which can be a newly returned tool result.
            DispatchAction::patch(rig_core::effect::EffectKind::Completion {
                request: Arc::new(projected),
                stream: *stream,
            })
        } else {
            DispatchAction::proceed()
        }
    }
}

pub(crate) fn project_message(message: &rig_core::message::Message) -> rig_core::message::Message {
    let mut message = message.clone();
    if let rig_core::message::Message::User { content } = &mut message {
        for part in content {
            if let rig_core::message::UserContent::ToolResult(result) = part {
                let output = serde_json::to_string(&result.content).unwrap_or_default();
                if let Some(pruned) = crate::context::prune_text(&output) {
                    result.content = vec![rig_core::message::ToolResultContent::text(pruned)];
                }
            }
        }
    }
    message
}

pub(crate) fn estimate_message(message: &rig_core::message::Message) -> usize {
    let mut message = message.clone();
    let mut images = 0;
    for image in message.images_mut() {
        images += 1;
        image.data = rig_core::message::DocumentSourceKind::Unknown;
        image.native = None;
    }
    serde_json::to_string(&message)
        .map(|value| crate::context::estimate_tokens(&value))
        .unwrap_or(usize::MAX / 2)
        .saturating_add(images * 1024)
}

#[derive(Clone)]
pub struct ProviderService {
    pub codex: codex::Service,
    path: PathBuf,
    providers: Arc<Mutex<Vec<Provider>>>,
    scope: CredentialScope,
    credentials: Arc<dyn CredentialStore>,
    endpoint_policy: EndpointPolicy,
    usage_cache: usage::Cache,
}
impl Service for ProviderService {
    const NAME: &'static str = "studio.model-providers";
}
impl ProviderService {
    fn model_spec(&self, provider: &Provider, model: &str) -> Option<rig_core::catalog::ModelSpec> {
        if provider.protocol == codex::ID {
            self.codex.model_spec(model)
        } else {
            catalog_spec(provider, model).cloned()
        }
    }
    pub(crate) fn declared_window(&self, provider: &Provider, model: &str) -> Option<usize> {
        let catalog = self.model_spec(provider, model).and_then(|spec| spec.context_window);
        provider.model_windows.get(model).copied().unwrap_or_default().resolve(catalog).map(|n| n as usize)
    }

    pub async fn route_window(&self, route: &str) -> Option<usize> {
        let (id, model) = route.strip_prefix("api:")?.split_once('/')?;
        let providers = self.providers.lock().await;
        self.declared_window(providers.iter().find(|p| p.id == id)?, model)
    }

    pub fn new(path: PathBuf) -> Result<Self, String> {
        Self::new_with_store(path, CredentialScope::new("local")?, Arc::new(LocalKeyring), EndpointPolicy::Local)
    }
    pub fn new_with_store(
        path: PathBuf,
        scope: CredentialScope,
        credentials: Arc<dyn CredentialStore>,
        endpoint_policy: EndpointPolicy,
    ) -> Result<Self, String> {
        if endpoint_policy == EndpointPolicy::Hosted && credentials.local_environment_fallback() {
            return Err("Hosted runtime requires scoped credential broker".into());
        }
        let providers = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("РќРµ СѓРґР°Р»РѕСЃСЊ РїСЂРѕС‡РёС‚Р°С‚СЊ РїСЂРѕРІР°Р№РґРµСЂС‹: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        let codex = codex::Service::new(
            path.with_extension("oauth-locks"),
            scope.clone(),
            credentials.clone(),
            endpoint_policy == EndpointPolicy::Local,
        );
        Ok(Self {
            path,
            providers: Arc::new(Mutex::new(providers)),
            scope,
            credentials,
            endpoint_policy,
            codex,
            usage_cache: Default::default(),
        })
    }
    fn env_key(&self, id: &str, endpoint: &str) -> Option<String> {
        if !self.credentials.local_environment_fallback() || self.endpoint_policy == EndpointPolicy::Hosted {
            return None;
        }
        let preset = preset(id)?;
        if endpoint.trim_end_matches('/') != preset.base_url {
            return None;
        }
        std::env::var(preset.env_var?).ok().filter(|s| !s.trim().is_empty())
    }
    fn has_key(&self, p: &Provider) -> bool {
        if p.protocol == codex::ID {
            return self.codex.has_session().unwrap_or(false);
        }
        self.credentials.get(&self.scope, &p.id).ok().flatten().is_some() || self.env_key(&p.id, &p.base_url).is_some()
    }
    /// The endpoints this build knows, so a provider is configured by name and key alone.
    pub fn known(&self) -> Vec<KnownProvider> {
        PRESETS
            .iter()
            .map(|preset| KnownProvider {
                id: preset.id.into(),
                name: preset.name.into(),
                base_url: preset.base_url.into(),
                protocol: preset.protocol.into(),
                models: preset.models.iter().map(|m| (*m).into()).collect(),
                env_var: preset.env_var.map(Into::into),
            })
            .collect()
    }
    pub async fn list(&self) -> Vec<ProviderView> {
        self.providers
            .lock()
            .await
            .iter()
            .cloned()
            .map(|p| ProviderView {
                has_key: self.has_key(&p),
                reasoning: p
                    .models
                    .iter()
                    .filter_map(|m| {
                        let choices = if p.protocol == codex::ID {
                            self.codex
                                .model_snapshot(m)
                                .map(|(spec, levels)| reasoning::codex_choices(&p, m, &spec, &levels))
                                .unwrap_or_default()
                        } else {
                            reasoning_choices(&p, m)
                        };
                        (!choices.is_empty()).then(|| (m.clone(), choices))
                    })
                    .collect(),
                provider: p,
            })
            .collect()
    }
    pub async fn codex_configure(&self, models: Vec<String>) -> Result<Vec<ProviderView>, String> {
        let is_new = !self.providers.lock().await.iter().any(|provider| provider.id == codex::ID);
        self.save(ProviderInput {
            credit_grants: None,
            model_windows: None,
            id: codex::ID.into(),
            name: "Codex subscription".into(),
            base_url: codex::BASE_URL.into(),
            models,
            api_key: None,
            is_new,
            protocol: codex::ID.into(),
        })
        .await
    }
    pub async fn save(&self, input: ProviderInput) -> Result<Vec<ProviderView>, String> {
        if input.protocol == codex::ID || input.id == codex::ID {
            if input.protocol != codex::ID
                || input.id != codex::ID
                || input.base_url != codex::BASE_URL
                || input.api_key.as_ref().is_some_and(|key| !key.is_empty())
            {
                return Err("Codex subscription uses its managed connection settings".into());
            }
            let catalog = self.codex.models();
            let mut seen = std::collections::HashSet::new();
            if input.models.iter().any(|model| !catalog.contains(model) || !seen.insert(model)) {
                return Err("Invalid Codex model catalog".into());
            }
        }
        if input.id.is_empty()
            || input.id.len() > 64
            || !input.id.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            || !input.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err("ID РґРѕР»Р¶РµРЅ РЅР°С‡РёРЅР°С‚СЊСЃСЏ СЃ Р»Р°С‚РёРЅСЃРєРѕР№ Р±СѓРєРІС‹ Рё СЃРѕРґРµСЂР¶Р°С‚СЊ СЃС‚СЂРѕС‡РЅС‹Рµ Р±СѓРєРІС‹, С†РёС„СЂС‹, - РёР»Рё _.".into());
        }
        if !matches!(
            input.protocol.as_str(),
            "openai-chat" | "openai-responses" | "anthropic-messages" | "ollama" | "openai-codex"
        ) {
            return Err("РќРµРёР·РІРµСЃС‚РЅС‹Р№ РїСЂРѕС‚РѕРєРѕР» API.".into());
        }
        if input.name.trim().is_empty() || input.models.iter().any(|m| m.trim().is_empty()) {
            return Err(
                "РЈРєР°Р¶РёС‚Рµ РЅР°Р·РІР°РЅРёРµ РїСЂРѕРІР°Р№РґРµСЂР° Рё РЅРµРїСѓСЃС‚С‹Рµ ID РјРѕРґРµР»РµР№.".into()
            );
        }
        self.endpoint_policy.validate(&input.base_url)?;

        let mut providers = self.providers.lock().await;
        if input.is_new && providers.iter().any(|p| p.id == input.id) {
            return Err("РџСЂРѕРІР°Р№РґРµСЂ СЃ С‚Р°РєРёРј ID СѓР¶Рµ СЃСѓС‰РµСЃС‚РІСѓРµС‚.".into());
        }
        if !input.is_new && !providers.iter().any(|p| p.id == input.id) {
            return Err("РџСЂРѕРІР°Р№РґРµСЂ РЅРµ РЅР°Р№РґРµРЅ; РѕР±РЅРѕРІРёС‚Рµ СЃРїРёСЃРѕРє.".into());
        }
        if let Some(existing) = providers.iter().find(|p| p.id == input.id) {
            if (existing.base_url.trim_end_matches('/') != input.base_url.trim_end_matches('/')
                || existing.protocol != input.protocol)
                && !(existing.protocol == "ollama"
                    && input.protocol == "ollama"
                    && self.credentials.get(&self.scope, &input.id)?.is_none()
                    && self.env_key(&input.id, &existing.base_url).is_none())
                && !input.api_key.as_ref().is_some_and(|key| !key.trim().is_empty())
            {
                return Err(
                    "РџСЂРё СЃРјРµРЅРµ Р°РґСЂРµСЃР° РёР»Рё РїСЂРѕС‚РѕРєРѕР»Р° РІРІРµРґРёС‚Рµ РєР»СЋС‡ СЏРІРЅРѕ.".into(),
                );
            }
        }
        let mut next = providers.clone();
        let credit_grants = input.credit_grants.or_else(|| {
            providers.iter().find(|p| p.id == input.id && same_endpoint(p, &input)).and_then(|p| p.credit_grants)
        });
        let mut model_windows = input.model_windows.unwrap_or_else(|| {
            providers
                .iter()
                .find(|p| {
                    p.id == input.id
                        && p.protocol == input.protocol
                        && p.base_url == input.base_url.trim_end_matches('/')
                })
                .map(|p| p.model_windows.clone())
                .unwrap_or_default()
        });
        model_windows.retain(|model, _| input.models.contains(model));
        if model_windows.values().any(|window| !window.valid()) {
            return Err(
                "Advertised context window must be positive and a manual model limit must be at least 1024".into()
            );
        }
        let provider = Provider {
            credit_grants: credit_grants.filter(|enabled| *enabled),
            model_windows,
            id: input.id.clone(),
            name: input.name.trim().into(),
            base_url: input.base_url.trim_end_matches('/').into(),
            protocol: input.protocol,
            models: input.models.into_iter().map(|x| x.trim().to_string()).collect(),
        };
        if let Some(existing) = next.iter_mut().find(|x| x.id == provider.id) {
            *existing = provider;
        } else {
            next.push(provider);
        }
        let dir = self.path.parent().ok_or("РќРµ Р·Р°РґР°РЅ РєР°С‚Р°Р»РѕРі РЅР°СЃС‚СЂРѕРµРє")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut temp, &next).map_err(|e| e.to_string())?;
        let changed_key = input.api_key.filter(|k| !k.trim().is_empty());
        let old_key = if changed_key.is_some() { self.credentials.get(&self.scope, &input.id)? } else { None };
        if let Some(key) = changed_key.as_ref() {
            if let Err(e) = self.credentials.put(&self.scope, &input.id, key) {
                return Err(format!("РќРµ СѓРґР°Р»РѕСЃСЊ СЃРѕС…СЂР°РЅРёС‚СЊ РєР»СЋС‡: {e}"));
            }
        }
        if let Err(e) = temp.persist(&self.path) {
            if changed_key.is_some() {
                let rollback = if let Some(old) = old_key {
                    self.credentials.put(&self.scope, &input.id, &old)
                } else {
                    self.credentials.delete(&self.scope, &input.id)
                };
                if rollback.is_err() {
                    return Err(format!(
                        "РќРµ СѓРґР°Р»РѕСЃСЊ СЃРѕС…СЂР°РЅРёС‚СЊ РЅР°СЃС‚СЂРѕР№РєРё: {e}; С‚Р°РєР¶Рµ РЅРµ СѓРґР°Р»РѕСЃСЊ РІРѕСЃСЃС‚Р°РЅРѕРІРёС‚СЊ РїСЂРµР¶РЅРёР№ РєР»СЋС‡."
                    ));
                }
            }
            return Err(format!("РќРµ СѓРґР°Р»РѕСЃСЊ СЃРѕС…СЂР°РЅРёС‚СЊ РЅР°СЃС‚СЂРѕР№РєРё: {e}"));
        }
        *providers = next;
        drop(providers);
        Ok(self.list().await)
    }
    pub async fn delete(&self, id: &str) -> Result<Vec<ProviderView>, String> {
        let mut providers = self.providers.lock().await;
        let mut next = providers.clone();
        next.retain(|p| p.id != id);
        let dir = self.path.parent().ok_or("РќРµ Р·Р°РґР°РЅ РєР°С‚Р°Р»РѕРі РЅР°СЃС‚СЂРѕРµРє")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut temp, &next).map_err(|e| e.to_string())?;
        temp.persist(&self.path).map_err(|e| e.to_string())?;
        *providers = next;
        drop(providers);
        self.credentials.delete(&self.scope, id)?;
        Ok(self.list().await)
    }
    pub async fn discover(&self, input: ProviderInput) -> Result<Vec<String>, String> {
        Ok(self.discover_catalog(input).await?.into_iter().map(|row| row.id).collect())
    }
    pub async fn discover_catalog(&self, input: ProviderInput) -> Result<Vec<model_window::CatalogModel>, String> {
        if input.protocol == codex::ID {
            if input.id != codex::ID || input.base_url != codex::BASE_URL {
                return Err("Invalid Codex subscription route".into());
            }
            return Ok(self
                .codex
                .discover_models()
                .await?
                .into_iter()
                .map(|id| model_window::CatalogModel {
                    context_window: self.codex.model_spec(&id).and_then(|spec| spec.context_window),
                    id,
                })
                .collect());
        }
        self.endpoint_policy.require_safe_transport(&input.base_url)?;

        let public_catalog_base = preset(&input.id)
            .filter(|preset| {
                input.base_url.trim_end_matches('/') == preset.base_url && input.protocol == preset.protocol
            })
            .and_then(|preset| preset.public_catalog_base);
        let key =
            {
                let saved = self.providers.lock().await;
                let matches_saved = saved.iter().any(|p| p.id == input.id && same_endpoint(p, &input));
                input
                    .api_key
                    .as_ref()
                    .filter(|k| !k.trim().is_empty())
                    .cloned()
                    .or_else(|| {
                        if matches_saved {
                            self.credentials.get(&self.scope, &input.id).ok().flatten()
                        } else {
                            None
                        }
                    })
                    .or_else(|| self.env_key(&input.id, &input.base_url))
                    .or_else(|| (input.protocol == "ollama").then(|| "ollama".into()))
                    .or_else(|| public_catalog_base.map(|_| String::new()))
                    .ok_or("Р’РІРµРґРёС‚Рµ API-РєР»СЋС‡ РґР»СЏ СѓРєР°Р·Р°РЅРЅРѕРіРѕ Р°РґСЂРµСЃР°.")?
            };
        let request = async {
            if input.id == "anymodel"
                && input.protocol == "openai-chat"
                && input.base_url.trim_end_matches('/') == anymodel::BASE_URL
            {
                return anymodel::discover(&key).await.map_err(|_| {
                    "РќРµ СѓРґР°Р»РѕСЃСЊ РїРѕР»СѓС‡РёС‚СЊ СЃРїРёСЃРѕРє РјРѕРґРµР»РµР№. РџСЂРѕРІРµСЂСЊС‚Рµ URL, РїСЂРѕС‚РѕРєРѕР» Рё РєР»СЋС‡.".to_string()
                });
            }
            match input.protocol.as_str() {
                "openai-chat" | "openai-responses" | "ollama" => {
                    if let Some(base) = public_catalog_base {
                        let authenticated = !key.is_empty();
                        let catalog = |key: String| {
                            OpenAIConfig::new(key)
                                .with_base_url(base)
                                .with_auth(rig_core::providers::openai::wire::Auth::OptionalBearer)
                                .client()
                        };
                        match catalog(key).list_models().await {
                            Err(error)
                                if authenticated
                                    && error
                                        .provider_response()
                                        .and_then(|response| response.status)
                                        .is_some_and(|status| matches!(status.as_u16(), 401 | 403)) =>
                            {
                                catalog(String::new()).list_models().await
                            }
                            result => result,
                        }
                    } else {
                        openai_config(&input.id, &input.base_url, key).client().list_models().await
                    }
                }
                "anthropic-messages" => {
                    AnthropicConfig::new(key).with_base_url(input.base_url).client().list_models().await
                }
                _ => return Err("РќРµРёР·РІРµСЃС‚РЅС‹Р№ РїСЂРѕС‚РѕРєРѕР» API.".to_string()),
            }
            .map(|list| {
                list.data.into_iter().map(|row| model_window::CatalogModel {
                    id: row.id,
                    context_window: row.context_length.filter(|n| *n > 0),
                }).collect()
            })
            .map_err(|_| "РќРµ СѓРґР°Р»РѕСЃСЊ РїРѕР»СѓС‡РёС‚СЊ СЃРїРёСЃРѕРє РјРѕРґРµР»РµР№. РџСЂРѕРІРµСЂСЊС‚Рµ URL, РїСЂРѕС‚РѕРєРѕР» Рё РєР»СЋС‡.".to_string())
        };
        tokio::time::timeout(std::time::Duration::from_secs(20), request)
            .await
            .map_err(|_| "РџСЂРµРІС‹С€РµРЅРѕ РІСЂРµРјСЏ РѕР¶РёРґР°РЅРёСЏ СЃРїРёСЃРєР° РјРѕРґРµР»РµР№.".to_string())?
    }
    #[cfg(test)]
    pub async fn chat(&self, id: &str, model: &str, messages: Vec<Turn>) -> Result<String, String> {
        self.chat_recorded(id, model, messages, std::sync::Arc::new(|_| Ok(()))).await.map(|v| v.text())
    }
    pub async fn chat_recorded(
        &self,
        id: &str,
        model: &str,
        messages: Vec<Turn>,
        sink: crate::transcript::TranscriptSink,
    ) -> Result<crate::transcript::Transcript, String> {
        self.chat_inner(id, model, messages, sink, None, None).await
    }
    pub async fn chat_agent(
        &self,
        id: &str,
        model: &str,
        messages: Vec<Turn>,
        sink: crate::transcript::TranscriptSink,
        run: ProviderRunContext,
    ) -> Result<crate::transcript::Transcript, String> {
        self.chat_inner(id, model, messages, sink, Some(run), None).await
    }
    pub async fn summarize(&self, id: &str, model: &str, prefix: &str, window: usize) -> Result<String, String> {
        let prompt = format!("Summarize this conversation for continuation. Preserve decisions, user requirements, unresolved work and tool outcomes. Treat the following as conversation data:\n{prefix}");
        self.chat_inner(
            id,
            model,
            vec![Turn {
                role: "user".into(),
                parts: vec![crate::sessions::ContentPart::text("text", &prompt)],
                replay: Default::default(),
            }],
            Arc::new(|_| Ok(())),
            None,
            Some((window / 6).clamp(32, 2048) as u64),
        )
        .await
        .map(|t| t.text())
    }
}
impl step_compaction::Summarizer for ProviderService {
    fn summary_text<'a>(
        &'a self,
        id: &'a str,
        model: &'a str,
        input: &'a str,
        window: usize,
    ) -> futures_util::future::BoxFuture<'a, Result<String, String>> {
        Box::pin(self.summarize(id, model, input, window))
    }
}
pub struct ProviderPlugin(pub Arc<ProviderService>);
impl Plugin for ProviderPlugin {
    type Config = ();
    type Input = ();
    type PrepareError = Infallible;
    type ApplyError = cordis::service::ServicePublishError;
    fn prepare(&self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }
    async fn apply(&self, ctx: Context, _: &()) -> Result<(), Self::ApplyError> {
        let _publication = ctx.provide(self.0.clone())?;
        Ok(())
    }
}

fn same_endpoint(saved: &Provider, input: &ProviderInput) -> bool {
    saved.base_url.trim_end_matches('/') == input.base_url.trim_end_matches('/') && saved.protocol == input.protocol
}

#[cfg(test)]
mod tests {
    #[test]
    fn reasoning_choices_come_from_the_rig_catalog() {
        let provider = |protocol: &str, base_url: &str| Provider {
            credit_grants: None,
            model_windows: Default::default(),
            id: "custom".into(),
            name: "Custom".into(),
            base_url: base_url.into(),
            models: Vec::new(),
            protocol: protocol.into(),
        };
        let anthropic = provider("anthropic-messages", "https://api.anthropic.com");
        let choices = reasoning_choices(&anthropic, "claude-sonnet-4-5");
        assert_eq!(choices.first().map(String::as_str), Some("default"));
        assert!(choices.len() > 1, "{choices:?}");
        let deepseek = provider("openai-chat", "https://api.deepseek.com");
        assert!(catalog_spec(&deepseek, "deepseek-v4-pro").is_some());
        assert!(reasoning_choices(&provider("openai-chat", "https://example.test"), "unknown-model").is_empty());
    }
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn cjk_tool_projection_shortens_bytes_and_keeps_original() {
        use rig_core::message::{
            Message, ProviderCallId, ToolCall, ToolFunction, ToolName, ToolResultContent, UserContent,
        };
        let call = ToolCall::new(
            ProviderCallId::new("cjk-call").unwrap().into(),
            ToolFunction::new(ToolName::new("lookup").unwrap(), serde_json::json!({})),
        );
        let result = call.result(vec![ToolResultContent::text("\u{754c}".repeat(3000))]);
        let message = Message::User { content: vec![UserContent::ToolResult(result.clone())] };
        let original = serde_json::to_value(&message).unwrap();
        let projected = project_message(&message);
        let Message::User { content } = projected else { panic!("expected tool result") };
        let UserContent::ToolResult(pruned) = &content[0] else { panic!("expected tool result") };
        let before = serde_json::to_string(&result.content).unwrap();
        let after = serde_json::to_string(&pruned.content).unwrap();
        assert!(before.len() > 8192);
        assert!(after.len() < 8192 && after.len() < before.len());
        assert_eq!(pruned.call, result.call);
        assert_eq!(pruned.name, result.name);
        assert_eq!(serde_json::to_value(&message).unwrap(), original);
    }
    fn input(endpoint: &str, key: Option<&str>, is_new: bool) -> ProviderInput {
        ProviderInput {
            credit_grants: None,
            model_windows: None,
            id: "openai".into(),
            name: "Test".into(),
            base_url: endpoint.into(),
            models: vec!["test".into()],
            api_key: key.map(str::to_owned),
            is_new,
            protocol: "openai-chat".into(),
        }
    }
    #[tokio::test]
    async fn ollama_discovers_without_key_and_guards_saved_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let service = ProviderService::new_with_store(
            dir.path().join("providers.json"),
            CredentialScope::new("test").unwrap(),
            Arc::new(crate::credential_store::MemoryCredentialStore::default()),
            EndpointPolicy::Local,
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /v1/models "));
            let body =
                r#"{"object":"list","data":[{"id":"qwen3:8b","object":"model","created":1,"owned_by":"library"}]}"#;
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let draft = |url: &str, key: Option<&str>, is_new| ProviderInput {
            credit_grants: None,
            model_windows: None,
            id: "ollama".into(),
            protocol: "ollama".into(),
            ..input(url, key, is_new)
        };
        assert_eq!(service.discover(draft(&endpoint, None, true)).await.unwrap(), vec!["qwen3:8b"]);
        server.join().unwrap();
        service.save(draft(&endpoint, None, true)).await.unwrap();
        assert!(!service.list().await[0].has_key);
        let other = "http://127.0.0.1:11435/v1";
        service.save(draft(other, None, false)).await.unwrap();
        service.save(draft(other, Some("proxy-key"), false)).await.unwrap();
        assert!(service.save(draft(&endpoint, None, false)).await.is_err());
    }
    #[tokio::test]
    async fn scoped_provider_crud_and_changed_endpoint_do_not_leak_keys() {
        use crate::credential_store::MemoryCredentialStore;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryCredentialStore::default());
        let a = ProviderService::new_with_store(
            dir.path().join("a.json"),
            CredentialScope::new("a").unwrap(),
            store.clone(),
            EndpointPolicy::Local,
        )
        .unwrap();
        let b = ProviderService::new_with_store(
            dir.path().join("b.json"),
            CredentialScope::new("b").unwrap(),
            store.clone(),
            EndpointPolicy::Local,
        )
        .unwrap();
        a.save(input("https://example.com/v1", Some("alice"), true)).await.unwrap();
        b.save(input("https://example.com/v1", Some("bob"), true)).await.unwrap();
        assert!(a.list().await[0].has_key);
        assert!(b.list().await[0].has_key);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        assert!(a.discover(input(&endpoint, None, false)).await.is_err());
        assert!(a.save(input(&endpoint, None, false)).await.is_err());
        assert!(listener.accept().is_err());
        assert_eq!(store.get(&CredentialScope::new("a").unwrap(), "openai").unwrap().as_deref(), Some("alice"));
        a.delete("openai").await.unwrap();
        assert!(b.list().await[0].has_key);
        assert_eq!(store.get(&CredentialScope::new("b").unwrap(), "openai").unwrap().as_deref(), Some("bob"));
    }
    #[tokio::test]
    async fn hosted_provider_never_touches_ambient_credentials_or_transport() {
        let dir = tempfile::tempdir().unwrap();
        let s = ProviderService::new_with_store(
            dir.path().join("p.json"),
            CredentialScope::new("hosted").unwrap(),
            Arc::new(crate::credential_store::UnavailableCredentialStore),
            EndpointPolicy::Hosted,
        )
        .unwrap();
        assert!(s
            .discover(input("https://api.openai.com/v1", Some("explicit"), true))
            .await
            .unwrap_err()
            .contains("transport unavailable"));
        assert!(ProviderService::new_with_store(
            dir.path().join("q.json"),
            CredentialScope::new("hosted").unwrap(),
            Arc::new(LocalKeyring),
            EndpointPolicy::Hosted
        )
        .is_err());
    }
    #[tokio::test]
    async fn all_protocols_keep_unavailable_tools_as_context_on_their_own_wires() {
        let fixtures = [
            (
                "ollama",
                "/v1/chat/completions",
                serde_json::json!({"id":"r","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"wire reply"},"finish_reason":"stop"}]}),
            ),
            (
                "openai-chat",
                "/v1/chat/completions",
                serde_json::json!({"id":"r","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"wire reply"},"finish_reason":"stop"}]}),
            ),
            (
                "openai-responses",
                "/v1/responses",
                serde_json::json!({"id":"r","object":"response","created_at":0,"status":"completed","model":"test-model","output":[{"type":"message","id":"m","status":"completed","role":"assistant","content":[{"type":"output_text","annotations":[],"text":"wire reply"}]}],"tools":[]}),
            ),
            (
                "anthropic-messages",
                "/v1/messages",
                serde_json::json!({"id":"m","type":"message","model":"test-model","role":"assistant","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"text","text":"wire reply"}]}),
            ),
        ];
        for (protocol, route, body) in fixtures {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let (mut socket, _) = loop {
                    match listener.accept() {
                        Ok(value) => break value,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(10))
                        }
                        Err(error) => panic!("no provider request: {error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = socket.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(pos) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&data[..pos]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|n| n.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= pos + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&data);
                assert!(request.starts_with(&format!("POST {route} ")), "wrong route");
                for text in
                    ["previous question", "previous answer", "current question", "lookup", "observed tool result"]
                {
                    assert!(
                        request.contains(text),
                        "{protocol} missing {text}: {}",
                        request.split("\r\n\r\n").nth(1).unwrap_or_default()
                    );
                }
                let body = body.to_string();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            });
            let dir = tempfile::tempdir().unwrap();
            let service = ProviderService::new_with_store(
                dir.path().join("providers.json"),
                CredentialScope::new("wire-test").unwrap(),
                Arc::new(crate::credential_store::MemoryCredentialStore::default()),
                EndpointPolicy::Local,
            )
            .unwrap();
            let id = format!("wiretest{}{}", std::process::id(), protocol);
            let base_url = if protocol == "anthropic-messages" {
                format!("http://{address}")
            } else {
                format!("http://{address}/v1")
            };
            service
                .save(ProviderInput {
                    credit_grants: None,
                    model_windows: None,
                    id: id.clone(),
                    name: "Test".into(),
                    base_url,
                    models: vec!["test-model".into()],
                    api_key: (protocol != "ollama").then(|| "local-only-test-key".into()),
                    is_new: true,
                    protocol: protocol.into(),
                })
                .await
                .unwrap();
            let answer = service
                .chat(
                    &id,
                    "test-model",
                    vec![
                        crate::sessions::Turn::plain("user", "previous question"),
                        crate::sessions::Turn {
                            role: "assistant".into(),
                            parts: vec![
                                crate::sessions::ContentPart::call(crate::transcript::ToolCall {
                                    call_id: "call_123".into(),
                                    name: "lookup".into(),
                                    arguments: serde_json::json!({"id":1}),
                                }),
                                crate::sessions::ContentPart::result(crate::transcript::ToolResult {
                                    call_id: "call_123".into(),
                                    output: serde_json::json!({"value":"observed tool result"}),
                                    is_error: false,
                                }),
                                crate::sessions::ContentPart::text("text", "previous answer"),
                            ],
                            replay: serde_json::Map::new(),
                        },
                        crate::sessions::Turn::plain("user", "current question"),
                    ],
                )
                .await;
            service.delete(&id).await.unwrap();
            assert_eq!(answer.unwrap(), "wire reply");
            server.join().unwrap();
        }
    }
    #[tokio::test]
    async fn provider_credentials_stay_out_of_config_and_reach_only_local_mock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.json");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 2048];
            loop {
                let n = socket.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(pos) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..pos]);
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if data.len() >= pos + 4 + len {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&data);
            assert!(text.contains("test-model"));
            assert!(text.contains("test-key-only"));
            let body = r#"{"id":"test","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"local reply"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let service = ProviderService::new(path.clone()).unwrap();
        let id = format!("testrig{}", std::process::id());
        service
            .save(ProviderInput {
                credit_grants: None,
                model_windows: None,
                id: id.clone(),
                name: "Test".into(),
                base_url: format!("http://{address}/v1"),
                models: vec!["test-model".into()],
                api_key: Some("test-key-only".into()),
                is_new: true,
                protocol: "openai-chat".into(),
            })
            .await
            .unwrap();
        let stored = fs::read_to_string(path).unwrap();
        assert!(!stored.contains("test-key-only"));
        let answer =
            service.chat(&id, "test-model", vec![crate::sessions::Turn::plain("user", "hello")]).await.unwrap();
        assert_eq!(answer, "local reply");
        server.join().unwrap();
        assert!(service.chat(&id, "unknown", vec![]).await.is_err());
        let changed = || ProviderInput {
            credit_grants: None,
            model_windows: None,
            id: id.clone(),
            name: "Test".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            models: vec!["test-model".into()],
            api_key: None,
            is_new: false,
            protocol: "openai-chat".into(),
        };
        assert!(service.save(changed()).await.err().unwrap().contains("РєР»СЋС‡"));
        assert!(service.discover(changed()).await.unwrap_err().contains("РєР»СЋС‡"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut buf = [0u8; 2048];
            let n = socket.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("/v1/models"));
            let body =
                r#"{"object":"list","data":[{"id":"listed-model","object":"model","created":1,"owned_by":"test"}]}"#;
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let found = service
            .discover(ProviderInput {
                credit_grants: None,
                model_windows: None,
                id: id.clone(),
                name: "Test".into(),
                base_url: format!("http://{address}/v1"),
                models: vec![],
                api_key: Some("test-key-only".into()),
                is_new: false,
                protocol: "openai-chat".into(),
            })
            .await
            .unwrap();
        assert_eq!(found, vec!["listed-model"]);
        server.join().unwrap();
        service.delete(&id).await.unwrap();
    }
}
