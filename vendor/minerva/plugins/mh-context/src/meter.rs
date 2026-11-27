//! The last request estimate is keyed by the branch head and provider route.
//! It contains counts only. No prompts, tool payloads or credentials enter the meter.
use super::ContextService;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

pub fn pressure_threshold(window: usize) -> usize {
    window.saturating_mul(4) / 5
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMeasure {
    pub model: String,
    pub head: String,
    pub window: usize,
    pub system: usize,
    pub tools: usize,
    pub messages: usize,
    #[serde(default)]
    pub observed_input: Option<u32>,
    pub threshold: usize,
    pub auto: bool,
}

impl RequestMeasure {
    pub fn used(&self) -> usize {
        self.observed_input.map(|n| n as usize).unwrap_or_else(|| self.estimated())
    }
    pub fn estimated(&self) -> usize {
        self.system.saturating_add(self.tools).saturating_add(self.messages)
    }
}

impl ContextService {
    pub fn record_request(&self, session: &str, report: &RequestMeasure) -> Result<(), String> {
        let json = serde_json::to_string(report).map_err(|e| e.to_string())?;
        self.db()?
            .execute(
                "INSERT INTO request_meter VALUES (?1, ?2) ON CONFLICT(session) DO UPDATE SET report=excluded.report",
                params![session, json],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn confirm_request(&self, session: &str, run: &str, input: Option<u64>) -> Result<(), String> {
        if let Some(tokens) = input.and_then(|n| u32::try_from(n).ok()) {
            self.db()?.execute(
                "UPDATE request_meter SET report=json_set(report,'$.observed_input',?3) WHERE session=?1 AND json_extract(report,'$.head')=?2",
                params![session, run, tokens],
            ).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn last_request(&self, session: &str) -> Result<Option<RequestMeasure>, String> {
        let row: Option<String> = self
            .db()?
            .query_row("SELECT report FROM request_meter WHERE session=?1", [session], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        row.map(|s| serde_json::from_str(&s)).transpose().map_err(|e| e.to_string())
    }
}
