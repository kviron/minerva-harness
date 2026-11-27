//! Account model metadata, independent of the Rig release's bundled snapshot.
use rig_core::{
    catalog::{ModelSpec, ReasoningSupport},
    completion::Effort,
    providers::registry::ProviderId,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Model {
    pub slug: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_context_window: Option<u32>,
    #[serde(default)]
    pub effective_context_window_percent: Option<u32>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub supported_reasoning_levels: Vec<Level>,
    #[serde(default)]
    pub default_reasoning_level: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Level {
    pub effort: String,
}
#[derive(Clone)]
pub(super) struct Models {
    path: PathBuf,
    rows: Arc<Mutex<Option<Vec<Model>>>>,
}
impl Models {
    pub fn new(path: PathBuf) -> Self {
        let rows = std::fs::read(&path)
            .ok()
            .filter(|bytes| bytes.len() <= 1024 * 1024)
            .and_then(|bytes| Self::parse(&bytes).ok());
        Self { path, rows: Arc::new(Mutex::new(rows)) }
    }
    pub fn parse(bytes: &[u8]) -> Result<Vec<Model>, String> {
        #[derive(Deserialize)]
        struct Response {
            models: Vec<Model>,
        }
        if bytes.len() > 1024 * 1024 {
            return Err("Codex model catalog is too large".into());
        }
        let mut rows: Vec<Model> =
            serde_json::from_slice::<Response>(bytes).map_err(|_| "Invalid Codex model catalog")?.models;
        if rows.len() > 2048 {
            return Err("Too many Codex models".into());
        }
        let mut seen = std::collections::HashSet::new();
        for row in &rows {
            if row.slug.is_empty()
                || row.slug.len() > 256
                || row.slug.chars().any(char::is_control)
                || !seen.insert(row.slug.clone())
                || row.supported_reasoning_levels.len() > 32
                || row.supported_reasoning_levels.iter().any(|level| {
                    level.effort.is_empty()
                        || level.effort.len() > 32
                        || !level.effort.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
            {
                return Err("Invalid Codex model metadata".into());
            }
        }
        rows.retain(|row| row.visibility.is_empty() || row.visibility == "list");
        rows.sort_by_key(|row| row.priority);
        Ok(rows)
    }
    pub fn publish(&self, rows: Vec<Model>) -> Result<(), String> {
        let bytes =
            serde_json::to_vec(&serde_json::json!({"models":rows})).map_err(|_| "Invalid Codex model metadata")?;
        let dir = self.path.parent().ok_or("Codex catalog directory unavailable")?;
        std::fs::create_dir_all(dir).map_err(|_| "Cannot save Codex catalog")?;
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|_| "Cannot save Codex catalog")?;
        file.write_all(&bytes).map_err(|_| "Cannot save Codex catalog")?;
        file.persist(&self.path).map_err(|_| "Cannot save Codex catalog")?;
        *self.rows.lock().map_err(|_| "Codex catalog unavailable")? = Some(rows);
        Ok(())
    }
    pub fn clear(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Cannot remove Codex model catalog".into()),
        }
        *self.rows.lock().map_err(|_| "Codex catalog unavailable")? = None;
        Ok(())
    }
    pub fn ids(&self) -> Option<Vec<String>> {
        self.rows.lock().ok()?.as_ref().map(|rows| rows.iter().map(|row| row.slug.clone()).collect())
    }
    pub fn levels(&self, id: &str) -> Option<Vec<String>> {
        let rows = self.rows.lock().ok()?;
        let row = rows.as_ref()?.iter().find(|row| row.slug == id)?;
        Some(row.supported_reasoning_levels.iter().map(|level| level.effort.clone()).collect())
    }
    pub fn snapshot(&self, id: &str) -> Option<(ModelSpec, Vec<String>)> {
        let rows = self.rows.lock().ok()?;
        let row = rows.as_ref()?.iter().find(|row| row.slug == id)?;
        let mut spec =
            ModelSpec::new(ProviderId::catalog("chatgpt")?, id).with_display_name(&row.display_name).with_tools(true);
        spec.context_window =
            row.context_window.filter(|n| *n > 0).or(row.max_context_window).filter(|n| *n > 0).map(|n| {
                ((u64::from(n)
                    * u64::from(row.effective_context_window_percent.filter(|p| (1..=100).contains(p)).unwrap_or(100)))
                    / 100)
                    .max(1) as u32
            });
        spec.input.image = row.input_modalities.iter().any(|value| value == "image");
        spec.reasoning = ReasoningSupport::Listed {
            levels: row
                .supported_reasoning_levels
                .iter()
                .filter_map(|level| serde_json::from_value::<Effort>(serde_json::json!(level.effort)).ok())
                .collect(),
            budget: None,
            can_disable: row
                .supported_reasoning_levels
                .iter()
                .any(|level| matches!(level.effort.as_str(), "none" | "off")),
            default: row
                .default_reasoning_level
                .as_ref()
                .and_then(|level| serde_json::from_value(serde_json::json!(level)).ok()),
        };
        Some((spec, row.supported_reasoning_levels.iter().map(|level| level.effort.clone()).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_models_and_raw_efforts_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let models = Models::new(path.clone());
        let bytes=br#"{"models":[{"slug":"gpt-6.2-astra","visibility":"list","context_window":400000,"input_modalities":["text","image"],"supported_reasoning_levels":[{"effort":"high"},{"effort":"ultra"}]},{"slug":"hidden","visibility":"hide"}]}"#;
        models.publish(Models::parse(bytes).unwrap()).unwrap();
        let restored = Models::new(path);
        assert_eq!(restored.ids().unwrap(), vec!["gpt-6.2-astra"]);
        let (spec, levels) = restored.snapshot("gpt-6.2-astra").unwrap();
        assert_eq!(spec.context_window, Some(400000));
        assert!(spec.input.image);
        assert_eq!(levels, vec!["high", "ultra"]);
    }
    #[test]
    fn effective_capacity_uses_the_advertised_percentage_once() {
        let dir = tempfile::tempdir().unwrap();
        let models = Models::new(dir.path().join("models.json"));
        let bytes = br#"{"models":[{"slug":"fixture","context_window":500000,"max_context_window":872000,"effective_context_window_percent":95}]}"#;
        models.publish(Models::parse(bytes).unwrap()).unwrap();
        assert_eq!(models.snapshot("fixture").unwrap().0.context_window, Some(475000));
    }
    #[test]
    fn duplicate_catalog_is_rejected_without_replacing_cache() {
        assert!(Models::parse(br#"{"models":[{"slug":"gpt-6.2-terra"},{"slug":"gpt-6.2-terra"}]}"#).is_err());
    }
}
