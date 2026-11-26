use crate::{
    sessions::SessionService,
    tools::{AdmittedTools, Cancellation, LivePermissionPolicy, PermissionPolicy, ToolFuture},
};
use minerva_plugin_sdk::delegation::{Caller, StartRequest};
use std::sync::Arc;
use tokio::sync::OwnedRwLockWriteGuard;

/// A host-owned reservation retained by the runtime until the turn settles.
pub trait CheckoutWriteLease: Send + Sync {
    fn may_write(&self) -> bool;

    fn process_journal(&self) -> Option<Arc<dyn minerva_plugin_sdk::delegation::NativeProcessJournal>> {
        None
    }

    fn run_writable_delegation(
        &self,
        _caller: Caller,
        _request: StartRequest,
        _child: Option<String>,
        _policy: PermissionPolicy,
        _barrier: OwnedRwLockWriteGuard<()>,
        _cancel: Cancellation,
    ) -> ToolFuture {
        Box::pin(async { Err("This checkout lease cannot delegate writes".into()) })
    }

    fn authorizes_child(&self, _session: &str, _activation: &str) -> bool {
        false
    }
}

struct InheritedPolicy {
    delegation_tools: Option<AdmittedTools>,
    reader: Arc<dyn LivePermissionPolicy>,
    ancestors: Vec<String>,
    checkout_lease: Option<Arc<dyn CheckoutWriteLease>>,
}

impl LivePermissionPolicy for InheritedPolicy {
    fn run_writable_delegation(
        &self,
        caller: Caller,
        mut request: StartRequest,
        child: Option<String>,
        policy: PermissionPolicy,
        cancel: Cancellation,
    ) -> ToolFuture {
        let dispatch = || {
            let reader = self.checkout_lease.as_ref().ok_or("No host checkout lease")?;
            let tools = self.delegation_tools.as_ref().ok_or("No delegated write tools")?;
            let barrier = tools.try_quiesce_writes().map_err(|_| "Parent tools are still writing")?;
            let writes = tools.delegation_writes();
            if writes.is_empty() {
                return Err("No delegated write tools");
            }
            request.tool_registrations.extend(writes);
            Ok(reader.run_writable_delegation(caller, request, child, policy, barrier, cancel))
        };
        match dispatch() {
            Ok(start) => start,
            Err(error) => Box::pin(async move { Err(error.to_string()) }),
        }
    }

    fn read_policy(&self, session_id: &str) -> Result<PermissionPolicy, String> {
        if self.checkout_lease.as_ref().is_some_and(|lease| !lease.may_write()) {
            return Ok(PermissionPolicy::ReadOnly);
        }
        let own = self.reader.read_policy(session_id)?;
        self.ancestors.iter().try_fold(own, |policy, ancestor| Ok(policy.restrict(self.reader.read_policy(ancestor)?)))
    }
}

pub(super) fn for_session(
    store: &SessionService,
    session: &str,
    reader: Arc<dyn LivePermissionPolicy>,
    checkout_lease: Option<Arc<dyn CheckoutWriteLease>>,
    delegation_tools: AdmittedTools,
) -> Result<Arc<dyn LivePermissionPolicy>, String> {
    let ancestors = store.internal_delegation_ancestors(session).map_err(|error| error.to_string())?;
    if ancestors.is_empty() && checkout_lease.is_none() {
        return Ok(reader);
    }
    Ok(Arc::new(InheritedPolicy { reader, ancestors, checkout_lease, delegation_tools: Some(delegation_tools) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct Policies(Mutex<HashMap<String, PermissionPolicy>>);

    impl LivePermissionPolicy for Policies {
        fn read_policy(&self, session_id: &str) -> Result<PermissionPolicy, String> {
            self.0.lock().unwrap().get(session_id).copied().ok_or_else(|| session_id.to_owned())
        }
    }

    #[test]
    fn descendants_observe_live_policy_and_missing_ancestors_fail_closed() {
        use PermissionPolicy::{Allow, Ask, ReadOnly};
        let reader = Arc::new(Policies(Mutex::new(HashMap::from([
            ("child".into(), Allow),
            ("parent".into(), Ask),
            ("root".into(), Allow),
        ]))));
        let inherited = InheritedPolicy {
            reader: reader.clone(),
            ancestors: vec!["parent".into(), "root".into()],
            checkout_lease: None,
            delegation_tools: None,
        };
        assert_eq!(inherited.read_policy("child"), Ok(Ask));
        reader.0.lock().unwrap().insert("root".into(), ReadOnly);
        assert_eq!(inherited.read_policy("child"), Ok(ReadOnly));
        reader.0.lock().unwrap().remove("parent");
        assert_eq!(inherited.read_policy("child"), Err("parent".into()));
    }
    #[test]
    fn transferred_checkout_lease_restricts_parent_to_reads() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Lease(AtomicBool);
        impl CheckoutWriteLease for Lease {
            fn may_write(&self) -> bool {
                self.0.load(Ordering::Relaxed)
            }
        }
        let reader = Arc::new(Policies(Mutex::new(HashMap::from([("parent".into(), PermissionPolicy::Allow)]))));
        let lease = Arc::new(Lease(AtomicBool::new(true)));
        let policy = InheritedPolicy {
            reader,
            ancestors: Vec::new(),
            checkout_lease: Some(lease.clone()),
            delegation_tools: None,
        };
        assert_eq!(policy.read_policy("parent"), Ok(PermissionPolicy::Allow));
        lease.0.store(false, Ordering::Relaxed);
        assert_eq!(policy.read_policy("parent"), Ok(PermissionPolicy::ReadOnly));
        lease.0.store(true, Ordering::Relaxed);
        assert_eq!(policy.read_policy("parent"), Ok(PermissionPolicy::Allow));
    }
}
