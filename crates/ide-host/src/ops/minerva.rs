//! Embedded Minerva configuration and the workspace of the active IDE project.
use crate::Host;
use ide_command::command;
use ide_core::{IdeError, IdeResult};
use serde_json::{Value, json};

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
        "providers.list" => {
            serde_json::to_value(runtime.plugins.providers().map_err(error)?.list().await).map_err(error)
        }
        "providers.known" => serde_json::to_value(runtime.plugins.providers().map_err(error)?.known()).map_err(error),
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
        "plugins.list" => serde_json::to_value(runtime.plugins.list().await.map_err(error)?).map_err(error),
        "plugins.install" => {
            let path = params["path"].as_str().ok_or_else(|| error("Package directory required"))?;
            let id = host.minerva.install_plugin(path.into()).await.map_err(error)?;
            Ok(json!({"id":id,"restartRequired":true}))
        }
        "plugins.setEnabled" => {
            let id = params["id"].as_str().ok_or_else(|| error("Plugin ID required"))?;
            let enabled = params["enabled"].as_bool().ok_or_else(|| error("Enabled state required"))?;
            let plugins = runtime.plugins.set_enabled(id, enabled).await.map_err(error)?;
            for project in host.projects() {
                project.agents.minerva_models_changed();
            }
            serde_json::to_value(plugins).map_err(error)
        }
        "plugins.call" => {
            let id = params["id"].as_str().ok_or_else(|| error("Plugin ID required"))?;
            let method = params["method"].as_str().ok_or_else(|| error("Plugin method required"))?;
            let result = runtime.plugins.plugin_call(id, method, params["params"].clone()).await.map_err(error)?;
            // Existing model-engine packages change their catalog through these settings calls.
            if matches!(method, "settings.save" | "account.refresh") {
                for project in host.projects() {
                    project.agents.minerva_models_changed();
                }
            }
            Ok(result)
        }
        "plugins.ui" => {
            let id = params["id"].as_str().ok_or_else(|| error("Plugin ID required"))?;
            Ok(json!(runtime.plugins.ui_document(id).await.map_err(error)?))
        }
        _ => Err(error("Unsupported Minerva operation")),
    }?;
    serde_json::to_string(&result).map_err(error)
}

ide_command::domain!(minerva: minerva_workspace, minerva_call);
