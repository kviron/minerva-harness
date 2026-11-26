use crate::{limits::Limits, *};
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedFollowup {
    pub work_item_id: String,
    pub cancelled: bool,
    pub activation_id: Option<String>,
}

pub struct Admission {
    pub snapshot: DelegationSnapshot,
    pub fresh: bool,
}

/// Mutations are atomic. Implementations never invoke execution adapters.
pub trait RepositoryPort: Send + Sync {
    fn lookup_queued(&self, _: &Caller, _: &str, _: &str, _: &str) -> DelegationResult<Option<QueuedFollowup>> {
        Ok(None)
    }
    fn enqueue(&self, _: &Caller, _: &str, _: &StartRequest, _: &Limits) -> DelegationResult<QueuedFollowup> {
        Err(DelegationError::new(ErrorCode::Unsupported, "Follow-up queue is unavailable"))
    }
    fn promote(
        &self,
        _: &Caller,
        _: &str,
        _: &BTreeMap<String, u64>,
        _: &Limits,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        Ok(None)
    }
    fn lookup_operation(&self, caller: &Caller, request: &StartRequest)
        -> DelegationResult<Option<DelegationSnapshot>>;
    fn admit(&self, caller: &Caller, request: &StartRequest, limits: &Limits) -> DelegationResult<Admission>;
    fn admit_fallback(
        &self,
        _: &Caller,
        _: &StartRequest,
        _: &Target,
        _: &str,
        _: &Limits,
    ) -> DelegationResult<Admission> {
        Err(DelegationError::new(ErrorCode::Unsupported, "Fallback admission is unavailable"))
    }
    fn lookup_followup(
        &self,
        caller: &Caller,
        id: &str,
        operation: &str,
        prompt: &str,
    ) -> DelegationResult<Option<DelegationSnapshot>>;
    fn admit_followup(
        &self,
        caller: &Caller,
        id: &str,
        request: &StartRequest,
        limits: &Limits,
    ) -> DelegationResult<Admission>;
    fn lookup_scoped_followup(
        &self,
        _: &Caller,
        _: &str,
        _: &str,
        _: &str,
    ) -> DelegationResult<Option<DelegationSnapshot>> {
        Err(DelegationError::new(ErrorCode::Unsupported, "Scoped follow-up admission is unavailable"))
    }
    fn admit_scoped_followup(&self, _: &Caller, _: &str, _: &StartRequest, _: &Limits) -> DelegationResult<Admission> {
        Err(DelegationError::new(ErrorCode::Unsupported, "Scoped follow-up admission is unavailable"))
    }
    fn get(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot>;
    fn list(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>>;
    fn started(&self, id: &str, activation: &str, handle: &ExecutionHandle) -> DelegationResult<()>;
    fn settle(&self, id: &str, activation: &str, completion: &Completion) -> DelegationResult<()>;
    fn mark_unknown(&self, id: &str, activation: &str, reason: &str) -> DelegationResult<()>;
    fn request_stop(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot>;
    fn request_stop_tree(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>>;
}
