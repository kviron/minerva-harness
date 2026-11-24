//! Projection and validated updates of the session-owned desired model selection.
use super::*;

impl Engine {
    pub(super) fn options(
        models: Vec<Value>,
        levels: &BTreeMap<String, Vec<Value>>,
        current: Option<&str>,
        saved_effort: &Value,
    ) -> Vec<Value> {
        let selected = current
            .map(str::to_owned)
            .or_else(|| models.first().and_then(|v| v["value"].as_str()).map(str::to_owned))
            .unwrap_or_default();
        let mut options = vec![
            json!({"id":"model","name":"Model","category":"model","type":"select","currentValue":selected,"options":models}),
            json!({"id":"mode","name":"Permissions","category":"mode","type":"select","currentValue":"ask","options":[
                {"value":"read-only","name":"Read only"},{"value":"ask","name":"Ask before changes"},{"value":"allow","name":"Allow changes"}
            ]}),
        ];
        if let Some(level) = levels.get(&selected).and_then(|levels| levels.first()) {
            let mut level = level.clone();
            if saved_effort[0] == selected && has_choice(&level, &saved_effort[1]) {
                level["currentValue"] = saved_effort[1].clone();
            }
            options.push(level);
        }
        options
    }

    pub(super) async fn session_options(&self, runtime: &Arc<Runtime>, id: &str) -> Result<Vec<Value>, String> {
        let session = runtime.sessions.get(id)?;
        let selection = runtime.sessions.selection(id)?;
        let (models, levels) = self.catalog(runtime).await?;
        let settings = runtime.harness_call("workspace.get", json!({"sessionId":id})).await?;
        let selected = selection.as_ref().map(|value| value.model.as_str()).or(session.model.as_deref());
        let effort = selection
            .as_ref()
            .map(|value| json!([value.model, value.reasoning_effort.as_deref().unwrap_or("default")]))
            .unwrap_or_else(|| settings["modelEffort"].clone());
        // A missing route stays visible until the user explicitly chooses its replacement.
        let mut options = Self::options(models, &levels, Some(selected.unwrap_or_default()), &effort);
        options[1]["currentValue"] = settings["permissionPolicy"].clone();
        Ok(options)
    }

    pub(super) async fn select_model(&self, runtime: &Arc<Runtime>, id: &str, model: &str) -> Result<(), String> {
        let (models, _) = self.catalog(runtime).await?;
        if !models.iter().any(|value| value["value"] == model) {
            return Err("Unknown API model".into());
        }
        let active = self.runs.lock();
        if active
            .get(id)
            .is_some_and(|run| model.split_once('/').map(|(provider, _)| provider) != Some(run.provider.as_str()))
        {
            return Err("Wait for the current turn before changing providers".into());
        }
        runtime.sessions.set_model(id, model)?;
        Ok(())
    }

    pub(super) async fn select_effort(&self, runtime: &Arc<Runtime>, id: &str, value: &str) -> Result<(), String> {
        let selection = runtime.sessions.selection(id)?;
        let options = self.session_options(runtime, id).await?;
        let effort = options.iter().find(|option| option["id"] == "reasoning_effort");
        if !effort.is_some_and(|option| has_choice(option, &json!(value))) {
            return Err("Unsupported reasoning effort for this model".into());
        }
        let model = options[0]["currentValue"].as_str().ok_or("No model selected")?;
        if selection.as_ref().is_some_and(|selected| selected.model != model) {
            return Err("Model changed while choosing reasoning effort".into());
        }
        runtime.sessions.request_selection_if_revision(
            id,
            selection.as_ref().map_or(0, |selected| selected.revision),
            model,
            Some(value),
        )?;
        Ok(())
    }

    pub(super) async fn prompt_selection(
        &self,
        runtime: &Arc<Runtime>,
        id: &str,
    ) -> Result<(String, Option<Value>), String> {
        let config = self.session_options(runtime, id).await?;
        let model = config[0]["currentValue"].as_str().unwrap_or_default().to_owned();
        if model.is_empty() {
            return Err("Configure an API model in Minerva settings first".into());
        }
        if !has_choice(&config[0], &json!(model)) {
            return Err("Select an available API model for this connection".into());
        }
        let effort = config
            .iter()
            .find(|option| option["id"] == "reasoning_effort")
            .map(|option| option["currentValue"].clone());
        Ok((model, effort))
    }
}
