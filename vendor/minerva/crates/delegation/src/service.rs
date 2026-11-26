use crate::{limits::Limits, repository::RepositoryPort, *};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

pub struct FollowupRequest {
    pub operation_id: String,
    pub prompt: String,
    pub origin_run: Option<String>,
    pub tool_registrations: BTreeMap<String, u64>,
}

#[derive(serde::Serialize)]
#[serde(tag = "disposition", rename_all = "kebab-case")]
pub enum FollowupReceipt {
    Queued(crate::repository::QueuedFollowup),
    Activation(Box<DelegationSnapshot>),
}

pub struct DelegationService {
    repository: Arc<dyn RepositoryPort>,
    executor: Arc<dyn ExecutionPort>,
    limits: Limits,
    fallbacks: Vec<FallbackRule>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    starting: Mutex<BTreeMap<String, tokio::task::AbortHandle>>,
    admission: RwLock<bool>,
}

impl DelegationService {
    pub fn new(repository: Arc<dyn RepositoryPort>, executor: Arc<dyn ExecutionPort>, limits: Limits) -> Self {
        Self {
            repository,
            executor,
            limits,
            fallbacks: Vec::new(),
            tasks: Mutex::new(Vec::new()),
            starting: Mutex::new(BTreeMap::new()),
            admission: RwLock::new(true),
        }
    }

    pub fn with_fallbacks(mut self, fallbacks: Vec<FallbackRule>) -> Self {
        self.fallbacks = fallbacks;
        self
    }

    pub async fn discover(&self, caller: &Caller) -> DelegationResult<Vec<Candidate>> {
        validate_caller(caller)?;
        self.executor.discover(caller).await
    }

    pub async fn start(&self, caller: &Caller, request: &StartRequest) -> DelegationResult<DelegationSnapshot> {
        self.start_if(caller, request, || true).await
    }

    pub async fn start_if(
        &self,
        caller: &Caller,
        request: &StartRequest,
        admission_allowed: impl Fn() -> bool + Send + Sync,
    ) -> DelegationResult<DelegationSnapshot> {
        self.start_using(caller, request, admission_allowed, None).await
    }

    /// Trusted runtime entry for a one-shot execution grant. The grant is never
    /// created for the selected attempt after any pre-admission fallback.
    pub async fn start_scoped(
        &self,
        caller: &Caller,
        request: &StartRequest,
        admission_allowed: impl Fn() -> bool + Send + Sync,
        executor: Arc<dyn ExecutionPort>,
    ) -> DelegationResult<DelegationSnapshot> {
        self.start_using(caller, request, admission_allowed, Some(executor)).await
    }

    async fn start_using(
        &self,
        caller: &Caller,
        request: &StartRequest,
        admission_allowed: impl Fn() -> bool + Send + Sync,
        execution_scope: Option<Arc<dyn ExecutionPort>>,
    ) -> DelegationResult<DelegationSnapshot> {
        validate_caller(caller)?;
        if !valid_id(&request.operation_id)
            || request.prompt.trim().is_empty()
            || request.prompt.len() > self.limits.prompt_bytes
        {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid delegation operation or prompt"));
        }
        if let Some(prior) = self.repository.lookup_operation(caller, request)? {
            return Ok(prior);
        }
        if !*self.admission.read().unwrap_or_else(|p| p.into_inner()) {
            return Err(DelegationError::new(ErrorCode::Unavailable, "Delegation admission is closed"));
        }
        let candidates = self.executor.discover(caller).await?;
        let check = |target| {
            if execution_scope.as_ref().is_some_and(|scope| !scope.supports_scoped_write(caller, target)) {
                return Err(DelegationError::new(ErrorCode::Unavailable, "Target cannot accept delegated writes"));
            }
            self.check_candidate(caller, target, &candidates)
        };
        let fallback = match check(&request.target) {
            Ok(()) => None,
            Err(error) if error.code == ErrorCode::Unavailable => {
                if let Some(prior) = self.repository.lookup_operation(caller, request)? {
                    return Ok(prior);
                }
                let Some(rule) = self.fallbacks.iter().find(|rule| rule.primary == request.target) else {
                    return Err(error);
                };
                check(&rule.alternative)?;
                Some((&rule.alternative, error.message))
            }
            Err(error) => return Err(error),
        };
        // The gate protects only short admission + ownership transfer, never executor awaits.
        let gate = self.admission.read().unwrap_or_else(|p| p.into_inner());
        if !*gate {
            if let Some(prior) = self.repository.lookup_operation(caller, request)? {
                return Ok(prior);
            }
            return Err(DelegationError::new(ErrorCode::Unavailable, "Delegation admission is closed"));
        }
        if !admission_allowed() {
            if let Some(prior) = self.repository.lookup_operation(caller, request)? {
                return Ok(prior);
            }
            return Err(DelegationError::new(ErrorCode::Cancelled, "Delegation cancelled before admission"));
        }
        let admission = match fallback {
            Some((target, reason)) => self.repository.admit_fallback(caller, request, target, &reason, &self.limits)?,
            None => self.repository.admit(caller, request, &self.limits)?,
        };
        Ok(self.dispatch_using(admission, execution_scope.unwrap_or_else(|| self.executor.clone())))
    }

    fn check_candidate(&self, caller: &Caller, target: &Target, candidates: &[Candidate]) -> DelegationResult<()> {
        if !matches!(self.executor.capabilities_for(caller, target).read_only, Capability::Supported { .. }) {
            return Err(DelegationError::new(ErrorCode::Unsupported, "Target read-only capability is unverified"));
        }
        let Some(candidate) = candidates.iter().find(|candidate| &candidate.target == target && candidate.read_only)
        else {
            return Err(DelegationError::new(ErrorCode::Unsupported, "Target cannot guarantee read-only execution"));
        };
        match &candidate.unavailable_reason {
            Some(reason) => Err(DelegationError::new(ErrorCode::Unavailable, reason.clone())),
            None => Ok(()),
        }
    }

    pub fn followup_if(
        &self,
        caller: &Caller,
        id: &str,
        input: FollowupRequest,
        admission_allowed: impl FnOnce() -> bool,
    ) -> DelegationResult<FollowupReceipt> {
        let FollowupRequest { operation_id, prompt, origin_run, tool_registrations } = input;
        if !valid_id(&operation_id) || prompt.trim().is_empty() || prompt.len() > self.limits.prompt_bytes {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid delegation operation or prompt"));
        }
        validate_caller(caller)?;
        let prior = || self.lookup_followup(caller, id, &operation_id, &prompt);
        if let Some(snapshot) = prior()? {
            return Ok(snapshot);
        }
        let child = self.get(caller, id)?;
        let gate = self.admission.read().unwrap_or_else(|p| p.into_inner());
        if !*gate || !admission_allowed() {
            if let Some(snapshot) = prior()? {
                return Ok(snapshot);
            }
            return Err(DelegationError::new(ErrorCode::Cancelled, "Delegation admission is closed or cancelled"));
        }
        let capabilities = self.executor.capabilities_for(caller, &child.request.target);
        if !matches!(capabilities.read_only, Capability::Supported { .. })
            || !matches!(capabilities.continuation, Capability::Supported { .. })
        {
            return Err(DelegationError::new(
                ErrorCode::Unsupported,
                "Target cannot continue this child in read-only mode",
            ));
        }
        let request =
            StartRequest { operation_id, prompt, origin_run, tool_registrations, target: child.request.target };
        if child.state == ActivationState::Succeeded {
            match self.repository.admit_followup(caller, id, &request, &self.limits) {
                Ok(admission) => return Ok(FollowupReceipt::Activation(Box::new(self.dispatch(admission)))),
                Err(error) if error.code == ErrorCode::Busy => {}
                Err(error) => {
                    return self.lookup_followup(caller, id, &request.operation_id, &request.prompt)?.ok_or(error)
                }
            }
        }
        match self.repository.enqueue(caller, id, &request, &self.limits) {
            Ok(receipt) => Ok(FollowupReceipt::Queued(receipt)),
            Err(error) => self.lookup_followup(caller, id, &request.operation_id, &request.prompt)?.ok_or(error),
        }
    }

    pub fn lookup_followup(
        &self,
        caller: &Caller,
        id: &str,
        operation: &str,
        prompt: &str,
    ) -> DelegationResult<Option<FollowupReceipt>> {
        if let Some(receipt) = self.repository.lookup_queued(caller, id, operation, prompt)? {
            return Ok(Some(FollowupReceipt::Queued(receipt)));
        }
        match self.repository.lookup_followup(caller, id, operation, prompt) {
            Ok(snapshot) => Ok(snapshot.map(|s| FollowupReceipt::Activation(Box::new(s)))),
            Err(error) if error.code == ErrorCode::Conflict => {
                match self.repository.lookup_queued(caller, id, operation, prompt)? {
                    Some(receipt) => Ok(Some(FollowupReceipt::Queued(receipt))),
                    None => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn followup_scoped(
        &self,
        caller: &Caller,
        id: &str,
        request: &StartRequest,
        admission_allowed: impl Fn() -> bool + Send + Sync,
        executor: Arc<dyn ExecutionPort>,
    ) -> DelegationResult<DelegationSnapshot> {
        validate_caller(caller)?;
        if !valid_id(&request.operation_id)
            || request.prompt.trim().is_empty()
            || request.prompt.len() > self.limits.prompt_bytes
        {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid writable follow-up"));
        }
        let prior = || self.repository.lookup_scoped_followup(caller, id, &request.operation_id, &request.prompt);
        if let Some(prior) = prior()? {
            return Ok(prior);
        }
        if !*self.admission.read().unwrap_or_else(|poisoned| poisoned.into_inner()) || !admission_allowed() {
            return prior()?.ok_or_else(|| {
                DelegationError::new(ErrorCode::Cancelled, "Delegation admission is closed or cancelled")
            });
        }
        let child = self.get(caller, id)?;
        if child.state != ActivationState::Succeeded || child.handle.is_none() {
            return prior()?
                .ok_or_else(|| DelegationError::new(ErrorCode::Busy, "Child has no successful run to continue"));
        }
        let target = &child.request.target;
        let capabilities = self.executor.capabilities_for(caller, target);
        if !matches!(capabilities.continuation, Capability::Supported { .. })
            || !executor.supports_scoped_write(caller, target)
        {
            return Err(DelegationError::new(ErrorCode::Unsupported, "Target cannot continue with a write grant"));
        }
        let eligibility = executor.check_scoped_followup(&child).await;
        let gate = self.admission.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(prior) = prior()? {
            return Ok(prior);
        }
        if !*gate || !admission_allowed() {
            return Err(DelegationError::new(ErrorCode::Cancelled, "Delegation admission is closed or cancelled"));
        }
        eligibility?;
        let admission = self.repository.admit_scoped_followup(caller, id, request, &self.limits)?;
        Ok(self.dispatch_using(admission, executor))
    }

    pub fn promote_followup(
        &self,
        caller: &Caller,
        id: &str,
        parent_reads: &BTreeMap<String, u64>,
    ) -> DelegationResult<()> {
        let gate = self.admission.read().unwrap_or_else(|p| p.into_inner());
        if !*gate {
            return Ok(());
        }
        let child = self.get(caller, id)?;
        let capabilities = self.executor.capabilities_for(caller, &child.request.target);
        if !matches!(capabilities.read_only, Capability::Supported { .. })
            || !matches!(capabilities.continuation, Capability::Supported { .. })
        {
            return Err(DelegationError::new(ErrorCode::Unsupported, "Queued target cannot resume in read-only mode"));
        }
        if let Some(snapshot) = self.repository.promote(caller, id, parent_reads, &self.limits)? {
            self.dispatch(crate::repository::Admission { snapshot, fresh: true });
        }
        Ok(())
    }

    fn dispatch(&self, admission: crate::repository::Admission) -> DelegationSnapshot {
        self.dispatch_using(admission, self.executor.clone())
    }

    fn dispatch_using(
        &self,
        admission: crate::repository::Admission,
        executor: Arc<dyn ExecutionPort>,
    ) -> DelegationSnapshot {
        let snapshot = admission.snapshot;
        if !admission.fresh {
            return snapshot;
        }
        let repository = self.repository.clone();
        let owned = snapshot.clone();
        let summary_bytes = self.limits.summary_bytes;
        // No await between durable admission and ownership transfer to the runtime.
        let mut starting = self.starting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let task = tokio::spawn(async move {
            let result = execute_start(repository.as_ref(), executor.as_ref(), &owned, summary_bytes).await;
            if let Err(error) = result {
                let _ = repository.mark_unknown(&owned.id, &owned.activation_id, &error.message);
            }
        });
        starting.retain(|_, task| !task.is_finished());
        starting.insert(snapshot.activation_id.clone(), task.abort_handle());
        drop(starting);
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
        snapshot
    }

    /// Called once during runtime bootstrap after the repository proves the local run was never admitted.
    pub fn dispatch_recovered(&self, snapshot: DelegationSnapshot) {
        let gate = self.admission.read().unwrap_or_else(|p| p.into_inner());
        if *gate {
            self.dispatch(crate::repository::Admission { snapshot, fresh: true });
        }
    }

    pub fn close_admission(&self) {
        *self.admission.write().unwrap_or_else(|p| p.into_inner()) = false;
    }

    pub async fn drain_starts(&self) {
        let tasks = {
            let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *tasks)
        };
        for task in tasks {
            let _ = task.await;
        }
    }

    /// Timed-out start tasks remain tracked; no timeout proves engine exit.
    pub async fn drain_starts_until(&self, timeout: Duration) -> bool {
        let tasks = {
            let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
            std::mem::take(&mut *tasks)
        };
        let deadline = tokio::time::Instant::now() + timeout;
        let mut pending = Vec::new();
        for mut task in tasks {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                pending.push(task);
            }
        }
        let done = pending.is_empty();
        self.tasks.lock().unwrap_or_else(|p| p.into_inner()).extend(pending);
        done
    }

    pub fn get(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot> {
        validate_caller(caller)?;
        self.repository.get(caller, id)
    }

    pub fn list(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
        validate_caller(caller)?;
        self.repository.list(caller)
    }

    pub async fn poll(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot> {
        let snapshot = self.get(caller, id)?;
        if snapshot.handle.is_none()
            && (snapshot.state == ActivationState::Starting
                || (snapshot.state == ActivationState::Stopping
                    && self
                        .starting
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(&snapshot.activation_id)
                        .is_some_and(|task| !task.is_finished())))
        {
            // Dispatch owns the attempt until it records a handle or a failed start.
            return Ok(snapshot);
        }
        if !snapshot.state.terminal() {
            let observed = match self.executor.observe(&snapshot).await {
                Err(error) if error.code == ErrorCode::OutcomeUnknown => {
                    self.repository.mark_unknown(&snapshot.id, &snapshot.activation_id, &error.message)?;
                    return self.get(caller, id);
                }
                other => other?,
            };
            if let Some(mut completion) = observed {
                if !completion.state.terminal() || !valid_id(&completion.result_id) {
                    return Err(DelegationError::new(ErrorCode::Invalid, "Executor returned an invalid completion"));
                }
                completion.summary = bounded(&completion.summary, self.limits.summary_bytes);
                self.repository.settle(&snapshot.id, &snapshot.activation_id, &completion)?;
            }
        }
        self.get(caller, id)
    }

    pub async fn interrupt(&self, caller: &Caller, id: &str) -> DelegationResult<DelegationSnapshot> {
        validate_caller(caller)?;
        let snapshot = self.repository.request_stop(caller, id)?;
        if !snapshot.state.terminal() {
            self.executor.interrupt(&snapshot).await?;
        }
        self.get(caller, id)
    }

    pub fn interrupt_tree(&self, caller: &Caller) -> DelegationResult<Vec<DelegationSnapshot>> {
        validate_caller(caller)?;
        let snapshots = self.repository.request_stop_tree(caller)?;
        self.interrupt_activations(&snapshots);
        Ok(snapshots)
    }

    pub fn interrupt_activations(&self, snapshots: &[DelegationSnapshot]) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        tasks.retain(|task| !task.is_finished());
        for snapshot in snapshots {
            let executor = self.executor.clone();
            let repository = self.repository.clone();
            let snapshot = snapshot.clone();
            tasks.push(tokio::spawn(async move {
                let reason = match tokio::time::timeout(Duration::from_secs(5), executor.interrupt(&snapshot)).await {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => error.message,
                    Err(_) => "Cancellation request timed out".into(),
                };
                // With no handle, leave the stop intent available to an in-flight start.
                if snapshot.handle.is_some() {
                    let _ = repository.mark_unknown(&snapshot.id, &snapshot.activation_id, &reason);
                }
            }));
        }
    }

    /// Bounded observation only; dropping this future does not cancel the child.
    pub async fn wait(&self, caller: &Caller, id: &str, timeout: Duration) -> DelegationResult<DelegationSnapshot> {
        let deadline = tokio::time::Instant::now() + timeout.min(Duration::from_secs(30));
        loop {
            let snapshot = self.poll(caller, id).await?;
            if snapshot.state.terminal()
                || snapshot.state == ActivationState::Unknown
                || tokio::time::Instant::now() >= deadline
            {
                return Ok(snapshot);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn validate_caller(caller: &Caller) -> DelegationResult<()> {
    if !valid_id(&caller.session_id)
        || !valid_id(&caller.root_session_id)
        || caller.workspace_id.is_empty()
        || caller.cwd.is_empty()
    {
        return Err(DelegationError::new(ErrorCode::Invalid, "Invalid trusted delegation context"));
    }
    Ok(())
}
fn bounded(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

async fn execute_start(
    repository: &dyn RepositoryPort,
    executor: &dyn ExecutionPort,
    snapshot: &DelegationSnapshot,
    summary_bytes: usize,
) -> DelegationResult<()> {
    let current = repository.get(&snapshot.caller, &snapshot.id)?;
    if current.activation_id != snapshot.activation_id || current.state.terminal() {
        return Ok(());
    }
    if current.state == ActivationState::Stopping {
        return repository.settle(
            &snapshot.id,
            &snapshot.activation_id,
            &Completion {
                result_id: format!("{}-start", snapshot.activation_id),
                state: ActivationState::Cancelled,
                summary: "Delegation cancelled before execution".into(),
            },
        );
    }
    match executor.start(snapshot).await {
        Ok(handle) => {
            repository.started(&snapshot.id, &snapshot.activation_id, &handle)?;
            let current = repository.get(&snapshot.caller, &snapshot.id)?;
            if current.state == ActivationState::Stopping {
                executor.interrupt(&current).await?;
            }
        }
        Err(error) if error.code == ErrorCode::OutcomeUnknown => {
            repository.mark_unknown(&snapshot.id, &snapshot.activation_id, &error.message)?
        }
        Err(error) => repository.settle(
            &snapshot.id,
            &snapshot.activation_id,
            &Completion {
                result_id: format!("{}-start", snapshot.activation_id),
                state: if error.code == ErrorCode::Cancelled {
                    ActivationState::Cancelled
                } else {
                    ActivationState::Failed
                },
                summary: bounded(&error.message, summary_bytes),
            },
        )?,
    }
    Ok(())
}
