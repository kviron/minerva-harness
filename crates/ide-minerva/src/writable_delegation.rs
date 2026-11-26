use crate::{NativeRunLease, Service};
use minerva_runtime::{
    delegation::ChildCheckout,
    delegation_contract::{
        ActivationState, Caller, DelegationError, DelegationResult, DelegationSnapshot, ErrorCode, StartRequest,
    },
    tools::{Cancellation, PermissionPolicy},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::OwnedRwLockWriteGuard;

impl Service {
    /// Keep the caller's dispatch barrier alive even if it stops waiting.
    pub async fn run_writable_delegation(
        self: &Arc<Self>,
        caller: Caller,
        request: StartRequest,
        child: Option<String>,
        ceiling: PermissionPolicy,
        barrier: OwnedRwLockWriteGuard<()>,
        cancellation: Cancellation,
    ) -> DelegationResult<DelegationSnapshot> {
        let service = self.clone();
        tokio::spawn(async move {
            service.execute_writable_delegation(caller, request, child.as_deref(), ceiling, barrier, cancellation).await
        })
        .await
        .map_err(|error| DelegationError::new(ErrorCode::OutcomeUnknown, error.to_string()))?
    }

    async fn execute_writable_delegation(
        self: &Arc<Self>,
        caller: Caller,
        request: StartRequest,
        child: Option<&str>,
        ceiling: PermissionPolicy,
        barrier: OwnedRwLockWriteGuard<()>,
        cancellation: Cancellation,
    ) -> DelegationResult<DelegationSnapshot> {
        let run = request.origin_run.clone().ok_or_else(|| forbidden("Writable delegation requires a parent run"))?;
        let runtime = self.runtime().await.map_err(forbidden)?;
        let parent = self
            .native_run_lease(&caller.session_id, &run)
            .ok_or_else(|| forbidden("Parent run has no checkout lease"))?;
        let state = Arc::new(Mutex::new(Some((parent, barrier))));
        let pending = state.clone();
        let service = self.clone();
        let cancellation_at_grant = cancellation.clone();
        let scoped = runtime
            .start_scoped_delegation(
                &caller,
                &request,
                child,
                || !cancellation.is_cancelled(),
                move |snapshot| {
                    let service = service.clone();
                    let pending = pending.clone();
                    let cancellation = cancellation_at_grant.clone();
                    async move {
                        if cancellation.is_cancelled() {
                            return Err(DelegationError::new(
                                ErrorCode::Cancelled,
                                "Parent run was cancelled before checkout transfer",
                            ));
                        }
                        let child = format!("del_{}", snapshot.id);
                        let lease = grant(&service, &snapshot, &child, pending).await?;
                        let child_lease =
                            lease.child_checkout_lease(&child, &snapshot.activation_id).map_err(forbidden)?;
                        Ok(ChildCheckout { ceiling, lease: Arc::new(child_lease) })
                    }
                },
            )
            .await?;

        let mut stopping = false;
        loop {
            let snapshot = tokio::select! {
                biased;
                _ = cancellation.cancelled(), if !stopping => {
                    stopping = true;
                    let _ = tokio::time::timeout(Duration::from_secs(5), scoped.interrupt()).await;
                    continue;
                }
                snapshot = scoped.wait(Duration::from_secs(2)) => snapshot?,
            };
            if snapshot.state == ActivationState::Unknown {
                return Err(DelegationError::new(ErrorCode::OutcomeUnknown, "Child exit could not be confirmed"));
            }
            if snapshot.state.terminal() {
                if let Some(parent) = self.native_run_lease(&caller.session_id, &run)
                    && parent.child_holds_write(&snapshot.activation_id)
                {
                    parent.finish_handoff(&snapshot.activation_id).map_err(forbidden)?;
                }
                return Ok(snapshot);
            }
        }
    }
}

type PendingHandoff = Arc<Mutex<Option<(NativeRunLease, OwnedRwLockWriteGuard<()>)>>>;

async fn grant(
    service: &Service,
    snapshot: &DelegationSnapshot,
    child: &str,
    pending: PendingHandoff,
) -> DelegationResult<NativeRunLease> {
    let run = snapshot.request.origin_run.as_deref().ok_or_else(|| forbidden("Missing parent run"))?;
    let (lease, barrier) = pending
        .lock()
        .map_err(|_| forbidden("Checkout transfer is unavailable"))?
        .take()
        .ok_or_else(|| forbidden("Checkout transfer already used"))?;
    let runtime = service.runtime().await.map_err(forbidden)?;
    let session = &snapshot.caller.session_id;
    if runtime.get_run(session, run).map_err(forbidden)?.status != "running" {
        return Err(forbidden("Parent run is no longer running"));
    }
    let pause = runtime.plugins.jobs().map_err(forbidden)?.try_pause_launches(session).map_err(forbidden)?;
    lease.begin_handoff(child, &snapshot.activation_id, Box::new((barrier, pause))).map_err(forbidden)?;
    Ok(lease)
}

fn forbidden(error: impl Into<String>) -> DelegationError {
    DelegationError::new(ErrorCode::Forbidden, error)
}
