use super::{ActivationState, DelegationError, DelegationResult, ErrorCode};
use serde::{Deserialize, Serialize};

/// Constructed by the trusted delivery repository, never accepted as model instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagerNotice {
    pub version: u32,
    pub origin: NoticeOrigin,
    pub result_id: String,
    pub delegation_id: String,
    pub work_item_id: String,
    pub activation_id: String,
    pub state: ActivationState,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeOrigin {
    Manager,
}

impl ManagerNotice {
    pub fn validate(&self) -> DelegationResult<()> {
        let valid_id = |id: &str| {
            !id.is_empty() && id.len() <= 200 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        };
        if self.version != 1
            || !self.state.terminal()
            || self.summary.len() > 8 * 1024
            || [&self.result_id, &self.delegation_id, &self.work_item_id, &self.activation_id]
                .iter()
                .any(|id| !valid_id(id))
        {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid delegation manager notice"));
        }
        Ok(())
    }
}
