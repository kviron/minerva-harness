//! Validated selection API; persistence remains in SessionStore.
use super::*;

pub(super) fn validate_selection(id: &str, model: &str, effort: Option<&str>) -> Result<(), String> {
    if !valid_id(id)
        || model.trim().is_empty()
        || model.len() > 2048
        || model.chars().any(char::is_control)
        || effort.is_some_and(|value| value.is_empty() || value.len() > 128 || value.chars().any(char::is_control))
    {
        return Err("Invalid model selection".into());
    }
    Ok(())
}

impl SessionService {
    pub fn selection(&self, id: &str) -> Result<Option<crate::RouteSelection>, String> {
        self.store.selection(id)
    }
    pub fn request_selection(
        &self,
        id: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<crate::RouteSelection, String> {
        self.write_selection(id, model, effort, None, false)
    }
    pub fn request_selection_if_revision(
        &self,
        id: &str,
        revision: i64,
        model: &str,
        effort: Option<&str>,
    ) -> Result<crate::RouteSelection, String> {
        self.write_selection(id, model, effort, Some(revision), false)
    }
    pub fn ensure_selection(
        &self,
        id: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<crate::RouteSelection, String> {
        self.write_selection(id, model, effort, None, true)
    }
    fn write_selection(
        &self,
        id: &str,
        model: &str,
        effort: Option<&str>,
        expected: Option<i64>,
        absent: bool,
    ) -> Result<crate::RouteSelection, String> {
        validate_selection(id, model, effort)?;
        self.store.request_selection(id, model, effort, expected, absent)
    }
    pub fn record_selection_dispatch(
        &self,
        id: &str,
        dispatch: &crate::RouteDispatch,
    ) -> Result<crate::DispatchAdmission, String> {
        if !valid_id(id)
            || !valid_id(&dispatch.run_id)
            || dispatch.effect_id.is_empty()
            || dispatch.effect_id.len() > 1024
            || dispatch.effect_id.chars().any(char::is_control)
        {
            return Err("Invalid route dispatch identity".into());
        }
        self.store.record_selection_dispatch(id, dispatch)
    }
}
