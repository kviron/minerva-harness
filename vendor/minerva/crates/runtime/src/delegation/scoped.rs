use super::{routing::ScopedExecutor, ChildCheckout};
use crate::Runtime;
use minerva_delegation::{
    Caller, DelegationError, DelegationResult, DelegationService, DelegationSnapshot, ErrorCode, ExecutionPort,
    StartRequest,
};
use std::{future::Future, sync::Arc, time::Duration};

pub struct ScopedDelegation {
    pub admitted: DelegationSnapshot,
    service: Arc<DelegationService>,
    executor: Arc<dyn ExecutionPort>,
    sessions: Arc<crate::sessions::SessionService>,
}

impl ScopedDelegation {
    pub async fn interrupt(&self) -> DelegationResult<()> {
        let Some(snapshot) = self.sessions.request_stop_delegation_activation(
            &self.admitted.caller,
            &self.admitted.id,
            &self.admitted.activation_id,
        )?
        else {
            return Ok(());
        };
        self.executor.interrupt(&snapshot).await
    }

    pub async fn wait(&self, timeout: Duration) -> DelegationResult<DelegationSnapshot> {
        let current = self.service.wait(&self.admitted.caller, &self.admitted.id, timeout).await?;
        if current.activation_id == self.admitted.activation_id {
            return Ok(current);
        }
        self.sessions.settled_delegation_activation(
            &self.admitted.caller,
            &self.admitted.id,
            &self.admitted.activation_id,
        )
    }
}

impl Runtime {
    /// Host entry for a write grant issued at the parent step boundary.
    pub async fn start_scoped_delegation<F, Fut>(
        &self,
        caller: &Caller,
        request: &StartRequest,
        child: Option<&str>,
        admission_allowed: impl Fn() -> bool + Send + Sync,
        checkout: F,
    ) -> DelegationResult<ScopedDelegation>
    where
        F: Fn(DelegationSnapshot) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DelegationResult<ChildCheckout>> + Send + 'static,
    {
        let (service, internal, host) = {
            let delegation = self.delegation.lock().await;
            let delegation = delegation
                .as_ref()
                .ok_or_else(|| DelegationError::new(ErrorCode::Unavailable, "Delegation is unavailable"))?;
            (delegation.service.clone(), delegation.internal.clone(), delegation.host.clone())
        };
        let executor = Arc::new(ScopedExecutor { internal, host, checkout });
        let admitted = match child {
            Some(id) => service.followup_scoped(caller, id, request, admission_allowed, executor.clone()).await?,
            None => service.start_scoped(caller, request, admission_allowed, executor.clone()).await?,
        };
        Ok(ScopedDelegation { admitted, service, executor, sessions: self.sessions.clone() })
    }
}
