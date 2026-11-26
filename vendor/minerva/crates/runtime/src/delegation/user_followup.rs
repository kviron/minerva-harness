use crate::Runtime;
use minerva_delegation::{Caller, FollowupReceipt, FollowupRequest, IDE_USER_OPERATION_PREFIX};

impl Runtime {
    /// Trusted IDE entry. Model tools use the direct-parent admission path.
    pub async fn followup_delegation_from_user(
        &self,
        root: &Caller,
        id: &str,
        operation: &str,
        prompt: String,
    ) -> Result<FollowupReceipt, String> {
        if operation.is_empty() {
            return Err("A direct follow-up requires an operation ID".into());
        }
        let resolved = self
            .sessions
            .delegation_caller(&root.session_id, &root.workspace_id, &root.cwd)
            .map_err(|error| error.to_string())?;
        if resolved != *root || root.session_id != root.root_session_id {
            return Err("Only the root conversation can address a child on behalf of the user".into());
        }
        let service = self
            .delegation
            .lock()
            .await
            .as_ref()
            .map(|runtime| runtime.service.clone())
            .ok_or("Delegation is disabled")?;
        let child = service.get(root, id).map_err(|error| error.to_string())?;
        let operation_id = format!("{IDE_USER_OPERATION_PREFIX}{operation}");
        if let Some(receipt) =
            service.lookup_followup(&child.caller, id, &operation_id, &prompt).map_err(|error| error.to_string())?
        {
            return Ok(receipt);
        }
        let current = self.plugins.harness.context.workspace_get(&child.caller.session_id)?;
        if current.cwd.as_deref() != Some(child.caller.cwd.as_str()) {
            return Err("The parent workspace changed after delegation".into());
        }
        service
            .followup_if(
                &child.caller,
                id,
                FollowupRequest {
                    operation_id,
                    prompt,
                    origin_run: None,
                    tool_registrations: self.plugins.harness.tools.snapshot().delegation_reads(),
                },
                || true,
            )
            .map_err(|error| error.to_string())
    }
}
