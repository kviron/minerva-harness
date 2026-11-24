//! Adapts the session-owned selection journal to provider execution.
use crate::{providers::SelectionControl, sessions::SessionService};
use mh_sessions::{DispatchAdmission, RouteDispatch, RouteSelection};
use std::{future::Future, pin::Pin, sync::Arc};

pub(crate) struct SessionSelection {
    sessions: SessionService,
    session: String,
    run: String,
}

impl SessionSelection {
    pub(crate) async fn admit(
        sessions: SessionService,
        session: String,
        run: String,
        model: String,
        effort: Option<String>,
    ) -> Result<(Arc<dyn SelectionControl>, RouteSelection), String> {
        let selection = Self { sessions, session, run };
        let store = selection.sessions.clone();
        let id = selection.session.clone();
        let provider =
            model.split_once('/').map(|(provider, _)| provider.to_owned()).ok_or("Invalid API model route")?;
        let initial = tokio::task::spawn_blocking(move || store.ensure_selection(&id, &model, effort.as_deref()))
            .await
            .map_err(|error| error.to_string())??;
        if initial.model.split_once('/').map(|(provider, _)| provider) != Some(provider.as_str()) {
            return Err("Live model selection cannot change providers during a turn".into());
        }
        Ok((Arc::new(selection), initial))
    }
}

impl SelectionControl for SessionSelection {
    fn selection(&self) -> Pin<Box<dyn Future<Output = Result<RouteSelection, String>> + Send + '_>> {
        let sessions = self.sessions.clone();
        let session = self.session.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || sessions.selection(&session)?.ok_or("Missing model selection".into()))
                .await
                .map_err(|error| error.to_string())?
        })
    }

    fn record_dispatch(
        &self,
        dispatch: RouteDispatch,
    ) -> Pin<Box<dyn Future<Output = Result<DispatchAdmission, String>> + Send + '_>> {
        let sessions = self.sessions.clone();
        let session = self.session.clone();
        let run = self.run.clone();
        Box::pin(async move {
            if dispatch.run_id != run {
                return Err("Model dispatch belongs to another run".into());
            }
            tokio::task::spawn_blocking(move || sessions.record_selection_dispatch(&session, &dispatch))
                .await
                .map_err(|error| error.to_string())?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admission_rejects_a_pending_foreign_provider_before_context_preparation() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionService::open(&dir.path().join("sessions.sqlite3")).unwrap();
        store.create("session").unwrap();
        store.request_selection("session", "api:foreign/model", None).unwrap();
        assert!(SessionSelection::admit(
            store.clone(),
            "session".into(),
            "run".into(),
            "api:original/model".into(),
            None
        )
        .await
        .is_err());
        store.request_selection("session", "api:original/latest", None).unwrap();
        let (_, initial) =
            SessionSelection::admit(store, "session".into(), "run".into(), "api:original/old".into(), None)
                .await
                .unwrap();
        assert_eq!(initial.model, "api:original/latest");
    }
}
