//! Provider-neutral workspace, context and image attachment contracts.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSettings {
    pub cwd: Option<String>,
    pub permission_policy: crate::tools::PermissionPolicy,
    pub context_window: usize,
    /// Old profiles stored the default as a number, even when no cap was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context_window_explicit: Option<bool>,
    pub auto_compact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model_effort: Option<(String, String)>,
}
pub const DEFAULT_CONTEXT_WINDOW: usize = 32_768;

impl WorkspaceSettings {
    pub fn effective_window(&self, model_window: Option<usize>) -> usize {
        match model_window.filter(|n| *n > 0) {
            Some(window) if self.context_window_explicit.unwrap_or(self.context_window != DEFAULT_CONTEXT_WINDOW) => {
                window.min(self.context_window)
            }
            Some(window) => window,
            None => self.context_window,
        }
    }
}
impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            cwd: None,
            permission_policy: crate::tools::PermissionPolicy::ReadOnly,
            context_window: DEFAULT_CONTEXT_WINDOW,
            context_window_explicit: None,
            auto_compact: true,
            model_effort: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudget {
    pub context_window: usize,
    pub estimated_tokens: usize,
    pub available_tokens: usize,
    pub should_compact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextSummary {
    pub text: String,
    pub prefix_message_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRef {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub bytes: usize,
}

#[derive(Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentData {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub bytes: usize,
    pub data: String,
}
// Never include image payloads in diagnostic logs.
impl std::fmt::Debug for AttachmentData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentData")
            .field("id", &self.id)
            .field("mime_type", &self.mime_type)
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn legacy_default_adopts_model_capacity_while_explicit_caps_survive() {
        let mut settings: WorkspaceSettings = serde_json::from_value(serde_json::json!({
            "cwd": null, "permissionPolicy": "ask", "contextWindow": 32768, "autoCompact": true
        }))
        .unwrap();
        assert_eq!(settings.effective_window(Some(475000)), 475000);
        assert_eq!(settings.effective_window(None), 32768);
        settings.context_window_explicit = Some(true);
        assert_eq!(settings.effective_window(Some(475000)), 32768);
        settings.context_window_explicit = None;
        settings.context_window = 96000;
        assert_eq!(settings.effective_window(Some(475000)), 96000);
        assert_eq!(settings.effective_window(Some(8192)), 8192);
    }
}
