//! Embedded Minerva configuration and the workspace of the active IDE project.
use crate::Host;
use ide_command::command;
use ide_core::{IdeError, IdeResult};
use serde_json::Value;

fn error(message: impl std::fmt::Display) -> IdeError {
    IdeError::invalid(message.to_string())
}

#[command(trust = trusted)]
pub async fn minerva_workspace(host: &Host, window: &str) -> IdeResult<String> {
    host.require_trust(window)?;
    let root = host.project(window)?.root.clone();
    let workspace = host.minerva.workspace(&root).await.map_err(error)?;
    serde_json::to_string(&workspace).map_err(error)
}

#[command(trust = trusted)]
pub async fn minerva_call(host: &Host, window: &str, method: String, params: String) -> IdeResult<String> {
    host.require_trust(window)?;
    if params.len() > 2_000_000 {
        return Err(error("Minerva request exceeds 2 MB"));
    }
    let params: Value = serde_json::from_str(&params).map_err(error)?;
    let runtime = host.minerva.runtime().await.map_err(error)?;
    let result = match method.as_str() {
        "usage.list" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Usage {
                #[serde(default)]
                refresh: bool,
            }
            let input: Usage = serde_json::from_value(params).map_err(error)?;
            serde_json::to_value(runtime.plugins.providers().map_err(error)?.usage(input.refresh).await).map_err(error)
        }
        "codex.status" => {
            serde_json::to_value(runtime.plugins.providers().map_err(error)?.codex.status().map_err(error)?)
                .map_err(error)
        }
        "codex.signIn" => {
            serde_json::to_value(runtime.plugins.providers().map_err(error)?.codex.sign_in().map_err(error)?)
                .map_err(error)
        }
        "codex.signOut" => {
            let status = runtime.plugins.providers().map_err(error)?.codex.sign_out().await.map_err(error)?;
            for project in host.projects() {
                project.agents.minerva_models_changed();
            }
            serde_json::to_value(status).map_err(error)
        }
        "codex.models" => {
            let models = runtime.plugins.providers().map_err(error)?.codex.discover_models().await.map_err(error)?;
            for project in host.projects() {
                project.agents.minerva_models_changed();
            }
            serde_json::to_value(models).map_err(error)
        }
        "codex.configure" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Configure {
                models: Vec<String>,
            }
            let input: Configure = serde_json::from_value(params).map_err(error)?;
            let providers =
                runtime.plugins.providers().map_err(error)?.codex_configure(input.models).await.map_err(error)?;
            for project in host.projects() {
                project.agents.minerva_models_changed();
            }
            serde_json::to_value(providers).map_err(error)
        }
        "providers.list" => {
            serde_json::to_value(runtime.plugins.providers().map_err(error)?.list().await).map_err(error)
        }
        "providers.known" => serde_json::to_value(runtime.plugins.providers().map_err(error)?.known()).map_err(error),
        "providers.discover" => {
            let input = serde_json::from_value(params).map_err(error)?;
            let models = runtime.plugins.providers().map_err(error)?.discover(input).await.map_err(error)?;
            serde_json::to_value(models).map_err(error)
        }
        "providers.save" => {
            let input = serde_json::from_value(params).map_err(error)?;
            let providers = runtime.plugins.providers().map_err(error)?.save(input).await.map_err(error)?;
            for project in host.projects() {
                project.agents.minerva_models_changed();
            }
            serde_json::to_value(providers).map_err(error)
        }
        "providers.delete" => {
            let id = params["id"].as_str().ok_or_else(|| error("Provider ID required"))?;
            let providers = runtime.plugins.providers().map_err(error)?.delete(id).await.map_err(error)?;
            for project in host.projects() {
                project.agents.minerva_models_changed();
            }
            serde_json::to_value(providers).map_err(error)
        }
        _ => Err(error("Unsupported Minerva operation")),
    }?;
    serde_json::to_string(&result).map_err(error)
}

ide_command::domain!(minerva: minerva_workspace, minerva_call);
