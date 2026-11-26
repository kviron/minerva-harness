use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Caller {
    pub session_id: String,
    pub root_session_id: String,
    pub workspace_id: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum Target {
    Minerva { route: String, effort: Option<String> },
    IdeAgent { agent_id: String, options: BTreeMap<String, String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FallbackRule {
    pub primary: Target,
    pub alternative: Target,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FallbackSelection {
    pub requested_target: Target,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_registrations: BTreeMap<String, u64>,
    pub target: Target,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_run: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationState {
    Accepted,
    Starting,
    Running,
    Stopping,
    Unknown,
    Succeeded,
    Failed,
    Cancelled,
}
impl ActivationState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionHandle {
    pub session_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Completion {
    pub result_id: String,
    pub state: ActivationState,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DelegationSnapshot {
    pub id: String,
    pub work_item_id: String,
    pub activation_id: String,
    pub caller: Caller,
    pub request: StartRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackSelection>,
    pub depth: u32,
    pub state: ActivationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_handle: Option<ExecutionHandle>,
    pub handle: Option<ExecutionHandle>,
    pub completion: Option<Completion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub target: Target,
    pub read_only: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "support", rename_all = "kebab-case")]
pub enum Capability {
    Supported { scope: String },
    Unsupported { reason: String },
    Unknown { reason: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionCapabilities {
    pub read_only: Capability,
    pub interrupt: Capability,
    pub continuation: Capability,
    pub nested_delegation_control: Capability,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    Cancelled,
    Invalid,
    Forbidden,
    Unsupported,
    Unavailable,
    Busy,
    Conflict,
    OutcomeUnknown,
    StorageFailed,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DelegationError {
    pub code: ErrorCode,
    pub message: String,
}
impl DelegationError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}
impl std::fmt::Display for DelegationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for DelegationError {}
pub type DelegationResult<T> = Result<T, DelegationError>;
