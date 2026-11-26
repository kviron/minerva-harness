use minerva_runtime::delegation_contract::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

#[derive(Default)]
pub(super) struct ProjectExecutors(RwLock<BTreeMap<PathBuf, Arc<dyn ExecutionPort>>>);

impl ProjectExecutors {
    pub(super) fn insert(&self, root: &Path, executor: Arc<dyn ExecutionPort>) -> std::io::Result<()> {
        let root = root.canonicalize()?;
        self.0.write().unwrap_or_else(|e| e.into_inner()).insert(root, executor);
        Ok(())
    }

    fn select(&self, caller: &Caller) -> DelegationResult<Arc<dyn ExecutionPort>> {
        let root = Path::new(&caller.cwd).canonicalize().map_err(|_| unavailable())?;
        self.0.read().unwrap_or_else(|e| e.into_inner()).get(&root).cloned().ok_or_else(unavailable)
    }
}

fn unavailable() -> DelegationError {
    DelegationError::new(ErrorCode::Unavailable, "No OpenIDEA executor for this project")
}

impl ExecutionPort for ProjectExecutors {
    fn capabilities_for(&self, caller: &Caller, target: &Target) -> ExecutionCapabilities {
        self.select(caller)
            .map(|executor| executor.capabilities_for(caller, target))
            .unwrap_or_else(|_| self.capabilities(target))
    }

    fn discover<'a>(&'a self, caller: &'a Caller) -> ExecutionFuture<'a, Vec<Candidate>> {
        Box::pin(async move { self.select(caller)?.discover(caller).await })
    }

    fn start<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move { self.select(&snapshot.caller)?.start(snapshot).await })
    }

    fn supports_scoped_write(&self, caller: &Caller, target: &Target) -> bool {
        self.select(caller).is_ok_and(|executor| executor.supports_scoped_write(caller, target))
    }

    fn start_with_checkout<'a>(
        &'a self,
        snapshot: &'a DelegationSnapshot,
        checkout: NativeCheckout,
    ) -> ExecutionFuture<'a, ExecutionHandle> {
        Box::pin(async move { self.select(&snapshot.caller)?.start_with_checkout(snapshot, checkout).await })
    }

    fn observe<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, Option<Completion>> {
        Box::pin(async move {
            let executor = self
                .select(&snapshot.caller)
                .map_err(|error| DelegationError::new(ErrorCode::OutcomeUnknown, error.message))?;
            match executor.observe(snapshot).await {
                Err(error) if error.code == ErrorCode::Unavailable => {
                    Err(DelegationError::new(ErrorCode::OutcomeUnknown, error.message))
                }
                result => result,
            }
        })
    }

    fn interrupt<'a>(&'a self, snapshot: &'a DelegationSnapshot) -> ExecutionFuture<'a, ()> {
        Box::pin(async move { self.select(&snapshot.caller)?.interrupt(snapshot).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_runtime_routes_only_to_the_callers_project() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        let router = ProjectExecutors::default();
        let a: Arc<dyn ExecutionPort> = Arc::new(ProjectExecutors::default());
        let b: Arc<dyn ExecutionPort> = Arc::new(ProjectExecutors::default());
        router.insert(first.path(), a.clone()).unwrap();
        router.insert(second.path(), b.clone()).unwrap();
        let caller = |root: &Path| Caller {
            session_id: "session".into(),
            root_session_id: "session".into(),
            workspace_id: "workspace".into(),
            cwd: root.to_str().unwrap().into(),
        };
        assert!(Arc::ptr_eq(&router.select(&caller(first.path())).unwrap(), &a));
        assert!(Arc::ptr_eq(&router.select(&caller(second.path())).unwrap(), &b));
        assert!(router.select(&caller(foreign.path())).is_err());
    }
}
