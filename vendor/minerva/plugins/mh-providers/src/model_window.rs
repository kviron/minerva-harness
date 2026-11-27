//! Capacity advertised by one endpoint for one model and the optional operator limit.
use serde::{Deserialize, Serialize};

/// The catalog contract shared by provider adapters, discovery clients and the IDE.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context_window: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub discovered: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

impl ModelWindow {
    pub(crate) fn valid(self) -> bool {
        self.discovered != Some(0) && self.limit.is_none_or(|n| n >= 1024)
    }
    pub(crate) fn resolve(self, catalog: Option<u32>) -> Option<u32> {
        let advertised = self.discovered.or(catalog);
        match (self.limit, advertised) {
            (Some(limit), Some(window)) => Some(limit.min(window)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_and_user_cap_are_independent() {
        let settings = ModelWindow { discovered: Some(64000), limit: Some(98000) };
        assert_eq!(settings.resolve(None), Some(64000));
        let large = ModelWindow { discovered: Some(10_000_000), limit: Some(4_000_000) };
        assert!(large.valid());
        assert_eq!(large.resolve(None), Some(4_000_000));
        assert_eq!(ModelWindow { discovered: Some(128000), ..settings }.resolve(None), Some(98000));
        assert_eq!(ModelWindow::default().resolve(Some(500000)), Some(500000));
    }
}
