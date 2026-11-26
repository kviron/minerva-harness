use minerva_runtime::delegation_contract::NativeProcessJournal;
use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

type Owner = dyn Any + Send + Sync;

pub struct NativeRunLease {
    owner: Arc<RunOwner>,
}

struct RunOwner {
    service: Weak<crate::Service>,
    _checkout: Arc<Owner>,
    handoff: Mutex<Option<Handoff>>,
    journal: Option<Arc<dyn NativeProcessJournal>>,
}

struct Handoff {
    activation: String,
    child_session: String,
    _pause: Box<dyn Send + Sync>,
}

impl minerva_runtime::run_supervisor::CheckoutWriteLease for NativeRunLease {
    fn run_writable_delegation(
        &self,
        caller: minerva_runtime::delegation_contract::Caller,
        request: minerva_runtime::delegation_contract::StartRequest,
        child: Option<String>,
        policy: minerva_runtime::tools::PermissionPolicy,
        barrier: tokio::sync::OwnedRwLockWriteGuard<()>,
        cancel: minerva_runtime::tools::Cancellation,
    ) -> minerva_runtime::tools::ToolFuture {
        let service = self.owner.service.upgrade();
        Box::pin(async move {
            let service = service.ok_or("OpenIDEA checkout host is unavailable")?;
            let snapshot = service
                .run_writable_delegation(caller, request, child, policy, barrier, cancel)
                .await
                .map_err(|error| error.to_string())?;
            serde_json::to_value(snapshot).map_err(|error| error.to_string())
        })
    }

    fn may_write(&self) -> bool {
        self.parent_holds_write()
    }
}

pub struct ChildCheckoutLease {
    owner: Arc<RunOwner>,
    session: String,
    activation: String,
}

impl minerva_runtime::run_supervisor::CheckoutWriteLease for ChildCheckoutLease {
    fn may_write(&self) -> bool {
        self.owner.handoff.lock().is_ok_and(|handoff| {
            handoff
                .as_ref()
                .is_some_and(|current| current.activation == self.activation && current.child_session == self.session)
        })
    }

    fn process_journal(&self) -> Option<Arc<dyn NativeProcessJournal>> {
        self.owner.journal.clone()
    }

    fn authorizes_child(&self, session: &str, activation: &str) -> bool {
        self.session == session && self.activation == activation
    }
}

impl NativeRunLease {
    pub(super) fn begin_handoff(
        &self,
        child_session: &str,
        activation: &str,
        pause: Box<dyn Send + Sync>,
    ) -> Result<(), String> {
        let mut handoff = self.owner.handoff.lock().map_err(|_| "Native run lease is unavailable")?;
        if handoff.is_some() {
            return Err("Checkout already assigned to a child activation".into());
        }
        *handoff = Some(Handoff { activation: activation.into(), child_session: child_session.into(), _pause: pause });
        Ok(())
    }

    pub fn child_checkout_lease(&self, session: &str, activation: &str) -> Result<ChildCheckoutLease, String> {
        let lease =
            ChildCheckoutLease { owner: self.owner.clone(), session: session.into(), activation: activation.into() };
        use minerva_runtime::run_supervisor::CheckoutWriteLease;
        if !lease.may_write() {
            return Err("Checkout is not assigned to this child".into());
        }
        Ok(lease)
    }

    pub fn active_child(&self) -> Option<(String, String)> {
        self.owner
            .handoff
            .lock()
            .ok()?
            .as_ref()
            .map(|handoff| (handoff.child_session.clone(), handoff.activation.clone()))
    }

    pub fn child_holds_write(&self, activation: &str) -> bool {
        self.owner
            .handoff
            .lock()
            .is_ok_and(|handoff| handoff.as_ref().is_some_and(|handoff| handoff.activation == activation))
    }

    /// Call only after the exact activation and its processes are confirmed settled.
    pub fn finish_handoff(&self, activation: &str) -> Result<(), String> {
        let mut handoff = self.owner.handoff.lock().map_err(|_| "Native run lease is unavailable")?;
        if !handoff.as_ref().is_some_and(|handoff| handoff.activation == activation) {
            return Err("Checkout is not assigned to this activation".into());
        }
        handoff.take();
        Ok(())
    }

    pub fn parent_holds_write(&self) -> bool {
        self.owner.handoff.lock().is_ok_and(|handoff| handoff.is_none())
    }
}

#[derive(Default)]
pub(super) struct NativeRunLeases(Mutex<HashMap<String, HashMap<String, Weak<RunOwner>>>>);

impl NativeRunLeases {
    pub(super) fn bind(
        &self,
        session: &str,
        run: &str,
        checkout: Arc<Owner>,
        service: Weak<crate::Service>,
        journal: Option<Arc<dyn NativeProcessJournal>>,
    ) -> Result<NativeRunLease, String> {
        let mut leases = self.0.lock().map_err(|_| "Native run leases are unavailable")?;
        leases.retain(|_, runs| {
            runs.retain(|_, owner| owner.strong_count() != 0);
            !runs.is_empty()
        });
        let runs = leases.entry(session.into()).or_default();
        if runs.contains_key(run) {
            return Err("Native run already has a checkout lease".into());
        }
        // Each registration expires independently of other references to the checkout.
        let owner = Arc::new(RunOwner { service, _checkout: checkout, handoff: Mutex::new(None), journal });
        runs.insert(run.into(), Arc::downgrade(&owner));
        Ok(NativeRunLease { owner })
    }

    pub(super) fn get(&self, session: &str, run: &str) -> Option<NativeRunLease> {
        let leases = self.0.lock().ok()?;
        let owner = leases.get(session)?.get(run)?.upgrade()?;
        Some(NativeRunLease { owner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_activation_must_settle_to_return_write_access() {
        let leases = NativeRunLeases::default();
        let parent = leases.bind("root", "run", Arc::new(()), Weak::new(), None).unwrap();
        let pause = Arc::new(());
        let pause_held = Arc::downgrade(&pause);
        assert!(parent.parent_holds_write());
        parent.begin_handoff("del_child", "activation", Box::new(pause)).unwrap();
        assert!(!parent.parent_holds_write());
        assert!(parent.child_holds_write("activation"));
        assert!(!parent.child_holds_write("previous"));
        assert!(parent.child_checkout_lease("del_other", "activation").is_err());
        let child = parent.child_checkout_lease("del_child", "activation").unwrap();
        use minerva_runtime::run_supervisor::CheckoutWriteLease;
        assert!(child.may_write());
        assert!(parent.begin_handoff("del_other", "second", Box::new(())).is_err());
        let other = leases.get("root", "run").unwrap();
        drop(parent);
        assert!(other.finish_handoff("previous").is_err());
        assert!(pause_held.upgrade().is_some());
        other.finish_handoff("activation").unwrap();
        assert!(other.parent_holds_write());
        assert!(!child.may_write());
        assert!(pause_held.upgrade().is_none());
    }
}
