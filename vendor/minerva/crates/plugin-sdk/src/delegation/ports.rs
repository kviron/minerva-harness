use super::*;
use std::{future::Future, pin::Pin, sync::Arc};
pub type ExecutionFuture<'a, T> = Pin<Box<dyn Future<Output = DelegationResult<T>> + Send + 'a>>;

pub trait NativeProcessJournal: Send + Sync {
    fn record(&self, job: &str) -> Result<(), String>;
    fn confirm_stopped(&self, job: &str);
}

/// A one-attempt grant retained by the host while the native executor can write.
pub struct NativeCheckout {
    delegation: String,
    activation: String,
    caller: Caller,
    journal: Option<Arc<dyn NativeProcessJournal>>,
    target: Target,
    ceiling: crate::tools::PermissionPolicy,
    current_policy: Box<dyn Fn() -> crate::tools::PermissionPolicy + Send + Sync>,
}

impl NativeCheckout {
    pub fn new(
        snapshot: &DelegationSnapshot,
        ceiling: crate::tools::PermissionPolicy,
        current_policy: impl Fn() -> crate::tools::PermissionPolicy + Send + Sync + 'static,
    ) -> Self {
        Self {
            delegation: snapshot.id.clone(),
            activation: snapshot.activation_id.clone(),
            caller: snapshot.caller.clone(),
            journal: None,
            target: snapshot.request.target.clone(),
            ceiling,
            current_policy: Box::new(current_policy),
        }
    }

    pub fn with_process_journal(mut self, journal: Option<Arc<dyn NativeProcessJournal>>) -> Self {
        self.journal = journal;
        self
    }

    pub fn record_process(&self, snapshot: &DelegationSnapshot, job: &str) -> Result<(), String> {
        if self.write_policy(snapshot) == crate::tools::PermissionPolicy::ReadOnly {
            return Err("Checkout grant is no longer writable".into());
        }
        self.journal.as_ref().ok_or("Native process journal is unavailable")?.record(job)
    }

    pub fn confirm_process_stopped(&self, job: &str) {
        if let Some(journal) = &self.journal {
            journal.confirm_stopped(job);
        }
    }

    pub fn write_policy(&self, snapshot: &DelegationSnapshot) -> crate::tools::PermissionPolicy {
        if self.delegation == snapshot.id
            && self.activation == snapshot.activation_id
            && self.caller == snapshot.caller
            && self.target == snapshot.request.target
        {
            self.ceiling.restrict((self.current_policy)())
        } else {
            crate::tools::PermissionPolicy::ReadOnly
        }
    }
}

/// A trusted runtime adapter; model JSON never implements this port.
pub trait ExecutionPort: Send + Sync {
    fn capabilities_for(&self, _caller: &Caller, target: &Target) -> ExecutionCapabilities {
        self.capabilities(target)
    }
    fn capabilities(&self, _target: &Target) -> ExecutionCapabilities {
        let unknown = Capability::Unknown { reason: "Adapter has not verified this guarantee".into() };
        ExecutionCapabilities {
            read_only: unknown.clone(),
            interrupt: unknown.clone(),
            continuation: unknown.clone(),
            nested_delegation_control: unknown,
        }
    }
    fn discover<'a>(&'a self, caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>>;
    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle>;
    fn supports_scoped_write(&self, _caller: &Caller, _target: &Target) -> bool {
        false
    }
    fn check_scoped_followup<'a>(&'a self, _child: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn start_with_checkout<'a>(
        &'a self,
        _snapshot: &'a DelegationSnapshot,
        _checkout: NativeCheckout,
    ) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async {
            Err(DelegationError::new(ErrorCode::Unsupported, "Executor cannot accept a delegated checkout"))
        })
    }
    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>>;
    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()>;
}
