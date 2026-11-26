//! Explicit native model preferences validated against the host's picker catalog.
use crate::{AgentConfigChoice, catalogs::CatalogData};
use minerva_plugin_sdk::delegation::{DelegationError, DelegationResult, ErrorCode};
use std::collections::BTreeMap;

pub(super) fn catalog_targets(agent_id: &str, catalog: &CatalogData) -> Vec<minerva_plugin_sdk::delegation::Target> {
    let mut targets = Vec::new();
    for model in catalog
        .config
        .iter()
        .filter(|option| option.category.as_deref() == Some("model"))
        .flat_map(|option| &option.values)
    {
        let base = BTreeMap::from([("model".into(), model.value.clone())]);
        targets.push(minerva_plugin_sdk::delegation::Target::IdeAgent {
            agent_id: agent_id.into(),
            options: base.clone(),
        });
        for effort in catalog
            .thought_levels
            .get(&model.value)
            .into_iter()
            .flatten()
            .filter(|option| option.category.as_deref() == Some("thought_level"))
            .flat_map(|option| &option.values)
        {
            let mut options = base.clone();
            options.insert("effort".into(), effort.value.clone());
            targets.push(minerva_plugin_sdk::delegation::Target::IdeAgent { agent_id: agent_id.into(), options });
        }
    }
    targets
}

pub(super) fn validate(
    options: &BTreeMap<String, String>,
    catalog: &CatalogData,
) -> DelegationResult<Vec<AgentConfigChoice>> {
    let invalid = |message: &str| DelegationError::new(ErrorCode::Invalid, message);
    if options.keys().any(|key| !matches!(key.as_str(), "model" | "effort")) {
        return Err(invalid("Only model and effort native launch options are supported"));
    }
    let mut choices = Vec::new();
    if let Some(model) = options.get("model") {
        let model_option = catalog
            .config
            .iter()
            .find(|option| option.category.as_deref() == Some("model"))
            .ok_or_else(|| invalid("Native model catalog is unavailable"))?;
        if !model_option.values.iter().any(|value| &value.value == model) {
            return Err(invalid("Requested native model is not in the current catalog"));
        }
        choices.push(AgentConfigChoice { id: model_option.id.clone(), value: model.clone() });
        if let Some(effort) = options.get("effort") {
            let level = catalog
                .thought_levels
                .get(model)
                .into_iter()
                .flatten()
                .find(|option| {
                    option.category.as_deref() == Some("thought_level")
                        && option.values.iter().any(|value| &value.value == effort)
                })
                .ok_or_else(|| invalid("Requested effort is not offered for the native model"))?;
            choices.push(AgentConfigChoice { id: level.id.clone(), value: effort.clone() });
        }
    } else if options.contains_key("effort") {
        return Err(invalid("Native effort requires an explicit model"));
    }
    choices.push(AgentConfigChoice { id: "mode".into(), value: "read-only".into() });
    Ok(choices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> CatalogData {
        let parse = |id: &str, category: &str, values: &[&str]| {
            crate::config::options_of(&json!({"configOptions":[{
                "id":id,"name":id,"category":category,"type":"select","currentValue":values[0],
                "options":values.iter().map(|value| json!({"value":value,"name":value})).collect::<Vec<_>>()
            }]}))
        };
        CatalogData {
            config: parse("model", "model", &["fast", "deep"]),
            thought_levels: BTreeMap::from([
                ("fast".into(), parse("reasoning_effort", "thought_level", &["low"])),
                ("deep".into(), parse("reasoning_effort", "thought_level", &["high", "max"])),
            ]),
            refreshed_at_ms: None,
        }
    }

    #[test]
    fn native_preferences_are_exact_model_scoped_and_keep_readonly() {
        let catalog = catalog();
        let choices =
            validate(&BTreeMap::from([("model".into(), "deep".into()), ("effort".into(), "high".into())]), &catalog)
                .unwrap();
        assert_eq!(
            choices.iter().map(|choice| (choice.id.as_str(), choice.value.as_str())).collect::<Vec<_>>(),
            [("model", "deep"), ("reasoning_effort", "high"), ("mode", "read-only")]
        );
        for values in [
            BTreeMap::from([("model".into(), "missing".into())]),
            BTreeMap::from([("model".into(), "fast".into()), ("effort".into(), "high".into())]),
            BTreeMap::from([("effort".into(), "high".into())]),
            BTreeMap::from([("mode".into(), "allow".into())]),
        ] {
            assert!(validate(&values, &catalog).is_err());
        }
        let targets = catalog_targets("codex", &catalog);
        assert_eq!(targets.len(), 5);
        for target in targets {
            let minerva_plugin_sdk::delegation::Target::IdeAgent { options, .. } = target else { panic!() };
            assert!(validate(&options, &catalog).is_ok());
        }
    }
}
