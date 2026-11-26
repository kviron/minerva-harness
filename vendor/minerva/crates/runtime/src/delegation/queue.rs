use crate::{plugins::PluginManager, sessions::SessionService};
use minerva_delegation::{limits::Limits, DelegationService};

pub(super) fn advance(sessions: &SessionService, plugins: &PluginManager, service: &DelegationService) {
    let ready = match sessions.delegation_followups_ready(&Limits::default()) {
        Ok(ready) => ready,
        Err(error) => {
            tracing::warn!(%error, "Could not read ready delegation follow-ups");
            return;
        }
    };
    let reads = plugins.harness.tools.snapshot().delegation_reads();
    for child in ready {
        let Ok(settings) = plugins.harness.context.workspace_get(&child.caller.session_id) else { continue };
        if settings.cwd.as_deref() != Some(child.caller.cwd.as_str()) {
            continue;
        }
        if let Err(error) = service.promote_followup(&child.caller, &child.id, &reads) {
            tracing::trace!(delegation = %child.id, %error, "Delegation queue promotion deferred");
        }
    }
}
