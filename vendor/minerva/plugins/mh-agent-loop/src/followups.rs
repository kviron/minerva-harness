use crate::providers::FollowupControl;
use mh_sessions::{delegation::PreferredFollowup, sessions::SessionService};
use minerva_plugin_sdk::delegation::ExecutionHandle;

pub(crate) struct RunFollowups {
    sessions: SessionService,
    handle: ExecutionHandle,
}

impl RunFollowups {
    pub(crate) fn new(sessions: SessionService, session_id: &str, run_id: &str) -> Self {
        Self { sessions, handle: ExecutionHandle { session_id: session_id.into(), run_id: run_id.into() } }
    }
}

impl FollowupControl for RunFollowups {
    fn preferred(&self) -> Result<Option<PreferredFollowup>, String> {
        self.sessions.preferred_delegation_followup(&self.handle).map_err(|error| error.to_string())
    }

    fn claim(&self, work_item_id: &str) -> Result<bool, String> {
        self.sessions
            .claim_delegation_followup(&self.handle, work_item_id)
            .map(|prompt| prompt.is_some())
            .map_err(|error| error.to_string())
    }

    fn confirm(&self, work_item_id: &str) -> Result<bool, String> {
        self.sessions.confirm_delegation_followup(&self.handle, work_item_id).map_err(|error| error.to_string())
    }
}
