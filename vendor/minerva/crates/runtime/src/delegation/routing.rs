use minerva_delegation::*;
use std::sync::Arc;

pub(super) struct Executors {
    pub internal: Arc<dyn ExecutionPort>,
    pub host: Option<Arc<dyn ExecutionPort>>,
}

impl Executors {
    fn select(&self, target: &Target) -> DelegationResult<&dyn ExecutionPort> {
        match target {
            Target::Minerva { .. } => Ok(self.internal.as_ref()),
            Target::IdeAgent { .. } => self
                .host
                .as_deref()
                .ok_or_else(|| DelegationError::new(ErrorCode::Unavailable, "OpenIDEA executor is unavailable")),
        }
    }
}

impl ExecutionPort for Executors {
    fn capabilities_for(&self, caller: &Caller, target: &Target) -> ExecutionCapabilities {
        self.select(target).unwrap_or(self.internal.as_ref()).capabilities_for(caller, target)
    }
    fn capabilities(&self, target: &Target) -> ExecutionCapabilities {
        self.select(target).unwrap_or(self.internal.as_ref()).capabilities(target)
    }

    fn discover<'a>(&'a self, caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async move {
            let mut candidates = self.internal.discover(caller).await?;
            if let Some(host) = &self.host {
                match host.discover(caller).await {
                    Ok(external) => candidates.extend(external),
                    Err(error) if error.code == ErrorCode::Unavailable => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(candidates)
        })
    }

    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move { self.select(&snapshot.request.target)?.start(snapshot).await })
    }

    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        Box::pin(async move { self.select(&snapshot.request.target)?.observe(snapshot).await })
    }

    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async move { self.select(&snapshot.request.target)?.interrupt(snapshot).await })
    }
}

pub(super) struct ScopedExecutor<F> {
    pub internal: Arc<super::executor::MinervaExecutor>,
    pub host: Option<Arc<dyn ExecutionPort>>,
    pub checkout: F,
}

impl<F, Fut> ExecutionPort for ScopedExecutor<F>
where
    F: Fn(DelegationSnapshot) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = DelegationResult<super::ChildCheckout>> + Send,
{
    fn discover<'a>(&'a self, caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        self.internal.discover(caller)
    }

    fn supports_scoped_write(&self, caller: &Caller, target: &Target) -> bool {
        match target {
            Target::Minerva { .. } => true,
            Target::IdeAgent { .. } => {
                self.host.as_ref().is_some_and(|host| host.supports_scoped_write(caller, target))
            }
        }
    }

    fn check_scoped_followup<'a>(&'a self, child: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        match (&child.request.target, &self.host) {
            (Target::IdeAgent { .. }, Some(host)) => host.check_scoped_followup(child),
            _ => Box::pin(async { Ok(()) }),
        }
    }

    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move {
            let checkout = (self.checkout)(snapshot.clone()).await?;
            match &snapshot.request.target {
                Target::Minerva { .. } => self.internal.start_scoped(snapshot, Some(checkout)).await,
                Target::IdeAgent { .. } => {
                    let host = self.host.as_ref().ok_or_else(|| {
                        DelegationError::new(ErrorCode::Unavailable, "OpenIDEA executor is unavailable")
                    })?;
                    let session = format!("del_{}", snapshot.id);
                    if !checkout.lease.authorizes_child(&session, &snapshot.activation_id) {
                        return Err(DelegationError::new(ErrorCode::Forbidden, "Grant belongs to another activation"));
                    }
                    let plugins = self.internal.plugins.clone();
                    let caller = snapshot.caller.clone();
                    let journal = checkout.lease.process_journal();
                    let native = NativeCheckout::new(snapshot, checkout.ceiling, move || {
                        use crate::tools::PermissionPolicy;
                        if !checkout.lease.may_write() {
                            return PermissionPolicy::ReadOnly;
                        }
                        plugins
                            .upgrade()
                            .and_then(|plugins| plugins.harness.context.workspace_get(&caller.session_id).ok())
                            .filter(|settings| settings.cwd.as_deref() == Some(caller.cwd.as_str()))
                            .map_or(PermissionPolicy::ReadOnly, |settings| settings.permission_policy)
                    });
                    host.start_with_checkout(snapshot, native.with_process_journal(journal)).await
                }
            }
        })
    }

    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        match (&snapshot.request.target, &self.host) {
            (Target::IdeAgent { .. }, Some(host)) => host.observe(snapshot),
            _ => self.internal.observe(snapshot),
        }
    }

    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        match (&snapshot.request.target, &self.host) {
            (Target::IdeAgent { .. }, Some(host)) => host.interrupt(snapshot),
            _ => self.internal.interrupt(snapshot),
        }
    }
}
