use minerva_plugin_sdk::delegation::{DelegationError, DelegationResult, ErrorCode};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionPhase {
    Reserved,
    Dispatched,
    Unknown,
    Settled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Admission {
    #[serde(default)]
    pub requested_config: BTreeMap<String, String>,
    #[serde(default)]
    pub parent_session_id: String,
    #[serde(default)]
    pub root_session_id: String,
    #[serde(default)]
    pub delegation_id: String,
    pub operation_id: String,
    pub payload_hash: String,
    pub task_id: String,
    pub activation_id: String,
    pub request_id: String,
    #[serde(default)]
    pub codex_scoped_tools: bool,
    pub host_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    pub phase: AdmissionPhase,
    #[serde(default)]
    pub completion: Option<minerva_plugin_sdk::delegation::Completion>,
}

/// One ledger per project. Reservations are durable intent, not execution receipts.
pub struct AdmissionStore {
    path: PathBuf,
    rows: Mutex<BTreeMap<String, Admission>>,
    _lock: File,
}

fn storage(e: impl std::fmt::Display) -> DelegationError {
    DelegationError::new(ErrorCode::StorageFailed, e.to_string())
}

impl AdmissionStore {
    pub(crate) fn settled_for_task(&self, task: &str, session: &str) -> bool {
        let rows = self.rows.lock();
        !rows.values().any(|row| row.task_id == task && row.phase != AdmissionPhase::Settled)
            && rows.values().any(|row| {
                row.task_id == task && row.native_session_id.as_deref() == Some(session) && row.completion.is_some()
            })
    }
    pub fn for_delegation(&self, root: &str, parent: &str, delegation: &str) -> Option<Admission> {
        self.rows
            .lock()
            .values()
            .find(|row| {
                row.root_session_id == root && row.parent_session_id == parent && row.delegation_id == delegation
            })
            .cloned()
    }
    pub fn cancel_reserved(&self, operation: &str) -> DelegationResult<Admission> {
        use minerva_plugin_sdk::delegation::{ActivationState, Completion};
        self.update(operation, |row| {
            if row.phase != AdmissionPhase::Reserved {
                return Err(DelegationError::new(ErrorCode::Conflict, "Native attempt is no longer reserved"));
            }
            row.completion = Some(Completion {
                result_id: format!("{}_result", row.activation_id),
                state: ActivationState::Cancelled,
                summary: "Cancelled before dispatch".into(),
            });
            row.phase = AdmissionPhase::Settled;
            Ok(())
        })
    }
    pub fn for_execution(&self, task_id: &str, request_id: &str) -> Option<Admission> {
        self.rows.lock().values().find(|row| row.task_id == task_id && row.request_id == request_id).cloned()
    }

    pub fn claim_dispatch(&self, operation: &str) -> DelegationResult<Admission> {
        self.update(operation, |row| {
            if row.phase != AdmissionPhase::Reserved {
                return Err(DelegationError::new(
                    ErrorCode::OutcomeUnknown,
                    "Native attempt was already dispatched or its outcome is unknown",
                ));
            }
            row.phase = AdmissionPhase::Unknown;
            Ok(())
        })
    }

    pub fn bind_native_session(&self, operation: &str, session: &str) -> DelegationResult<Admission> {
        self.update(operation, |row| {
            if session.is_empty()
                || row.phase != AdmissionPhase::Unknown
                || row.native_session_id.as_deref().is_some_and(|bound| bound != session)
            {
                return Err(DelegationError::new(
                    ErrorCode::Conflict,
                    "Native session does not match the claimed attempt",
                ));
            }
            row.native_session_id = Some(session.into());
            Ok(())
        })
    }

    pub(crate) fn cancel_before_prompt(&self, operation: &str) -> DelegationResult<Admission> {
        use minerva_plugin_sdk::delegation::{ActivationState, Completion};
        self.update(operation, |row| {
            if row.phase != AdmissionPhase::Unknown || row.host_turn_id.is_some() {
                return Err(DelegationError::new(ErrorCode::Conflict, "Native turn already has an execution identity"));
            }
            row.completion = Some(Completion {
                result_id: format!("{}_result", row.activation_id),
                state: ActivationState::Cancelled,
                summary: "Cancelled before sending the prompt".into(),
            });
            row.phase = AdmissionPhase::Settled;
            Ok(())
        })
    }

    pub fn complete_turn(
        &self,
        operation: &str,
        turn: &str,
        completion: minerva_plugin_sdk::delegation::Completion,
    ) -> DelegationResult<Admission> {
        self.update(operation, |row| {
            if row.phase == AdmissionPhase::Reserved
                || (row.phase == AdmissionPhase::Settled && row.host_turn_id.is_none())
            {
                return Err(DelegationError::new(ErrorCode::Conflict, "Native attempt was never claimed for dispatch"));
            }
            if turn.is_empty() || !completion.state.terminal() {
                return Err(DelegationError::new(
                    ErrorCode::Invalid,
                    "Native completion requires a terminal outcome and turn identity",
                ));
            }
            if row.host_turn_id.as_deref().is_some_and(|id| id != turn)
                || row.completion.as_ref().is_some_and(|old| old != &completion)
            {
                return Err(DelegationError::new(ErrorCode::Conflict, "Completion belongs to another native attempt"));
            }
            row.host_turn_id = Some(turn.into());
            row.completion = Some(completion);
            row.phase = AdmissionPhase::Settled;
            Ok(())
        })
    }
    pub fn open(project_store: &Path) -> DelegationResult<Self> {
        let dir = project_store.join("delegation");
        std::fs::create_dir_all(&dir).map_err(storage)?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("admissions.lock"))
            .map_err(storage)?;
        lock.try_lock()
            .map_err(|_| DelegationError::new(ErrorCode::Busy, "Delegation admission ledger is already open"))?;
        let path = dir.join("admissions.json");
        let rows = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(storage)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(storage(e)),
        };
        Ok(Self { path, rows: Mutex::new(rows), _lock: lock })
    }

    pub fn lookup(&self, operation: &str) -> Option<Admission> {
        self.rows.lock().get(operation).cloned()
    }

    pub fn reserve(&self, admission: Admission) -> DelegationResult<Admission> {
        if admission.operation_id.is_empty()
            || admission.payload_hash.is_empty()
            || admission.task_id.is_empty()
            || admission.activation_id.is_empty()
            || admission.request_id.is_empty()
            || admission.phase != AdmissionPhase::Reserved
            || admission.host_turn_id.is_some()
        {
            return Err(DelegationError::new(ErrorCode::Invalid, "Invalid admission intent"));
        }
        let mut rows = self.rows.lock();
        if let Some(old) = rows.get(&admission.operation_id) {
            if old.payload_hash != admission.payload_hash
                || old.parent_session_id != admission.parent_session_id
                || old.root_session_id != admission.root_session_id
                || old.delegation_id != admission.delegation_id
                || old.task_id != admission.task_id
                || old.activation_id != admission.activation_id
                || old.request_id != admission.request_id
                || old.codex_scoped_tools != admission.codex_scoped_tools
                || (admission.native_session_id.is_some() && old.native_session_id != admission.native_session_id)
            {
                return Err(DelegationError::new(ErrorCode::Conflict, "Operation already has a different intent"));
            }
            return Ok(old.clone());
        }
        let mut next = rows.clone();
        if rows.values().any(|row| row.task_id == admission.task_id && row.phase != AdmissionPhase::Settled) {
            return Err(DelegationError::new(ErrorCode::Busy, "Native task already has an unsettled attempt"));
        }
        next.insert(admission.operation_id.clone(), admission.clone());
        self.save(&next)?;
        *rows = next;
        Ok(admission)
    }

    /// Bind an identity supplied by the host before writing a prompt to an executor.
    /// The attempted delivery remains unknown until matching terminal evidence arrives.
    pub fn bind_native_turn(&self, operation: &str, session: &str, turn: &str) -> DelegationResult<Admission> {
        if turn.is_empty() {
            return Err(DelegationError::new(ErrorCode::Invalid, "Empty native turn identity"));
        }
        self.update(operation, |row| {
            if row.phase != AdmissionPhase::Unknown
                || row.native_session_id.as_deref() != Some(session)
                || row.host_turn_id.as_deref().is_some_and(|id| id != turn)
            {
                return Err(DelegationError::new(
                    ErrorCode::Conflict,
                    "Attempt already has a different or settled turn",
                ));
            }
            row.host_turn_id = Some(turn.into());
            Ok(())
        })
    }

    pub fn mark_unknown(&self, operation: &str) -> DelegationResult<Admission> {
        self.update(operation, |row| {
            if row.phase != AdmissionPhase::Settled {
                row.phase = AdmissionPhase::Unknown;
            }
            Ok(())
        })
    }

    pub fn settle(&self, operation: &str, turn: &str) -> DelegationResult<Admission> {
        self.update(operation, |row| {
            if row.host_turn_id.as_deref() != Some(turn) {
                return Err(DelegationError::new(ErrorCode::Conflict, "Completion belongs to another native turn"));
            }
            row.phase = AdmissionPhase::Settled;
            Ok(())
        })
    }

    fn update(
        &self,
        operation: &str,
        mutate: impl FnOnce(&mut Admission) -> DelegationResult<()>,
    ) -> DelegationResult<Admission> {
        let mut rows = self.rows.lock();
        let mut next = rows.clone();
        let row = next
            .get_mut(operation)
            .ok_or_else(|| DelegationError::new(ErrorCode::Invalid, "Unknown admission operation"))?;
        mutate(row)?;
        let result = row.clone();
        self.save(&next)?;
        *rows = next;
        Ok(result)
    }

    fn save(&self, rows: &BTreeMap<String, Admission>) -> DelegationResult<()> {
        let parent = self.path.parent().ok_or_else(|| storage("Missing ledger directory"))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(storage)?;
        tmp.write_all(&serde_json::to_vec(rows).map_err(storage)?).map_err(storage)?;
        tmp.as_file().sync_all().map_err(storage)?;
        tmp.persist(&self.path).map_err(storage)?;
        Ok(())
    }
}
